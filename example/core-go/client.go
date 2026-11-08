package main

// The Go core's business logic: plain Go against the GENERATED contract types
// (wabi_generated.go, from wit/client.wit). Everything ABI-related — handles,
// futures, streams, codec, the exported C symbols — lives in the generated
// file and the runtime module wabi.example/wabiabi (runtime/go).
//
// It is a second, independent implementation of the Rust core's contract and
// of its HTTP protocol (session.go): the cross-language tests run it against
// the same mock service (demo-mock-server).

import (
	"context"
	"sync"
	"sync/atomic"
	"time"

	wabiabi "wabi.example/wabiabi"
)

const coreInfo = "demo-go-core/0.3.0 (goroutines, net/http)"

var (
	errNotFound     = &ClientError{Tag: ClientErrorNotFound}
	errUnauthorized = &ClientError{Tag: ClientErrorUnauthorized}
	errDisconnected = &ClientError{Tag: ClientErrorDisconnected}
)

func networkError(m string) *ClientError {
	return &ClientError{Tag: ClientErrorNetworkError, NetworkError: m}
}

func protocolError(m string) *ClientError {
	return &ClientError{Tag: ClientErrorProtocolError, ProtocolError: m}
}

func keyError(m string) *ClientError {
	return &ClientError{Tag: ClientErrorKeyError, KeyError: m}
}

// transport is the connection pool to one backend, shared by every client
// that was given it (resource `transport` of the contract).
type transport struct {
	base     string // endpoint without trailing slash
	requests atomic.Uint64
}

// newTransport is the constructor the generated dispatcher calls.
func newTransport(endpoint string) Transport { return &transport{base: endpoint} }

func (t *transport) RequestCount() uint64 { return t.requests.Load() }

func (t *transport) OpenClient(o ClientOptions) Client {
	c := newClient(o).(*client)
	c.transport = t
	return c
}

// Absorb: `other` was moved in by the host, nobody else can reach it any more.
func (t *transport) Absorb(other Transport) { t.requests.Add(other.RequestCount()) }

func (t *transport) Close() {}

type client struct {
	opts ClientOptions

	lifecycle sync.Mutex // serialises connect/disconnect

	mu      sync.Mutex
	state   ConnectionState
	closed  bool
	session *session // current session; ended on disconnect/hang-up/close
	// What the next session sends its requests over; a private one unless
	// the host gave the client a shared transport.
	transport *transport
	subs    map[int]chan ClientEvent
	nextSub int
}

// newClient is the constructor the generated dispatcher calls.
func newClient(o ClientOptions) Client {
	return &client{opts: o, state: ConnectionStateDisconnected, subs: map[int]chan ClientEvent{}}
}

// publish delivers an event to every subscription. Slow subscribers drop
// events instead of blocking the core (same policy as the Rust core).
func (c *client) publish(e ClientEvent) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.publishLocked(e)
}

func (c *client) publishLocked(e ClientEvent) {
	for _, ch := range c.subs {
		select {
		case ch <- e:
		default:
		}
	}
}

// setState must be called with c.mu held.
func (c *client) setState(s ConnectionState) {
	if c.state != s {
		c.state = s
		c.publishLocked(ClientEvent{Tag: ClientEventConnectionStateChanged, ConnectionStateChanged: s})
	}
}

func (c *client) UseTransport(t Transport) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.transport = t.(*transport)
}

func (c *client) State() ConnectionState {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.state
}

func (c *client) Connect(ctx context.Context) *ClientError {
	c.lifecycle.Lock()
	defer c.lifecycle.Unlock()
	c.mu.Lock()
	if c.closed {
		c.mu.Unlock()
		return errDisconnected
	}
	if c.session != nil && c.session.alive() {
		c.mu.Unlock()
		return nil // already connected: idempotent
	}
	c.setState(ConnectionStateConnecting)
	link := c.transport
	if link == nil {
		link = &transport{base: c.opts.Endpoint}
	}
	c.mu.Unlock()

	s, err := openSession(ctx, c.opts, link, c.publish)

	c.mu.Lock()
	defer c.mu.Unlock()
	if err == nil && c.closed {
		s.end()
		err = errDisconnected
	}
	if err != nil {
		c.setState(ConnectionStateDisconnected)
		return err
	}
	c.session = s
	c.setState(ConnectionStateConnected)
	// Report a session ended by the backend or the network.
	go func() {
		<-s.ctx.Done()
		c.mu.Lock()
		defer c.mu.Unlock()
		if c.session == s {
			c.session = nil
			c.setState(ConnectionStateDisconnected)
		}
	}()
	return nil
}

// dropSession must be called with c.mu held.
func (c *client) dropSession() {
	if c.session != nil {
		c.session.end()
		c.session = nil
	}
	c.setState(ConnectionStateDisconnected)
}

func (c *client) Disconnect(context.Context) *ClientError {
	c.lifecycle.Lock()
	defer c.lifecycle.Unlock()
	c.mu.Lock()
	defer c.mu.Unlock()
	c.dropSession()
	return nil
}

// call runs one request in the current session.
func (c *client) call(ctx context.Context, method, path string, body, out any) *ClientError {
	c.mu.Lock()
	s := c.session
	c.mu.Unlock()
	if s == nil || !s.alive() {
		return errDisconnected
	}
	return s.call(ctx, method, path, body, out, time.Duration(c.opts.RequestTimeoutMs)*time.Millisecond)
}

func (c *client) KeyFingerprint(ctx context.Context) (string, *ClientError) {
	var who whoamiDto
	err := c.call(ctx, "GET", pathWhoami, nil, &who)
	return who.Key, err
}

func (c *client) GetCustomer(ctx context.Context, id string) (Customer, *ClientError) {
	var cu Customer
	err := c.call(ctx, "GET", pathCustomers+"?id="+queryValue(id), nil, &cu)
	return cu, err
}

func (c *client) CreateCustomer(ctx context.Context, name string, email *string) (Customer, *ClientError) {
	var cu Customer
	err := c.call(ctx, "POST", pathCustomers, map[string]any{"name": name, "email": email}, &cu)
	return cu, err
}

func (c *client) SendCommand(ctx context.Context, cmd Command) (CommandResult, *ClientError) {
	var res commandDto // []byte travels as base64, like in the Rust core
	if err := c.call(ctx, "POST", pathCommands, commandDto{Name: cmd.Name, Payload: cmd.Payload}, &res); err != nil {
		return CommandResult{}, err
	}
	if res.Payload == nil {
		res.Payload = []byte{}
	}
	return CommandResult{Name: res.Name, Payload: res.Payload}, nil
}

// Events opens an independent subscription; cancelling the stream unsubscribes.
func (c *client) Events() wabiabi.Stream[ClientEvent] {
	ch, unsubscribe := c.subscribe()
	return wabiabi.Stream[ClientEvent]{C: ch, Cancel: unsubscribe}
}

func (c *client) subscribe() (<-chan ClientEvent, func()) {
	c.mu.Lock()
	defer c.mu.Unlock()
	ch := make(chan ClientEvent, 1024)
	if c.closed {
		close(ch)
		return ch, func() {}
	}
	id := c.nextSub
	c.nextSub++
	c.subs[id] = ch
	return ch, func() {
		c.mu.Lock()
		defer c.mu.Unlock()
		if _, ok := c.subs[id]; ok {
			delete(c.subs, id)
			close(ch)
		}
	}
}

// Close is the resource destructor: fails in-flight work, ends all streams.
func (c *client) Close() {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return
	}
	c.dropSession()
	c.closed = true
	for id, ch := range c.subs {
		delete(c.subs, id)
		close(ch)
	}
}

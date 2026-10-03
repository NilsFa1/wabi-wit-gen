package main

// The Go core's business logic: plain Go against the GENERATED contract
// types (wabi_generated.go, from wit/client.wit). Everything ABI-related —
// handles, futures, streams, codec, the exported C symbols — lives in the
// generated file and the runtime module wabi.example/wabiabi (runtime/go).
// It implements the same observable contract as the Rust core against an
// in-process backend (mem:// endpoints), which the cross-language tests use.

import (
	"context"
	"strconv"
	"sync"
	"time"

	wabiabi "wabi.example/wabiabi"
)

const coreInfo = "wabi-go-core/0.2.0 (goroutines, generated glue)"

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

type client struct {
	opts ClientOptions

	mu        sync.Mutex
	state     ConnectionState
	closed    bool
	conn      context.Context // live connection; cancelled on disconnect/hang-up/close
	closeConn context.CancelFunc
	subs      map[int]chan ClientEvent
	nextSub   int
	customers map[string]Customer
	nextID    int
}

// newClient is the constructor the generated dispatcher calls.
func newClient(o ClientOptions) Client {
	email := "erika@example.com"
	return &client{
		opts:      o,
		state:     ConnectionStateDisconnected,
		subs:      map[int]chan ClientEvent{},
		customers: map[string]Customer{"4711": {Id: "4711", Name: "Erika Mustermann", Email: &email}},
		nextID:    5000,
	}
}

// publish must be called with c.mu held. Slow subscribers drop events
// instead of blocking the core (same policy as the Rust core).
func (c *client) publish(e ClientEvent) {
	for _, ch := range c.subs {
		select {
		case ch <- e:
		default:
		}
	}
}

func (c *client) setState(s ConnectionState) {
	if c.state != s {
		c.state = s
		c.publish(ClientEvent{Tag: ClientEventConnectionStateChanged, ConnectionStateChanged: s})
	}
}

func (c *client) State() ConnectionState {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.state
}

func (c *client) Connect(ctx context.Context) *ClientError {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return errDisconnected
	}
	if c.conn != nil && c.conn.Err() == nil {
		return nil
	}
	c.setState(ConnectionStateConnecting)
	switch {
	case c.opts.Endpoint == "mem://unreachable":
		c.setState(ConnectionStateDisconnected)
		return networkError("connection refused")
	case len(c.opts.Endpoint) < 6 || c.opts.Endpoint[:6] != "mem://":
		c.setState(ConnectionStateDisconnected)
		return networkError("go core supports mem:// endpoints only: " + c.opts.Endpoint)
	case c.opts.ApiKey == nil || *c.opts.ApiKey != "demo-key":
		c.setState(ConnectionStateDisconnected)
		return errUnauthorized
	}
	c.conn, c.closeConn = context.WithCancel(context.Background())
	c.setState(ConnectionStateConnected)
	return nil
}

func (c *client) dropConnection() {
	if c.closeConn != nil {
		c.closeConn()
		c.conn, c.closeConn = nil, nil
	}
	c.setState(ConnectionStateDisconnected)
}

func (c *client) Disconnect(context.Context) *ClientError {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.dropConnection()
	return nil
}

// live returns the current connection context or `disconnected`.
func (c *client) live() (context.Context, *ClientError) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.conn == nil || c.conn.Err() != nil {
		return nil, errDisconnected
	}
	return c.conn, nil
}

func (c *client) GetCustomer(_ context.Context, id string) (Customer, *ClientError) {
	if _, err := c.live(); err != nil {
		return Customer{}, err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if cu, ok := c.customers[id]; ok {
		return cu, nil
	}
	return Customer{}, errNotFound
}

func (c *client) CreateCustomer(_ context.Context, name string, email *string) (Customer, *ClientError) {
	if _, err := c.live(); err != nil {
		return Customer{}, err
	}
	if name == "" {
		return Customer{}, protocolError("name must not be empty")
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.nextID++
	cu := Customer{Id: strconv.Itoa(c.nextID), Name: name, Email: email}
	c.customers[cu.Id] = cu
	c.publish(ClientEvent{Tag: ClientEventCustomerUpdated, CustomerUpdated: cu})
	return cu, nil
}

// SendCommand honours host cancellation (ctx) and connection loss.
func (c *client) SendCommand(ctx context.Context, cmd Command) (CommandResult, *ClientError) {
	out, err := c.run(ctx, cmd.Name, cmd.Payload)
	if err != nil {
		return CommandResult{}, err
	}
	return CommandResult{Name: cmd.Name, Payload: out}, nil
}

func (c *client) run(ctx context.Context, name string, payload []byte) ([]byte, *ClientError) {
	conn, err := c.live()
	if err != nil {
		return nil, err
	}
	switch name {
	case "echo":
		return payload, nil
	case "broadcast":
		c.mu.Lock()
		c.publish(ClientEvent{Tag: ClientEventMessageReceived, MessageReceived: string(payload)})
		c.mu.Unlock()
		return []byte{}, nil
	case "sleep":
		ms, _ := strconv.Atoi(string(payload))
		timeout := time.Duration(1<<62 - 1)
		if c.opts.RequestTimeoutMs > 0 {
			timeout = time.Duration(c.opts.RequestTimeoutMs) * time.Millisecond
		}
		select {
		case <-time.After(time.Duration(ms) * time.Millisecond):
			return payload, nil
		case <-time.After(timeout):
			return nil, networkError("request timed out")
		case <-ctx.Done():
			return nil, errDisconnected // result is discarded: the future was cancelled
		case <-conn.Done():
			return nil, errDisconnected
		}
	case "fail":
		return nil, protocolError("command failed on purpose")
	case "hang-up":
		c.mu.Lock()
		c.dropConnection()
		c.mu.Unlock()
		return nil, errDisconnected
	}
	return nil, protocolError("unknown command `" + name + "`")
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
	c.dropConnection()
	c.closed = true
	for id, ch := range c.subs {
		delete(c.subs, id)
		close(ch)
	}
}

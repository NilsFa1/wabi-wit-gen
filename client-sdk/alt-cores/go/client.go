package main

// The Go core's "business logic": plain Go, no cgo. It implements the same
// observable contract as the Rust core against an in-process backend
// (mem:// endpoints), which is what the cross-language test suites use.
// A production Go core would put its net/http + gorilla/websocket code here.

import (
	"context"
	"strconv"
	"sync"
	"time"
)

type connState uint32

const (
	stateDisconnected connState = 1
	stateConnecting   connState = 2
	stateConnected    connState = 3
)

// clientError mirrors WIT `client-error`; tag values come from abi-ids.lock.
type clientError struct {
	tag uint64
	msg string
}

var (
	errNotFound     = &clientError{tag: 1}
	errUnauthorized = &clientError{tag: 2}
	errDisconnected = &clientError{tag: 3}
)

func networkError(m string) *clientError  { return &clientError{tag: 4, msg: m} }
func protocolError(m string) *clientError { return &clientError{tag: 5, msg: m} }

type customer struct {
	id, name string
	email    *string
}

type event struct {
	tag      uint64 // 1 customer-updated, 2 connection-state-changed, 3 message-received
	customer customer
	state    connState
	text     string
}

type options struct {
	endpoint  string
	apiKey    *string
	timeoutMs uint64
}

type client struct {
	opts options

	mu        sync.Mutex
	state     connState
	closed    bool
	conn      context.Context // live connection; cancelled on disconnect/hang-up/close
	closeConn context.CancelFunc
	subs      map[int]chan event
	nextSub   int
	customers map[string]customer
	nextID    int
}

func newClient(o options) *client {
	email := "erika@example.com"
	return &client{
		opts:      o,
		state:     stateDisconnected,
		subs:      map[int]chan event{},
		customers: map[string]customer{"4711": {id: "4711", name: "Erika Mustermann", email: &email}},
		nextID:    5000,
	}
}

// publish must be called with c.mu held. Slow subscribers drop events
// instead of blocking the core (same policy as the Rust core).
func (c *client) publish(e event) {
	for _, ch := range c.subs {
		select {
		case ch <- e:
		default:
		}
	}
}

func (c *client) setState(s connState) {
	if c.state != s {
		c.state = s
		c.publish(event{tag: 2, state: s})
	}
}

func (c *client) getState() connState {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.state
}

func (c *client) connect(ctx context.Context) *clientError {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.closed {
		return errDisconnected
	}
	if c.conn != nil && c.conn.Err() == nil {
		return nil
	}
	c.setState(stateConnecting)
	switch {
	case c.opts.endpoint == "mem://unreachable":
		c.setState(stateDisconnected)
		return networkError("connection refused")
	case len(c.opts.endpoint) < 6 || c.opts.endpoint[:6] != "mem://":
		c.setState(stateDisconnected)
		return networkError("go core supports mem:// endpoints only: " + c.opts.endpoint)
	case c.opts.apiKey == nil || *c.opts.apiKey != "demo-key":
		c.setState(stateDisconnected)
		return errUnauthorized
	}
	c.conn, c.closeConn = context.WithCancel(context.Background())
	c.setState(stateConnected)
	return nil
}

func (c *client) dropConnection() {
	if c.closeConn != nil {
		c.closeConn()
		c.conn, c.closeConn = nil, nil
	}
	c.setState(stateDisconnected)
}

func (c *client) disconnect() *clientError {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.dropConnection()
	return nil
}

// live returns the current connection context or `disconnected`.
func (c *client) live() (context.Context, *clientError) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if c.conn == nil || c.conn.Err() != nil {
		return nil, errDisconnected
	}
	return c.conn, nil
}

func (c *client) getCustomer(id string) (customer, *clientError) {
	if _, err := c.live(); err != nil {
		return customer{}, err
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	if cu, ok := c.customers[id]; ok {
		return cu, nil
	}
	return customer{}, errNotFound
}

func (c *client) createCustomer(name string, email *string) (customer, *clientError) {
	if _, err := c.live(); err != nil {
		return customer{}, err
	}
	if name == "" {
		return customer{}, protocolError("name must not be empty")
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	c.nextID++
	cu := customer{id: strconv.Itoa(c.nextID), name: name, email: email}
	c.customers[cu.id] = cu
	c.publish(event{tag: 1, customer: cu})
	return cu, nil
}

// sendCommand honours host cancellation (ctx) and connection loss.
func (c *client) sendCommand(ctx context.Context, name string, payload []byte) ([]byte, *clientError) {
	conn, err := c.live()
	if err != nil {
		return nil, err
	}
	switch name {
	case "echo":
		return payload, nil
	case "broadcast":
		c.mu.Lock()
		c.publish(event{tag: 3, text: string(payload)})
		c.mu.Unlock()
		return []byte{}, nil
	case "sleep":
		ms, _ := strconv.Atoi(string(payload))
		timeout := time.Duration(1<<62 - 1)
		if c.opts.timeoutMs > 0 {
			timeout = time.Duration(c.opts.timeoutMs) * time.Millisecond
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

func (c *client) subscribe() (<-chan event, func()) {
	c.mu.Lock()
	defer c.mu.Unlock()
	ch := make(chan event, 1024)
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

// close is the resource destructor: fails in-flight work, ends all streams.
func (c *client) close() {
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

package wabiabi

// Host calls (ABI 1.1 and 1.2, ABI.md §4a): the core calls functions of WIT
// imports that the APP implements. The core invokes the registered handler
// with a call id; the app answers later, from any thread, with exactly one
// <prefix>_host_reply(call, status, response).
//
// ABI 1.2 (<prefix>_host_register2): the handler also gets the object on
// whose behalf the core calls, and the app is told when the core gives up on
// a call (ctx cancelled).

import (
	"context"
	"fmt"
	"sync"
	"unsafe"
)

// HostError is a host call that did not produce a return value: no handler
// registered, cancelled, the app answered with an error status, or the
// answer could not be decoded.
type HostError struct {
	Status  int32
	Message string
}

func (e *HostError) Error() string {
	return fmt.Sprintf("host call failed (%d): %s", e.Status, e.Message)
}

type hostReply struct {
	status  int32
	payload []byte
}

type pendingCall struct {
	req   unsafe.Pointer // C memory, valid until the reply (ABI contract)
	reply chan hostReply // buffered(1)
}

var host struct {
	// Readers hold `registered` while they run the handler, so that
	// registering (the writer) returns only once no invocation of the old
	// handler is running.
	registered          sync.RWMutex
	handler, cancel, ud unsafe.Pointer
	v2                  bool

	mu      sync.Mutex
	pending map[uint64]*pendingCall
}

// HostRegister installs (or with nil removes) the process-wide handler.
func HostRegister(handler, userData unsafe.Pointer) int32 {
	host.registered.Lock()
	defer host.registered.Unlock()
	host.handler, host.cancel, host.ud, host.v2 = handler, nil, userData, false
	return OK
}

// HostRegister2 (ABI 1.2): the handler learns the object, onCancel abandoned calls.
func HostRegister2(handler, onCancel, userData unsafe.Pointer) int32 {
	host.registered.Lock()
	defer host.registered.Unlock()
	host.handler, host.cancel, host.ud, host.v2 = handler, onCancel, userData, true
	return OK
}

type objectKey struct{}

// WithObject marks work as an operation of `object`: host calls made with the
// returned context name that object to the app. CallAsync does this for
// every method.
func WithObject(ctx context.Context, object uint64) context.Context {
	return context.WithValue(ctx, objectKey{}, object)
}

// HostReply completes a host call; response is copied.
func HostReply(call uint64, status int32, response unsafe.Pointer, n int) int32 {
	payload, ok := Borrow(response, n)
	if !ok {
		return ErrInvalidArgument
	}
	host.mu.Lock()
	p, found := host.pending[call]
	delete(host.pending, call)
	host.mu.Unlock()
	if !found {
		return ErrInvalidHandle
	}
	Free(p.req) // the app is done with the request now
	p.reply <- hostReply{status, payload}
	return OK
}

// notifyAbandoned tells the app that nobody waits for `call` any more, unless
// it has answered already.
func notifyAbandoned(call uint64) {
	host.mu.Lock()
	_, open := host.pending[call]
	host.mu.Unlock()
	if !open {
		return
	}
	host.registered.RLock()
	defer host.registered.RUnlock()
	if host.handler != nil && host.cancel != nil {
		invokeHostCancel(host.cancel, host.ud, call)
	}
}

// HostCall runs import function `method` in the app and waits for its answer.
// Call it only from async work (never from a sync dispatch, which runs inside
// an ABI call of the host). ctx cancellation abandons the call and tells the
// app; the request memory stays valid until the app replies anyway.
func HostCall(ctx context.Context, method uint32, req []byte) ([]byte, error) {
	object, _ := ctx.Value(objectKey{}).(uint64)
	host.registered.RLock()
	if host.handler == nil {
		host.registered.RUnlock()
		return nil, &HostError{ErrUnsupported, "no host handler registered (host_register)"}
	}
	call := 4<<56 | serial.Add(1)
	p := &pendingCall{reply: make(chan hostReply, 1)}
	var n int
	p.req, n = Alloc(req)
	host.mu.Lock()
	if host.pending == nil {
		host.pending = map[uint64]*pendingCall{}
	}
	host.pending[call] = p
	host.mu.Unlock()
	if host.v2 {
		invokeHost2(host.handler, host.ud, object, method, p.req, n, call)
	} else {
		invokeHost(host.handler, host.ud, method, p.req, n, call)
	}
	host.registered.RUnlock()

	select {
	case r := <-p.reply:
		if r.status != OK {
			return nil, &HostError{r.status, string(r.payload)}
		}
		return r.payload, nil
	case <-ctx.Done():
		notifyAbandoned(call)
		return nil, &HostError{ErrCancelled, "cancelled"}
	}
}

package wabiabi

// Host calls (ABI 1.1, ABI.md §4a): the core calls functions of WIT imports
// that the APP implements. The core invokes the registered handler with a
// call id; the app answers later, from any thread, with exactly one
// <prefix>_host_reply(call, status, response).

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
	mu          sync.Mutex
	handler, ud unsafe.Pointer
	pending     map[uint64]*pendingCall
}

// HostRegister installs (or with nil removes) the process-wide handler.
func HostRegister(handler, userData unsafe.Pointer) int32 {
	host.mu.Lock()
	defer host.mu.Unlock()
	host.handler, host.ud = handler, userData
	return OK
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

// HostCall runs import function `method` in the app and waits for its answer.
// Call it only from async work (never from a sync dispatch, which runs inside
// an ABI call of the host). ctx cancellation abandons the call; the request
// memory stays valid until the app replies anyway.
func HostCall(ctx context.Context, method uint32, req []byte) ([]byte, error) {
	host.mu.Lock()
	h, ud := host.handler, host.ud
	if h == nil {
		host.mu.Unlock()
		return nil, &HostError{ErrUnsupported, "no host handler registered (host_register)"}
	}
	if host.pending == nil {
		host.pending = map[uint64]*pendingCall{}
	}
	call := 4<<56 | serial.Add(1)
	p := &pendingCall{reply: make(chan hostReply, 1)}
	var n int
	p.req, n = Alloc(req)
	host.pending[call] = p
	host.mu.Unlock()

	invokeHost(h, ud, method, p.req, n, call)

	select {
	case r := <-p.reply:
		if r.status != OK {
			return nil, &HostError{r.status, string(r.payload)}
		}
		return r.payload, nil
	case <-ctx.Done():
		return nil, &HostError{ErrCancelled, "cancelled"}
	}
}

package wabiabi

import (
	"context"
	"sync"
	"unsafe"
)

// future is the generic state machine behind every future handle:
// the first transition wins, the callback fires exactly once.
type future struct {
	handle     uint64
	mu         sync.Mutex
	done       bool
	taken      bool
	status     int32
	payload    []byte
	subscribed bool
	cb, ud     unsafe.Pointer
	cancel     context.CancelFunc

	// A stream pull must restore a dequeued item and release busy before its
	// cancellation becomes observable. Its worker performs that finalization.
	deferredCancel  bool
	cancelRequested bool
}

func newFuture() (*future, context.Context) {
	ctx, cancel := context.WithCancel(context.Background())
	f := &future{cancel: cancel}
	f.handle = newHandle(KindFuture, f)
	return f, ctx
}

// complete runs on a core goroutine; reports whether the supplied result won.
func (f *future) complete(status int32, payload []byte) bool {
	return f.completeWith(status, payload, nil)
}

// beforePublish restores stream state before poll/take/subscribe can observe
// completion. It must not call this future or host code.
func (f *future) completeWith(status int32, payload []byte, beforePublish func(bool)) bool {
	f.mu.Lock()
	accepted := !f.done && !f.cancelRequested
	if !f.done {
		if f.cancelRequested {
			status, payload = ErrCancelled, nil
		}
		f.done, f.status, f.payload = true, status, payload
	}
	if beforePublish != nil {
		beforePublish(accepted)
	}
	cb, ud := f.cb, f.ud
	f.cb = nil
	f.mu.Unlock()
	f.cancel()
	if cb != nil {
		invokeCallback(cb, ud, f.handle)
	}
	return accepted
}

// cancelByHost never invokes the callback on the calling (host) thread.
func (f *future) cancelByHost() {
	f.mu.Lock()
	if f.done || f.cancelRequested {
		f.mu.Unlock()
		return
	}
	if f.deferredCancel {
		f.cancelRequested = true
		f.mu.Unlock()
		f.cancel()
		return
	}
	f.done, f.status = true, ErrCancelled
	cb, ud := f.cb, f.ud
	f.cb = nil
	f.mu.Unlock()
	f.cancel()
	if cb != nil {
		go invokeCallback(cb, ud, f.handle)
	}
}

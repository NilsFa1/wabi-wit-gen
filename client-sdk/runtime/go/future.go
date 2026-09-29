package acmeabi

import (
	"context"
	"sync"
	"unsafe"
)

// future is the generic state machine behind every future handle (ABI.md §6):
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
}

func newFuture() (*future, context.Context) {
	ctx, cancel := context.WithCancel(context.Background())
	f := &future{cancel: cancel}
	f.handle = newHandle(KindFuture, f)
	return f, ctx
}

// complete runs on a goroutine (a "core thread"); reports whether it won.
func (f *future) complete(status int32, payload []byte) bool {
	f.mu.Lock()
	if f.done {
		f.mu.Unlock()
		return false
	}
	f.done, f.status, f.payload = true, status, payload
	cb, ud := f.cb, f.ud
	f.cb = nil
	f.mu.Unlock()
	f.cancel()
	if cb != nil {
		invokeCallback(cb, ud, f.handle)
	}
	return true
}

// cancelByHost never invokes the callback on the calling (host) thread.
func (f *future) cancelByHost() {
	f.mu.Lock()
	if f.done {
		f.mu.Unlock()
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

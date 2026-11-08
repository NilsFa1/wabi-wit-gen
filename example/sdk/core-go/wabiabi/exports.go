package wabiabi

import (
	"context"
	"fmt"
	"unsafe"
)

// Dispatcher is implemented by the generated code: method id -> decode the
// request, call the core, encode the answer (ABI.md §4, §5).
type Dispatcher interface {
	// DispatchSync runs a sync method or constructor. On failure the []byte
	// is an optional UTF-8 diagnostic.
	DispatchSync(object uint64, method uint32, req []byte) ([]byte, int32)
	// DispatchAsync validates the call and returns the work to run on a
	// goroutine; ctx is cancelled when the host cancels or drops the future.
	DispatchAsync(object uint64, method uint32, req []byte) (func(ctx context.Context) []byte, int32)
}

// The functions below are the generic implementations of the ABI functions.
// The generated //export wrappers only convert C types and call these.

func CallSync(d Dispatcher, object uint64, method uint32, req unsafe.Pointer, reqLen int) (out unsafe.Pointer, outLen int, status int32) {
	defer func() {
		if r := recover(); r != nil {
			out, outLen = Alloc([]byte(fmt.Sprint("panic: ", r)))
			status = ErrInternal
		}
	}()
	b, ok := Borrow(req, reqLen)
	if !ok {
		return nil, 0, ErrInvalidArgument
	}
	payload, st := d.DispatchSync(object, method, b)
	out, outLen = Alloc(payload)
	return out, outLen, st
}

func CallAsync(d Dispatcher, object uint64, method uint32, req unsafe.Pointer, reqLen int) (uint64, int32) {
	b, ok := Borrow(req, reqLen)
	if !ok {
		return 0, ErrInvalidArgument
	}
	run, st := d.DispatchAsync(object, method, b)
	if st != OK {
		return 0, st
	}
	f, ctx := newFuture()
	ctx = WithObject(ctx, object) // host calls made by this operation belong to `object` (ABI 1.2)
	go func() {
		defer func() {
			if r := recover(); r != nil {
				f.complete(ErrInternal, []byte(fmt.Sprint("panic: ", r)))
			}
		}()
		f.complete(OK, run(ctx))
	}()
	return f.handle, OK
}

func ObjectDrop(h uint64) int32 { return dropObject(h) }

func FutureSubscribe(h uint64, cb, ud unsafe.Pointer) int32 {
	if cb == nil {
		return ErrInvalidArgument
	}
	f, ok := lookup[*future](h)
	if !ok {
		return ErrInvalidHandle
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.subscribed {
		return ErrBusy
	}
	f.subscribed = true
	if f.done {
		return Ready
	}
	f.cb, f.ud = cb, ud
	return OK
}

func FuturePoll(h uint64) int32 {
	f, ok := lookup[*future](h)
	if !ok {
		return ErrInvalidHandle
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.done {
		return Ready
	}
	return Pending
}

func FutureTake(h uint64) (unsafe.Pointer, int, int32) {
	f, ok := lookup[*future](h)
	if !ok {
		return nil, 0, ErrInvalidHandle
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	switch {
	case !f.done:
		return nil, 0, Pending
	case f.taken:
		return nil, 0, ErrAlreadyTaken
	}
	f.taken = true
	p, n := Alloc(f.payload)
	f.payload = nil
	return p, n, f.status
}

func FutureCancel(h uint64) int32 {
	f, ok := lookup[*future](h)
	if !ok {
		return ErrInvalidHandle
	}
	f.cancelByHost()
	return OK
}

func FutureDrop(h uint64) int32 {
	f, ok := remove[*future](h)
	if !ok {
		return ErrInvalidHandle
	}
	f.cancelByHost()
	return OK
}

func StreamNext(h uint64) (uint64, int32) {
	s, ok := lookup[*stream](h)
	if !ok {
		return 0, ErrInvalidHandle
	}
	return s.next()
}

func StreamCancel(h uint64) int32 {
	s, ok := lookup[*stream](h)
	if !ok {
		return ErrInvalidHandle
	}
	s.cancel()
	return OK
}

func StreamDrop(h uint64) int32 {
	s, ok := remove[*stream](h)
	if !ok {
		return ErrInvalidHandle
	}
	s.cancel()
	return OK
}

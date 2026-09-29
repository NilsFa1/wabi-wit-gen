package main

// Go ABI adapter: implements abi/acme_client.h with cgo. This file replaces
// core-rust/src/ffi — nothing on the SDK side changes.
//
//   go build -buildmode=c-shared -o libacme_client.so .

/*
#cgo CFLAGS: -I${SRCDIR}/../../abi
#define ACME_CLIENT_NO_PROTOTYPES
#include "acme_client.h"
#include <stdlib.h>

// Go cannot call C function pointers directly.
static inline void acme_invoke(acme_future_callback cb, void *ud, uint64_t future) { cb(ud, future); }
*/
import "C"

import (
	"context"
	"fmt"
	"sync"
	"sync/atomic"
	"unsafe"
)

const (
	statusOK              = 0
	statusPending         = 1
	statusReady           = 2
	statusStreamEnd       = 3
	errInvalidHandle      = -1
	errInvalidArgument    = -2
	errUnknownMethod      = -3
	errCancelled          = -4
	errBusy               = -5
	errAlreadyTaken       = -6
	errUnsupported        = -7
	errInternal           = -8
	abiVersion            = 1<<16 | 0
	capabilities          = 0b111111 // sync, async, streams, cancellation, threaded callbacks, msgpack v1
	kindObject     uint64 = 1
	kindFuture     uint64 = 2
	kindStream     uint64 = 3
)

// Method ids: wit/abi-ids.lock (a Go emitter of acme-wit-gen would generate these).
const (
	mCtor           = 0x0100
	mState          = 0x0101
	mConnect        = 0x0102
	mDisconnect     = 0x0103
	mGetCustomer    = 0x0104
	mCreateCustomer = 0x0105
	mSendCommand    = 0x0106
	mEvents         = 0x0107
)

var (
	contractID = C.CString("acme:client@1.0.0") // static for the process lifetime
	coreInfo   = C.CString("acme-go-core/0.1.0 (goroutines)")
	statusStrs = map[int32]*C.char{}
)

func init() {
	for code, name := range map[int32]string{
		statusOK: "ACME_OK", statusPending: "ACME_PENDING", statusReady: "ACME_READY", statusStreamEnd: "ACME_STREAM_END",
		errInvalidHandle: "ACME_ERR_INVALID_HANDLE", errInvalidArgument: "ACME_ERR_INVALID_ARGUMENT",
		errUnknownMethod: "ACME_ERR_UNKNOWN_METHOD", errCancelled: "ACME_ERR_CANCELLED", errBusy: "ACME_ERR_BUSY",
		errAlreadyTaken: "ACME_ERR_ALREADY_TAKEN", errUnsupported: "ACME_ERR_UNSUPPORTED", errInternal: "ACME_ERR_INTERNAL",
	} {
		statusStrs[code] = C.CString(name)
	}
	statusStrs[1000] = C.CString("ACME_UNKNOWN_STATUS")
}

// ------------------------------------------------------------ handle table

var (
	tableMu sync.Mutex
	table   = map[uint64]any{}
	serial  atomic.Uint64
)

func newHandle(kind uint64, v any) uint64 {
	h := kind<<56 | serial.Add(1) // never reused
	tableMu.Lock()
	table[h] = v
	tableMu.Unlock()
	return h
}

func lookup[T any](h uint64) (T, bool) {
	tableMu.Lock()
	defer tableMu.Unlock()
	v, ok := table[h].(T)
	return v, ok
}

func remove[T any](h uint64) (T, bool) {
	tableMu.Lock()
	defer tableMu.Unlock()
	v, ok := table[h].(T)
	if ok {
		delete(table, h)
	}
	return v, ok
}

// ------------------------------------------------------------------ buffers

func toBuffer(b []byte) C.acme_buffer {
	if len(b) == 0 {
		return C.acme_buffer{}
	}
	p := C.malloc(C.size_t(len(b)))
	copy(unsafe.Slice((*byte)(p), len(b)), b)
	return C.acme_buffer{ptr: (*C.uint8_t)(p), len: C.size_t(len(b))}
}

func fromBytes(b C.acme_bytes) ([]byte, bool) {
	if b.len == 0 {
		return nil, true
	}
	if b.ptr == nil {
		return nil, false
	}
	return C.GoBytes(unsafe.Pointer(b.ptr), C.int(b.len)), true
}

// ------------------------------------------------------------------ futures

type future struct {
	handle     uint64
	mu         sync.Mutex
	done       bool
	taken      bool
	status     int32
	payload    []byte
	subscribed bool
	cb         C.acme_future_callback
	ud         unsafe.Pointer
	cancel     context.CancelFunc
	onCancel   func(item []byte) // stream pulls: give back an item that lost the race
}

func newFuture() (*future, context.Context) {
	ctx, cancel := context.WithCancel(context.Background())
	f := &future{cancel: cancel}
	f.handle = newHandle(kindFuture, f)
	return f, ctx
}

// complete runs on a goroutine ("core thread"); first transition wins.
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
		C.acme_invoke(cb, ud, C.uint64_t(f.handle))
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
	f.done, f.status = true, errCancelled
	cb, ud := f.cb, f.ud
	f.cb = nil
	f.mu.Unlock()
	f.cancel()
	if cb != nil {
		go C.acme_invoke(cb, ud, C.uint64_t(f.handle))
	}
}

// ------------------------------------------------------------------ streams

type stream struct {
	mu       sync.Mutex
	items    <-chan []byte
	leftover [][]byte // items whose pull was cancelled after dequeue: delivered first
	busy     atomic.Bool
	stop     chan struct{}
	stopOnce sync.Once
	release  func()
}

func (s *stream) next() (uint64, int32) {
	if !s.busy.CompareAndSwap(false, true) {
		return 0, errBusy
	}
	f, ctx := newFuture()
	select {
	case <-s.stop:
		s.busy.Store(false)
		f.complete(statusStreamEnd, nil)
		return f.handle, statusOK
	default:
	}
	s.mu.Lock()
	if len(s.leftover) > 0 {
		item := s.leftover[0]
		s.leftover = s.leftover[1:]
		s.mu.Unlock()
		s.busy.Store(false)
		f.complete(statusOK, item)
		return f.handle, statusOK
	}
	s.mu.Unlock()
	go func() {
		var status int32
		var item []byte
		select {
		case it, ok := <-s.items:
			if ok {
				status, item = statusOK, it
			} else {
				status = statusStreamEnd
			}
		case <-s.stop:
			status = errCancelled
		case <-ctx.Done(): // this pull was cancelled; nothing consumed
			s.busy.Store(false)
			return
		}
		s.busy.Store(false)
		if !f.complete(status, item) && status == statusOK {
			s.mu.Lock() // cancel won the race after we dequeued: keep the item
			s.leftover = append(s.leftover, item)
			s.mu.Unlock()
		}
	}()
	return f.handle, statusOK
}

func (s *stream) cancel() {
	s.stopOnce.Do(func() {
		close(s.stop)
		s.release()
	})
}

// ------------------------------------------------------------------ exports

//export acme_abi_version
func acme_abi_version() C.uint32_t { return abiVersion }

//export acme_abi_capabilities
func acme_abi_capabilities() C.uint64_t { return capabilities }

//export acme_contract_id
func acme_contract_id() *C.char { return contractID }

//export acme_core_info
func acme_core_info() *C.char { return coreInfo }

//export acme_status_name
func acme_status_name(status C.acme_status) *C.char {
	if s, ok := statusStrs[int32(status)]; ok {
		return s
	}
	return statusStrs[1000]
}

//export acme_buffer_free
func acme_buffer_free(b C.acme_buffer) {
	if b.ptr != nil {
		C.free(unsafe.Pointer(b.ptr))
	}
}

//export acme_call_sync
func acme_call_sync(object C.uint64_t, method C.uint32_t, request C.acme_bytes, out *C.acme_buffer) (status C.acme_status) {
	if out == nil {
		return errInvalidArgument
	}
	*out = C.acme_buffer{}
	defer func() {
		if r := recover(); r != nil {
			*out = toBuffer([]byte(fmt.Sprint("panic: ", r)))
			status = errInternal
		}
	}()
	req, ok := fromBytes(request)
	if !ok {
		return errInvalidArgument
	}
	payload, st := dispatchSync(uint64(object), uint32(method), req)
	*out = toBuffer(payload)
	return C.acme_status(st)
}

//export acme_call_async
func acme_call_async(object C.uint64_t, method C.uint32_t, request C.acme_bytes, out *C.uint64_t) C.acme_status {
	if out == nil {
		return errInvalidArgument
	}
	*out = 0
	req, ok := fromBytes(request)
	if !ok {
		return errInvalidArgument
	}
	run, st := dispatchAsync(uint64(object), uint32(method), req)
	if st != statusOK {
		return C.acme_status(st)
	}
	f, ctx := newFuture()
	go func() {
		defer func() {
			if r := recover(); r != nil {
				f.complete(errInternal, []byte(fmt.Sprint("panic: ", r)))
			}
		}()
		f.complete(statusOK, run(ctx))
	}()
	*out = C.uint64_t(f.handle)
	return statusOK
}

//export acme_object_drop
func acme_object_drop(object C.uint64_t) C.acme_status {
	c, ok := remove[*client](uint64(object))
	if !ok {
		return errInvalidHandle
	}
	c.close()
	return statusOK
}

//export acme_future_subscribe
func acme_future_subscribe(h C.uint64_t, cb C.acme_future_callback, ud unsafe.Pointer) C.acme_status {
	if cb == nil {
		return errInvalidArgument
	}
	f, ok := lookup[*future](uint64(h))
	if !ok {
		return errInvalidHandle
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.subscribed {
		return errBusy
	}
	f.subscribed = true
	if f.done {
		return statusReady
	}
	f.cb, f.ud = cb, ud
	return statusOK
}

//export acme_future_poll
func acme_future_poll(h C.uint64_t) C.acme_status {
	f, ok := lookup[*future](uint64(h))
	if !ok {
		return errInvalidHandle
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	if f.done {
		return statusReady
	}
	return statusPending
}

//export acme_future_take
func acme_future_take(h C.uint64_t, out *C.acme_buffer) C.acme_status {
	if out == nil {
		return errInvalidArgument
	}
	*out = C.acme_buffer{}
	f, ok := lookup[*future](uint64(h))
	if !ok {
		return errInvalidHandle
	}
	f.mu.Lock()
	defer f.mu.Unlock()
	switch {
	case !f.done:
		return statusPending
	case f.taken:
		return errAlreadyTaken
	}
	f.taken = true
	*out = toBuffer(f.payload)
	f.payload = nil
	return C.acme_status(f.status)
}

//export acme_future_cancel
func acme_future_cancel(h C.uint64_t) C.acme_status {
	f, ok := lookup[*future](uint64(h))
	if !ok {
		return errInvalidHandle
	}
	f.cancelByHost()
	return statusOK
}

//export acme_future_drop
func acme_future_drop(h C.uint64_t) C.acme_status {
	f, ok := remove[*future](uint64(h))
	if !ok {
		return errInvalidHandle
	}
	f.cancelByHost()
	return statusOK
}

//export acme_stream_next
func acme_stream_next(h C.uint64_t, out *C.uint64_t) C.acme_status {
	if out == nil {
		return errInvalidArgument
	}
	*out = 0
	s, ok := lookup[*stream](uint64(h))
	if !ok {
		return errInvalidHandle
	}
	f, st := s.next()
	*out = C.uint64_t(f)
	return C.acme_status(st)
}

//export acme_stream_cancel
func acme_stream_cancel(h C.uint64_t) C.acme_status {
	s, ok := lookup[*stream](uint64(h))
	if !ok {
		return errInvalidHandle
	}
	s.cancel()
	return statusOK
}

//export acme_stream_drop
func acme_stream_drop(h C.uint64_t) C.acme_status {
	s, ok := remove[*stream](uint64(h))
	if !ok {
		return errInvalidHandle
	}
	s.cancel()
	return statusOK
}

func main() {} // required by -buildmode=c-shared

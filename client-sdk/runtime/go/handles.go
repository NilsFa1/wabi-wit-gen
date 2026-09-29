package acmeabi

import (
	"sync"
	"sync/atomic"
)

// Handle kinds (high byte of a handle); handles are never reused.
const (
	KindObject uint64 = 1
	KindFuture uint64 = 2
	KindStream uint64 = 3
)

var (
	tableMu sync.Mutex
	table   = map[uint64]any{}
	serial  atomic.Uint64
)

func newHandle(kind uint64, v any) uint64 {
	h := kind<<56 | serial.Add(1)
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

// Closer is implemented by every generated resource interface: Close runs
// when the host drops its handle (in-flight calls may still use the object).
type Closer interface{ Close() }

type object struct {
	group uint32
	value Closer
}

// NewObject registers a resource instance and returns its handle.
func NewObject(group uint32, v Closer) uint64 { return newHandle(KindObject, &object{group, v}) }

// Object returns the resource instance behind handle h if it has type T.
func Object[T any](h uint64) (T, bool) {
	var zero T
	o, ok := lookup[*object](h)
	if !ok {
		return zero, false
	}
	v, ok := o.value.(T)
	return v, ok
}

func dropObject(h uint64) int32 {
	o, ok := remove[*object](h)
	if !ok {
		return ErrInvalidHandle
	}
	o.value.Close()
	return OK
}

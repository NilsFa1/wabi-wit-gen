package wabiabi

/*
#include <stdlib.h>
*/
import "C"

import "unsafe"

// ABI status codes (wabi_abi.h).
const (
	OK                 int32 = 0
	Pending            int32 = 1
	Ready              int32 = 2
	StreamEnd          int32 = 3
	ErrInvalidHandle   int32 = -1
	ErrInvalidArgument int32 = -2
	ErrUnknownMethod   int32 = -3
	ErrCancelled       int32 = -4
	ErrBusy            int32 = -5
	ErrAlreadyTaken    int32 = -6
	ErrUnsupported     int32 = -7
	ErrInternal        int32 = -8
)

// Capability bits (wabi_abi.h).
const (
	CapSyncCalls        uint64 = 1 << 0
	CapAsyncCalls       uint64 = 1 << 1
	CapStreams          uint64 = 1 << 2
	CapCancellation     uint64 = 1 << 3
	CapThreadedCallback uint64 = 1 << 4
	CapPayloadMsgpack1  uint64 = 1 << 5
	CapHostCalls        uint64 = 1 << 6
	CapHostCalls2       uint64 = 1 << 7
)

// AbiVersion is the ABI version this runtime implements (1.2: host calls
// know their instance and can be cancelled).
const AbiVersion uint32 = 1<<16 | 2

// Capabilities of a core built on this runtime; hostCalls if its package has WIT imports.
func Capabilities(hostCalls bool) uint64 {
	c := CapSyncCalls | CapAsyncCalls | CapStreams | CapCancellation | CapThreadedCallback | CapPayloadMsgpack1
	if hostCalls {
		c |= CapHostCalls | CapHostCalls2
	}
	return c
}

var statusNames = map[int32]unsafe.Pointer{}

func init() {
	for code, name := range map[int32]string{
		OK: "WABI_OK", Pending: "WABI_PENDING", Ready: "WABI_READY", StreamEnd: "WABI_STREAM_END",
		ErrInvalidHandle: "WABI_ERR_INVALID_HANDLE", ErrInvalidArgument: "WABI_ERR_INVALID_ARGUMENT",
		ErrUnknownMethod: "WABI_ERR_UNKNOWN_METHOD", ErrCancelled: "WABI_ERR_CANCELLED", ErrBusy: "WABI_ERR_BUSY",
		ErrAlreadyTaken: "WABI_ERR_ALREADY_TAKEN", ErrUnsupported: "WABI_ERR_UNSUPPORTED", ErrInternal: "WABI_ERR_INTERNAL",
	} {
		statusNames[code] = CString(name)
	}
	statusNames[1000] = CString("WABI_UNKNOWN_STATUS")
}

// StatusName returns a static NUL-terminated C string.
func StatusName(status int32) unsafe.Pointer {
	if p, ok := statusNames[status]; ok {
		return p
	}
	return statusNames[1000]
}

// CString allocates a NUL-terminated C string that lives for the process lifetime.
func CString(s string) unsafe.Pointer { return unsafe.Pointer(C.CString(s)) }

// Alloc copies b into C memory owned by the host until <prefix>_buffer_free.
func Alloc(b []byte) (unsafe.Pointer, int) {
	if len(b) == 0 {
		return nil, 0
	}
	p := C.malloc(C.size_t(len(b)))
	copy(unsafe.Slice((*byte)(p), len(b)), b)
	return p, len(b)
}

// Free releases a buffer returned by Alloc.
func Free(p unsafe.Pointer) {
	if p != nil {
		C.free(p)
	}
}

// Borrow copies host-owned request bytes (valid only during the call).
func Borrow(p unsafe.Pointer, n int) ([]byte, bool) {
	if n == 0 {
		return nil, true
	}
	if p == nil {
		return nil, false
	}
	return C.GoBytes(p, C.int(n)), true
}

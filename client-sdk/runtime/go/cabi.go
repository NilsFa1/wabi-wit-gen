package wabiabi

// The only C code of the runtime: calling host function pointers, which Go
// cannot do directly. Function pointers travel through the Go API as
// unsafe.Pointer so the generated package (with its own cgo types) can pass them.

/*
#include <stdint.h>
#include <stdlib.h>

typedef void (*wabi_rt_future_cb)(void *, uint64_t);
typedef void (*wabi_rt_host_handler)(void *, uint32_t, const uint8_t *, size_t, uint64_t);

static inline void wabi_rt_invoke(void *cb, void *ud, uint64_t future) {
	((wabi_rt_future_cb)cb)(ud, future);
}
static inline void wabi_rt_invoke_host(void *h, void *ud, uint32_t method, const uint8_t *req, size_t len, uint64_t call) {
	((wabi_rt_host_handler)h)(ud, method, req, len, call);
}
*/
import "C"

import "unsafe"

func invokeCallback(cb, ud unsafe.Pointer, future uint64) {
	C.wabi_rt_invoke(cb, ud, C.uint64_t(future))
}

func invokeHost(h, ud unsafe.Pointer, method uint32, req unsafe.Pointer, n int, call uint64) {
	C.wabi_rt_invoke_host(h, ud, C.uint32_t(method), (*C.uint8_t)(req), C.size_t(n), C.uint64_t(call))
}

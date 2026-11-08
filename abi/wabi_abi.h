/*
 * wabi_abi.h — Wabi stable native ABI, version 1.0: types and constants
 *
 * This header is the *implementation protocol* between the language SDKs
 * (C#, Java, Swift, ...) and a native core (Rust, Go, Zig, ...). It is NOT the
 * public API: the public API is wit/client.wit. The ABI is deliberately tiny and
 * generic — it never grows when a WIT function is added. A WIT function is
 * addressed by a numeric method id (pinned in wit/abi-ids.lock) and its
 * arguments/results travel as MessagePack payloads (see ABI.md §5).
 *
 * Only fixed-width C types cross this boundary. No language-specific types
 * (Rust Vec/String/Result/Box/Arc, Go slices/strings/interfaces, Zig slices or
 * error unions) may ever appear here.
 *
 * Normative rules live in ABI.md; the short versions are repeated inline.
 *
 * This header holds everything that is identical for every WIT package
 * (types, status codes, capabilities). The functions are package-specific
 * only in their symbol prefix and live in wabi_abi_functions.h; each package
 * gets a small generated header (c/<prefix>.h) that binds them.
 */
#ifndef WABI_ABI_H
#define WABI_ABI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32)
#  if defined(WABI_BUILDING_CORE)
#    define WABI_API __declspec(dllexport)
#  else
#    define WABI_API __declspec(dllimport)
#  endif
#else
#  define WABI_API __attribute__((visibility("default")))
#endif

/* ------------------------------------------------------------------------ */
/* Versioning                                                                */
/* ------------------------------------------------------------------------ */

#define WABI_ABI_VERSION_MAJOR 1u
#define WABI_ABI_VERSION_MINOR 2u /* 1.1: optional host calls (WABI_CAP_HOST_CALLS)
                                     1.2: host calls know their instance and can be cancelled
                                          (WABI_CAP_HOST_CALLS_2); resource handles in payloads */
/* (major << 16) | minor. A host accepts a core iff major is equal and the
 * core's minor is >= the minor the host was generated against. */
#define WABI_ABI_VERSION ((WABI_ABI_VERSION_MAJOR << 16) | WABI_ABI_VERSION_MINOR)

/* Capability bits reported by <prefix>_abi_capabilities(). Unknown bits must be
 * ignored by hosts. */
#define WABI_CAP_SYNC_CALLS        (1ull << 0) /* <prefix>_call_sync                      */
#define WABI_CAP_ASYNC_CALLS       (1ull << 1) /* <prefix>_call_async + wabi_future_*     */
#define WABI_CAP_STREAMS           (1ull << 2) /* wabi_stream_*                       */
#define WABI_CAP_CANCELLATION      (1ull << 3) /* cancel interrupts in-flight work (informative) */
#define WABI_CAP_THREADED_CALLBACK (1ull << 4) /* futures may complete later, callbacks come
                                                  from core threads (informative; an eager core
                                                  completes every future before returning it) */
#define WABI_CAP_PAYLOAD_MSGPACK_1 (1ull << 5) /* payload encoding v1 (ABI.md §5)     */
#define WABI_CAP_HOST_CALLS        (1ull << 6) /* ABI 1.1: core calls host-implemented
                                                  WIT imports (<prefix>_host_register,
                                                  <prefix>_host_reply; ABI.md §4a) */
#define WABI_CAP_HOST_CALLS_2      (1ull << 7) /* ABI 1.2: <prefix>_host_register2 (the handler
                                                  learns the instance, the host learns about
                                                  abandoned calls; ABI.md §4a) */

/* ------------------------------------------------------------------------ */
/* Primitive types                                                           */
/* ------------------------------------------------------------------------ */

/* Opaque handles. 0 is never a valid handle. A core never reuses a handle
 * value within the lifetime of a process, so stale handles are always
 * detected (WABI_ERR_INVALID_HANDLE) instead of aliasing a new object. */
typedef uint64_t wabi_object_handle;
typedef uint64_t wabi_future_handle;
typedef uint64_t wabi_stream_handle;
#define WABI_NULL_HANDLE ((uint64_t)0)

/* Borrowed bytes, host -> core. Valid only for the duration of the call; the
 * core copies whatever it needs to keep. ptr may be NULL iff len == 0. */
typedef struct wabi_bytes {
    const uint8_t *ptr;
    size_t len;
} wabi_bytes;

/* Owned bytes, core -> host. Allocated by the core, owned by the host after the
 * call returns, released with <prefix>_buffer_free() of the SAME library, passing
 * back exactly the ptr/len received. {NULL, 0} is the empty buffer; freeing
 * it is a no-op. Hosts must never free it with their own allocator. */
typedef struct wabi_buffer {
    uint8_t *ptr;
    size_t len;
} wabi_buffer;

typedef int32_t wabi_status;

/* >= 0: success / informational, < 0: failure. Domain errors from WIT
 * declared by the WIT contract are NOT statuses: they are part of the
 * encoded result payload (result<T, E> -> [1, E]). Statuses only describe
 * ABI-level conditions. */
#define WABI_OK                    0
#define WABI_PENDING               1  /* future not completed yet                     */
#define WABI_READY                 2  /* future already completed (subscribe)         */
#define WABI_STREAM_END            3  /* stream exhausted (result of a next future)   */
#define WABI_ERR_INVALID_HANDLE   -1  /* 0, unknown, dropped, or wrong handle kind    */
#define WABI_ERR_INVALID_ARGUMENT -2  /* NULL out-pointer, malformed request payload  */
#define WABI_ERR_UNKNOWN_METHOD   -3  /* method id not implemented by this core       */
#define WABI_ERR_CANCELLED        -4  /* operation was cancelled before completion    */
#define WABI_ERR_BUSY             -5  /* already subscribed / next already pending    */
#define WABI_ERR_ALREADY_TAKEN    -6  /* result was already taken                     */
#define WABI_ERR_UNSUPPORTED      -7  /* e.g. sync call of an async method            */
#define WABI_ERR_INTERNAL         -8  /* bug/panic inside the core                    */

/* Completion callback. See ABI.md §4 for the full contract; in short:
 *   - invoked EXACTLY ONCE per <prefix>_future_subscribe() that returned WABI_OK,
 *     including when the future is cancelled or dropped,
 *   - never invoked from inside an ABI call made by the host (no reentrancy
 *     into host frames), always from a core-owned thread,
 *   - may run concurrently with other callbacks and with host calls,
 *   - must return quickly: no blocking, no waiting on other futures,
 *   - may call any ABI function (e.g. wabi_future_take).
 * It carries no result; the host calls <prefix>_future_take() afterwards. */
typedef void (*wabi_future_callback)(void *user_data, wabi_future_handle future);

/* Host-call handler (ABI 1.1, ABI.md §4a): the CORE calls the HOST to run a
 * function of a WIT *import* (e.g. "sign with the key in the Secure Enclave").
 *   - invoked on a core thread, never inside an ABI call made by the host,
 *   - must return quickly; the host answers LATER, from any thread, with
 *     exactly one <prefix>_host_reply(call, ...),
 *   - request/request_len stay valid until that reply (so the host may read
 *     them asynchronously); they encode the import's parameters (§5),
 *   - may be invoked concurrently for different calls.
 * Plain pointer + length (no struct by value) to suit every FFI. */
typedef void (*wabi_host_handler)(void *user_data, uint32_t method_id,
                                  const uint8_t *request, size_t request_len, uint64_t call);

/* Host-call handler of ABI 1.2: like wabi_host_handler, plus `object`, the
 * handle of the instance on whose behalf the core calls (the object whose
 * method is running), or 0 if the call belongs to no instance. A host uses it
 * to route the call to the implementation registered for that instance. */
typedef void (*wabi_host_handler2)(void *user_data, wabi_object_handle object, uint32_t method_id,
                                   const uint8_t *request, size_t request_len, uint64_t call);

/* Cancellation of a host call (ABI 1.2): the core no longer waits for `call`
 * (the surrounding operation was cancelled or its object dropped). The host
 * should stop working on it; it MUST still answer with <prefix>_host_reply()
 * exactly once (the answer is discarded and releases the request). Invoked at
 * most once per call, on a core thread, never inside an ABI call made by the
 * host; it may race with the host's own reply. Must return quickly. */
typedef void (*wabi_host_cancel)(void *user_data, uint64_t call);

/* The 17 functions are declared in wabi_abi_functions.h under a per-package
 * symbol prefix (see there and ABI.md §1.1). */

#ifdef __cplusplus
}
#endif

#endif /* WABI_ABI_H */

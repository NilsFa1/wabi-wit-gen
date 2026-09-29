/*
 * acme_abi.h — Acme stable native ABI, version 1.0: types and constants
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
 * only in their symbol prefix and live in acme_abi_functions.h; each package
 * gets a small generated header (generated/c/<prefix>.h) that binds them.
 */
#ifndef ACME_ABI_H
#define ACME_ABI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#if defined(_WIN32)
#  if defined(ACME_BUILDING_CORE)
#    define ACME_API __declspec(dllexport)
#  else
#    define ACME_API __declspec(dllimport)
#  endif
#else
#  define ACME_API __attribute__((visibility("default")))
#endif

/* ------------------------------------------------------------------------ */
/* Versioning                                                                */
/* ------------------------------------------------------------------------ */

#define ACME_ABI_VERSION_MAJOR 1u
#define ACME_ABI_VERSION_MINOR 0u
/* (major << 16) | minor. A host accepts a core iff major is equal and the
 * core's minor is >= the minor the host was generated against. */
#define ACME_ABI_VERSION ((ACME_ABI_VERSION_MAJOR << 16) | ACME_ABI_VERSION_MINOR)

/* Capability bits reported by <prefix>_abi_capabilities(). Unknown bits must be
 * ignored by hosts. */
#define ACME_CAP_SYNC_CALLS        (1ull << 0) /* <prefix>_call_sync                      */
#define ACME_CAP_ASYNC_CALLS       (1ull << 1) /* <prefix>_call_async + acme_future_*     */
#define ACME_CAP_STREAMS           (1ull << 2) /* acme_stream_*                       */
#define ACME_CAP_CANCELLATION      (1ull << 3) /* cancel interrupts in-flight work (informative) */
#define ACME_CAP_THREADED_CALLBACK (1ull << 4) /* futures may complete later, callbacks come
                                                  from core threads (informative; an eager core
                                                  completes every future before returning it) */
#define ACME_CAP_PAYLOAD_MSGPACK_1 (1ull << 5) /* payload encoding v1 (ABI.md §5)     */

/* ------------------------------------------------------------------------ */
/* Primitive types                                                           */
/* ------------------------------------------------------------------------ */

/* Opaque handles. 0 is never a valid handle. A core never reuses a handle
 * value within the lifetime of a process, so stale handles are always
 * detected (ACME_ERR_INVALID_HANDLE) instead of aliasing a new object. */
typedef uint64_t acme_object_handle;
typedef uint64_t acme_future_handle;
typedef uint64_t acme_stream_handle;
#define ACME_NULL_HANDLE ((uint64_t)0)

/* Borrowed bytes, host -> core. Valid only for the duration of the call; the
 * core copies whatever it needs to keep. ptr may be NULL iff len == 0. */
typedef struct acme_bytes {
    const uint8_t *ptr;
    size_t len;
} acme_bytes;

/* Owned bytes, core -> host. Allocated by the core, owned by the host after the
 * call returns, released with <prefix>_buffer_free() of the SAME library, passing
 * back exactly the ptr/len received. {NULL, 0} is the empty buffer; freeing
 * it is a no-op. Hosts must never free it with their own allocator. */
typedef struct acme_buffer {
    uint8_t *ptr;
    size_t len;
} acme_buffer;

typedef int32_t acme_status;

/* >= 0: success / informational, < 0: failure. Domain errors from WIT
 * (e.g. client-error.not-found) are NOT statuses: they are part of the
 * encoded result payload (result<T, E> -> [1, E]). Statuses only describe
 * ABI-level conditions. */
#define ACME_OK                    0
#define ACME_PENDING               1  /* future not completed yet                     */
#define ACME_READY                 2  /* future already completed (subscribe)         */
#define ACME_STREAM_END            3  /* stream exhausted (result of a next future)   */
#define ACME_ERR_INVALID_HANDLE   -1  /* 0, unknown, dropped, or wrong handle kind    */
#define ACME_ERR_INVALID_ARGUMENT -2  /* NULL out-pointer, malformed request payload  */
#define ACME_ERR_UNKNOWN_METHOD   -3  /* method id not implemented by this core       */
#define ACME_ERR_CANCELLED        -4  /* operation was cancelled before completion    */
#define ACME_ERR_BUSY             -5  /* already subscribed / next already pending    */
#define ACME_ERR_ALREADY_TAKEN    -6  /* result was already taken                     */
#define ACME_ERR_UNSUPPORTED      -7  /* e.g. sync call of an async method            */
#define ACME_ERR_INTERNAL         -8  /* bug/panic inside the core                    */

/* Completion callback. See ABI.md §4 for the full contract; in short:
 *   - invoked EXACTLY ONCE per <prefix>_future_subscribe() that returned ACME_OK,
 *     including when the future is cancelled or dropped,
 *   - never invoked from inside an ABI call made by the host (no reentrancy
 *     into host frames), always from a core-owned thread,
 *   - may run concurrently with other callbacks and with host calls,
 *   - must return quickly: no blocking, no waiting on other futures,
 *   - may call any ABI function (e.g. acme_future_take).
 * It carries no result; the host calls <prefix>_future_take() afterwards. */
typedef void (*acme_future_callback)(void *user_data, acme_future_handle future);

/* The 17 functions are declared in acme_abi_functions.h under a per-package
 * symbol prefix (see there and ABI.md §1.1). */

#ifdef __cplusplus
}
#endif

#endif /* ACME_ABI_H */

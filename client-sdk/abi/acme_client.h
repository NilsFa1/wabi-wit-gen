/*
 * acme_client.h — Acme stable native ABI, version 1.0
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
 */
#ifndef ACME_CLIENT_H
#define ACME_CLIENT_H

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

/* Capability bits reported by acme_abi_capabilities(). Unknown bits must be
 * ignored by hosts. */
#define ACME_CAP_SYNC_CALLS        (1ull << 0) /* acme_call_sync                      */
#define ACME_CAP_ASYNC_CALLS       (1ull << 1) /* acme_call_async + acme_future_*     */
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
 * call returns, released with acme_buffer_free() of the SAME library, passing
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
 *   - invoked EXACTLY ONCE per acme_future_subscribe() that returned ACME_OK,
 *     including when the future is cancelled or dropped,
 *   - never invoked from inside an ABI call made by the host (no reentrancy
 *     into host frames), always from a core-owned thread,
 *   - may run concurrently with other callbacks and with host calls,
 *   - must return quickly: no blocking, no waiting on other futures,
 *   - may call any ABI function (e.g. acme_future_take).
 * It carries no result; the host calls acme_future_take() afterwards. */
typedef void (*acme_future_callback)(void *user_data, acme_future_handle future);

#ifndef ACME_CLIENT_NO_PROTOTYPES /* alternative cores (cgo) may need only the types */

/* ------------------------------------------------------------------------ */
/* Library metadata — thread-safe, callable at any time, no allocation.      */
/* ------------------------------------------------------------------------ */

ACME_API uint32_t acme_abi_version(void);
ACME_API uint64_t acme_abi_capabilities(void);
/* Static NUL-terminated UTF-8 strings owned by the library (never freed). */
ACME_API const char *acme_contract_id(void);   /* e.g. "acme:client@1.0.0"      */
ACME_API const char *acme_core_info(void);     /* e.g. "rust-core/0.1.0 tokio"  */
ACME_API const char *acme_status_name(acme_status status);

/* ------------------------------------------------------------------------ */
/* Memory                                                                    */
/* ------------------------------------------------------------------------ */

ACME_API void acme_buffer_free(acme_buffer buffer);

/* ------------------------------------------------------------------------ */
/* Calls                                                                     */
/* ------------------------------------------------------------------------ */

/* Synchronous call of a non-async WIT function. `object` is the resource
 * handle for methods, ACME_NULL_HANDLE for constructors/static/free functions.
 * On return *out_response is always written (possibly {NULL,0}) and owned by
 * the host: on ACME_OK it is the encoded return value, on failure an optional
 * UTF-8 diagnostic. Never blocks on I/O. */
ACME_API acme_status acme_call_sync(acme_object_handle object, uint32_t method_id,
                                    acme_bytes request, acme_buffer *out_response);

/* Starts an async WIT function. On ACME_OK *out_future owns a new future. */
ACME_API acme_status acme_call_async(acme_object_handle object, uint32_t method_id,
                                     acme_bytes request, acme_future_handle *out_future);

/* Releases an object. In-flight futures of the object complete (typically
 * with a domain error or ACME_ERR_CANCELLED); its streams end. Their handles
 * stay valid until dropped individually. */
ACME_API acme_status acme_object_drop(acme_object_handle object);

/* ------------------------------------------------------------------------ */
/* Futures                                                                   */
/* ------------------------------------------------------------------------ */

/* Registers the completion callback (at most once per future).
 *   ACME_OK    -> callback will be invoked exactly once, later, on a core thread
 *   ACME_READY -> already complete; callback will NOT be invoked
 *   ACME_ERR_BUSY / ACME_ERR_INVALID_HANDLE -> callback will NOT be invoked */
ACME_API acme_status acme_future_subscribe(acme_future_handle future,
                                           acme_future_callback callback, void *user_data);

/* Non-blocking state query: ACME_PENDING or ACME_READY (or an error). */
ACME_API acme_status acme_future_poll(acme_future_handle future);

/* Moves the outcome out of a completed future (once). Returns
 *   ACME_OK            -> *out_result = encoded return value
 *   ACME_STREAM_END    -> (stream next futures only) no more items
 *   ACME_ERR_CANCELLED -> cancelled; no value
 *   ACME_PENDING       -> not complete yet; nothing taken
 *   ACME_ERR_ALREADY_TAKEN, ACME_ERR_* -> *out_result may hold a diagnostic
 * *out_result is always written and owned by the host. */
ACME_API acme_status acme_future_take(acme_future_handle future, acme_buffer *out_result);

/* Requests cancellation. Idempotent. If the future is still pending it
 * completes with ACME_ERR_CANCELLED (and the callback fires); if it already
 * completed, this is a no-op and the original outcome is kept
 * ("completion wins"). Returns ACME_OK for any live handle. */
ACME_API acme_status acme_future_cancel(acme_future_handle future);

/* Releases the handle. A pending future is cancelled first. A registered
 * callback still fires exactly once (possibly after drop returned); by then
 * the handle is invalid. Second drop -> ACME_ERR_INVALID_HANDLE, never UB. */
ACME_API acme_status acme_future_drop(acme_future_handle future);

/* ------------------------------------------------------------------------ */
/* Streams                                                                   */
/* ------------------------------------------------------------------------ */

/* Streams are obtained as return values of WIT functions (the handle is
 * encoded in the response payload; the host owns it). */

/* Requests the next item. At most one next-future may be pending per stream
 * (ACME_ERR_BUSY otherwise). The future resolves to ACME_OK + item payload,
 * ACME_STREAM_END, or ACME_ERR_CANCELLED. Cancelling a next-future only
 * abandons that pull: no item is lost. */
ACME_API acme_status acme_stream_next(acme_stream_handle stream, acme_future_handle *out_future);

/* Stops the producer. A pending next-future completes with ACME_ERR_CANCELLED,
 * later ones with ACME_STREAM_END. Idempotent. */
ACME_API acme_status acme_stream_cancel(acme_stream_handle stream);

/* Cancels (if needed) and releases the stream handle. */
ACME_API acme_status acme_stream_drop(acme_stream_handle stream);

#endif /* ACME_CLIENT_NO_PROTOTYPES */

#ifdef __cplusplus
}
#endif

#endif /* ACME_CLIENT_H */

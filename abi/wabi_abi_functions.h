/*
 * wabi_abi_functions.h — the functions of the Wabi native ABI v1.2
 * (17 core functions + 3 optional host-call functions).
 *
 * Every WIT package exports these functions under its own SYMBOL PREFIX,
 * derived from the package name (ABI.md §1.1):
 *
 * The prefix is selected by the WIT contract. Generated SDK copies include
 * concrete examples for that package.
 *
 * so several cores/packages can live in one process — even statically linked
 * into one binary (iOS xcframework) — without symbol clashes.
 *
 * Usage (normally via the generated c/<prefix>.h):
 *
 *     #define WABI_PREFIX <symbol_prefix>
 *     #include "wabi_abi_functions.h"
 *     #undef WABI_PREFIX
 *
 * Deliberately WITHOUT include guard: include it once per package.
 */
#include "wabi_abi.h"

#ifndef WABI_PREFIX
#  error "define WABI_PREFIX (the package symbol prefix) before including wabi_abi_functions.h"
#endif

#ifndef WABI_FN
#  define WABI_FN_CAT2(p, n) p##_##n
#  define WABI_FN_CAT(p, n) WABI_FN_CAT2(p, n)
/* WABI_FN(call_sync) -> <prefix>_call_sync */
#  define WABI_FN(name) WABI_FN_CAT(WABI_PREFIX, name)
#endif

#ifdef __cplusplus
extern "C" {
#endif
/* ------------------------------------------------------------------------ */
/* Library metadata — thread-safe, callable at any time, no allocation.      */
/* ------------------------------------------------------------------------ */

WABI_API uint32_t WABI_FN(abi_version)(void);
WABI_API uint64_t WABI_FN(abi_capabilities)(void);
/* Static NUL-terminated UTF-8 strings owned by the library (never freed). */
WABI_API const char *WABI_FN(contract_id)(void);   /* WIT package and its version */
WABI_API const char *WABI_FN(core_info)(void);     /* implementation description */
WABI_API const char *WABI_FN(status_name)(wabi_status status);

/* ------------------------------------------------------------------------ */
/* Memory                                                                    */
/* ------------------------------------------------------------------------ */

WABI_API void WABI_FN(buffer_free)(wabi_buffer buffer);

/* ------------------------------------------------------------------------ */
/* Calls                                                                     */
/* ------------------------------------------------------------------------ */

/* Synchronous call of a non-async WIT function. `object` is the resource
 * handle for methods, WABI_NULL_HANDLE for constructors/static/free functions.
 * On return *out_response is always written (possibly {NULL,0}) and owned by
 * the host: on WABI_OK it is the encoded return value, on failure an optional
 * UTF-8 diagnostic. Never blocks on I/O. */
WABI_API wabi_status WABI_FN(call_sync)(wabi_object_handle object, uint32_t method_id,
                                    wabi_bytes request, wabi_buffer *out_response);

/* Starts an async WIT function. On WABI_OK *out_future owns a new future. */
WABI_API wabi_status WABI_FN(call_async)(wabi_object_handle object, uint32_t method_id,
                                     wabi_bytes request, wabi_future_handle *out_future);

/* Releases an object. In-flight futures of the object complete (typically
 * with a domain error or WABI_ERR_CANCELLED); its streams end. Their handles
 * stay valid until dropped individually. */
WABI_API wabi_status WABI_FN(object_drop)(wabi_object_handle object);

/* ------------------------------------------------------------------------ */
/* Futures                                                                   */
/* ------------------------------------------------------------------------ */

/* Registers the completion callback (at most once per future).
 *   WABI_OK    -> callback will be invoked exactly once, later, on a core thread
 *   WABI_READY -> already complete; callback will NOT be invoked
 *   WABI_ERR_BUSY / WABI_ERR_INVALID_HANDLE -> callback will NOT be invoked */
WABI_API wabi_status WABI_FN(future_subscribe)(wabi_future_handle future,
                                           wabi_future_callback callback, void *user_data);

/* Non-blocking state query: WABI_PENDING or WABI_READY (or an error). */
WABI_API wabi_status WABI_FN(future_poll)(wabi_future_handle future);

/* Moves the outcome out of a completed future (once). Returns
 *   WABI_OK            -> *out_result = encoded return value
 *   WABI_STREAM_END    -> (stream next futures only) no more items
 *   WABI_ERR_CANCELLED -> cancelled; no value
 *   WABI_PENDING       -> not complete yet; nothing taken
 *   WABI_ERR_ALREADY_TAKEN, WABI_ERR_* -> *out_result may hold a diagnostic
 * *out_result is always written and owned by the host. */
WABI_API wabi_status WABI_FN(future_take)(wabi_future_handle future, wabi_buffer *out_result);

/* Requests cancellation. Idempotent. If the future is still pending it
 * completes with WABI_ERR_CANCELLED (and the callback fires); if it already
 * completed, this is a no-op and the original outcome is kept
 * ("completion wins"). Returns WABI_OK for any live handle. */
WABI_API wabi_status WABI_FN(future_cancel)(wabi_future_handle future);

/* Releases the handle. A pending future is cancelled first. A registered
 * callback still fires exactly once (possibly after drop returned); by then
 * the handle is invalid. Second drop -> WABI_ERR_INVALID_HANDLE, never UB. */
WABI_API wabi_status WABI_FN(future_drop)(wabi_future_handle future);

/* ------------------------------------------------------------------------ */
/* Streams                                                                   */
/* ------------------------------------------------------------------------ */

/* Streams are obtained as return values of WIT functions (the handle is
 * encoded in the response payload; the host owns it). */

/* Requests the next item. At most one next-future may be pending per stream
 * (WABI_ERR_BUSY otherwise). The future resolves to WABI_OK + item payload,
 * WABI_STREAM_END, or WABI_ERR_CANCELLED. Cancelling a next-future only
 * abandons that pull: no item is lost. */
WABI_API wabi_status WABI_FN(stream_next)(wabi_stream_handle stream, wabi_future_handle *out_future);

/* Stops the producer. A pending next-future completes with WABI_ERR_CANCELLED,
 * later ones with WABI_STREAM_END. Idempotent. */
WABI_API wabi_status WABI_FN(stream_cancel)(wabi_stream_handle stream);

/* Cancels (if needed) and releases the stream handle. */
WABI_API wabi_status WABI_FN(stream_drop)(wabi_stream_handle stream);


/* ------------------------------------------------------------------------ */
/* Host calls — ABI 1.1, only if WABI_CAP_HOST_CALLS (ABI.md §4a)            */
/* ------------------------------------------------------------------------ */

/* Installs the handler for all WIT imports of this package (one per
 * process; NULL uninstalls). Until a handler is installed, core operations
 * that need the host fail with a domain error or WABI_ERR_UNSUPPORTED.
 * Superseded by host_register2 (ABI 1.2); both set the same single handler. */
WABI_API wabi_status WABI_FN(host_register)(wabi_host_handler handler, void *user_data);

/* ABI 1.2, only if WABI_CAP_HOST_CALLS_2. Like host_register, with a handler
 * that learns the instance a call belongs to and an optional `on_cancel` that
 * is told when the core gives up on a call. A NULL handler uninstalls.
 *
 * Shutdown: when this function (or host_register) returns, the previously
 * installed handler and on_cancel are no longer running and will never be
 * invoked again, so the host may release them and their user_data. Calls that
 * are still open stay open: their replies are accepted as usual. It MUST NOT
 * be called from inside a handler or on_cancel. */
WABI_API wabi_status WABI_FN(host_register2)(wabi_host_handler2 handler, wabi_host_cancel on_cancel, void *user_data);

/* Completes a host call exactly once, from any thread. WABI_OK: `response`
 * is the encoded return value of the import (§5); otherwise an ABI status
 * (e.g. WABI_ERR_UNKNOWN_METHOD, WABI_ERR_INTERNAL) and an optional UTF-8
 * diagnostic. The core copies `response`. Unknown/answered calls ->
 * WABI_ERR_INVALID_HANDLE. */
WABI_API wabi_status WABI_FN(host_reply)(uint64_t call, wabi_status status, wabi_bytes response);
#ifdef __cplusplus
}
#endif

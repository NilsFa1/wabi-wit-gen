/*
 * acme_abi_functions.h — the 17 functions of the Acme native ABI v1.0.
 *
 * Every WIT package exports these functions under its own SYMBOL PREFIX,
 * derived from the package name (ABI.md §1.1):
 *
 *     acme:client  ->  acme_client_call_sync, acme_client_future_take, ...
 *     geo:lib      ->  geo_lib_call_sync, ...
 *
 * so several cores/packages can live in one process — even statically linked
 * into one binary (iOS xcframework) — without symbol clashes.
 *
 * Usage (normally via the generated generated/c/<prefix>.h):
 *
 *     #define ACME_PREFIX acme_client
 *     #include "acme_abi_functions.h"
 *     #undef ACME_PREFIX
 *
 * Deliberately WITHOUT include guard: include it once per package.
 */
#include "acme_abi.h"

#ifndef ACME_PREFIX
#  error "define ACME_PREFIX (the package symbol prefix, e.g. acme_client) before including acme_abi_functions.h"
#endif

#ifndef ACME_FN
#  define ACME_FN_CAT2(p, n) p##_##n
#  define ACME_FN_CAT(p, n) ACME_FN_CAT2(p, n)
/* ACME_FN(call_sync) -> <prefix>_call_sync */
#  define ACME_FN(name) ACME_FN_CAT(ACME_PREFIX, name)
#endif

#ifdef __cplusplus
extern "C" {
#endif
/* ------------------------------------------------------------------------ */
/* Library metadata — thread-safe, callable at any time, no allocation.      */
/* ------------------------------------------------------------------------ */

ACME_API uint32_t ACME_FN(abi_version)(void);
ACME_API uint64_t ACME_FN(abi_capabilities)(void);
/* Static NUL-terminated UTF-8 strings owned by the library (never freed). */
ACME_API const char *ACME_FN(contract_id)(void);   /* e.g. "acme:client@1.0.0"      */
ACME_API const char *ACME_FN(core_info)(void);     /* e.g. "rust-core/0.1.0 tokio"  */
ACME_API const char *ACME_FN(status_name)(acme_status status);

/* ------------------------------------------------------------------------ */
/* Memory                                                                    */
/* ------------------------------------------------------------------------ */

ACME_API void ACME_FN(buffer_free)(acme_buffer buffer);

/* ------------------------------------------------------------------------ */
/* Calls                                                                     */
/* ------------------------------------------------------------------------ */

/* Synchronous call of a non-async WIT function. `object` is the resource
 * handle for methods, ACME_NULL_HANDLE for constructors/static/free functions.
 * On return *out_response is always written (possibly {NULL,0}) and owned by
 * the host: on ACME_OK it is the encoded return value, on failure an optional
 * UTF-8 diagnostic. Never blocks on I/O. */
ACME_API acme_status ACME_FN(call_sync)(acme_object_handle object, uint32_t method_id,
                                    acme_bytes request, acme_buffer *out_response);

/* Starts an async WIT function. On ACME_OK *out_future owns a new future. */
ACME_API acme_status ACME_FN(call_async)(acme_object_handle object, uint32_t method_id,
                                     acme_bytes request, acme_future_handle *out_future);

/* Releases an object. In-flight futures of the object complete (typically
 * with a domain error or ACME_ERR_CANCELLED); its streams end. Their handles
 * stay valid until dropped individually. */
ACME_API acme_status ACME_FN(object_drop)(acme_object_handle object);

/* ------------------------------------------------------------------------ */
/* Futures                                                                   */
/* ------------------------------------------------------------------------ */

/* Registers the completion callback (at most once per future).
 *   ACME_OK    -> callback will be invoked exactly once, later, on a core thread
 *   ACME_READY -> already complete; callback will NOT be invoked
 *   ACME_ERR_BUSY / ACME_ERR_INVALID_HANDLE -> callback will NOT be invoked */
ACME_API acme_status ACME_FN(future_subscribe)(acme_future_handle future,
                                           acme_future_callback callback, void *user_data);

/* Non-blocking state query: ACME_PENDING or ACME_READY (or an error). */
ACME_API acme_status ACME_FN(future_poll)(acme_future_handle future);

/* Moves the outcome out of a completed future (once). Returns
 *   ACME_OK            -> *out_result = encoded return value
 *   ACME_STREAM_END    -> (stream next futures only) no more items
 *   ACME_ERR_CANCELLED -> cancelled; no value
 *   ACME_PENDING       -> not complete yet; nothing taken
 *   ACME_ERR_ALREADY_TAKEN, ACME_ERR_* -> *out_result may hold a diagnostic
 * *out_result is always written and owned by the host. */
ACME_API acme_status ACME_FN(future_take)(acme_future_handle future, acme_buffer *out_result);

/* Requests cancellation. Idempotent. If the future is still pending it
 * completes with ACME_ERR_CANCELLED (and the callback fires); if it already
 * completed, this is a no-op and the original outcome is kept
 * ("completion wins"). Returns ACME_OK for any live handle. */
ACME_API acme_status ACME_FN(future_cancel)(acme_future_handle future);

/* Releases the handle. A pending future is cancelled first. A registered
 * callback still fires exactly once (possibly after drop returned); by then
 * the handle is invalid. Second drop -> ACME_ERR_INVALID_HANDLE, never UB. */
ACME_API acme_status ACME_FN(future_drop)(acme_future_handle future);

/* ------------------------------------------------------------------------ */
/* Streams                                                                   */
/* ------------------------------------------------------------------------ */

/* Streams are obtained as return values of WIT functions (the handle is
 * encoded in the response payload; the host owns it). */

/* Requests the next item. At most one next-future may be pending per stream
 * (ACME_ERR_BUSY otherwise). The future resolves to ACME_OK + item payload,
 * ACME_STREAM_END, or ACME_ERR_CANCELLED. Cancelling a next-future only
 * abandons that pull: no item is lost. */
ACME_API acme_status ACME_FN(stream_next)(acme_stream_handle stream, acme_future_handle *out_future);

/* Stops the producer. A pending next-future completes with ACME_ERR_CANCELLED,
 * later ones with ACME_STREAM_END. Idempotent. */
ACME_API acme_status ACME_FN(stream_cancel)(acme_stream_handle stream);

/* Cancels (if needed) and releases the stream handle. */
ACME_API acme_status ACME_FN(stream_drop)(acme_stream_handle stream);

#ifdef __cplusplus
}
#endif

/*
 * Generic C shim of a Java core built with GraalVM native-image
 * (runtime/java-core). Compiled once per WIT package with its symbol prefix
 * and linked into the same shared library as the native image:
 *
 *   cc -c -fPIC -DWABI_PREFIX=gematik_pki -Iabi wabi_graal_shim.c
 *
 * It exports the ABI functions under <prefix>_* (ABI.md §1.1), attaches the
 * calling thread to the core's isolate (created lazily, once per process) and
 * forwards to the wabi_jvm_* entry points of NativeExports.java. Struct
 * arguments (wabi_bytes/wabi_buffer by value) are unpacked here because
 * GraalVM entry points cannot take structs by value.
 */
#include "wabi_abi_functions.h" /* the prototypes this file must match */

#include <pthread.h>
#include <stddef.h>

/* GraalVM C API (graal_isolate.h), declared here so the shim builds before the image. */
int graal_create_isolate(void *params, void **isolate, void **thread);
int graal_attach_thread(void *isolate, void **thread);

/* Entry points of NativeExports.java (first parameter: the isolate thread). */
const char *wabi_jvm_contract_id(void *t);
const char *wabi_jvm_core_info(void *t);
void wabi_jvm_buffer_free(void *t, void *ptr);
int wabi_jvm_call_sync(void *t, uint64_t obj, uint32_t method, const uint8_t *req, size_t len, uint8_t **out_ptr, size_t *out_len);
int wabi_jvm_call_async(void *t, uint64_t obj, uint32_t method, const uint8_t *req, size_t len, uint64_t *out_future);
int wabi_jvm_object_drop(void *t, uint64_t obj);
int wabi_jvm_future_subscribe(void *t, uint64_t future, wabi_future_callback cb, void *user_data);
int wabi_jvm_future_poll(void *t, uint64_t future);
int wabi_jvm_future_take(void *t, uint64_t future, uint8_t **out_ptr, size_t *out_len);
int wabi_jvm_future_cancel(void *t, uint64_t future);
int wabi_jvm_future_drop(void *t, uint64_t future);
int wabi_jvm_stream_next(void *t, uint64_t stream, uint64_t *out_future);
int wabi_jvm_stream_cancel(void *t, uint64_t stream);

static void *g_isolate;
static pthread_once_t g_once = PTHREAD_ONCE_INIT;

static void create_isolate(void) {
    void *thread;
    if (graal_create_isolate(NULL, &g_isolate, &thread) != 0) g_isolate = NULL;
}

/* The calling thread's isolate thread (attaching it on first use), or NULL. */
static void *current(void) {
    pthread_once(&g_once, create_isolate);
    void *thread = NULL;
    if (!g_isolate || graal_attach_thread(g_isolate, &thread) != 0) return NULL;
    return thread;
}

#define ENTER(fail) void *t = current(); if (!t) return fail

WABI_API uint32_t WABI_FN(abi_version)(void) { return WABI_ABI_VERSION; }

/* sync + async + streams + cancellation + threaded callbacks + msgpack v1 */
WABI_API uint64_t WABI_FN(abi_capabilities)(void) {
    return WABI_CAP_SYNC_CALLS | WABI_CAP_ASYNC_CALLS | WABI_CAP_STREAMS | WABI_CAP_CANCELLATION |
           WABI_CAP_THREADED_CALLBACK | WABI_CAP_PAYLOAD_MSGPACK_1;
}

WABI_API const char *WABI_FN(contract_id)(void) { ENTER(""); return wabi_jvm_contract_id(t); }
WABI_API const char *WABI_FN(core_info)(void) { ENTER(""); return wabi_jvm_core_info(t); }

WABI_API const char *WABI_FN(status_name)(wabi_status s) {
    switch (s) {
    case WABI_OK: return "WABI_OK";
    case WABI_PENDING: return "WABI_PENDING";
    case WABI_READY: return "WABI_READY";
    case WABI_STREAM_END: return "WABI_STREAM_END";
    case WABI_ERR_INVALID_HANDLE: return "WABI_ERR_INVALID_HANDLE";
    case WABI_ERR_INVALID_ARGUMENT: return "WABI_ERR_INVALID_ARGUMENT";
    case WABI_ERR_UNKNOWN_METHOD: return "WABI_ERR_UNKNOWN_METHOD";
    case WABI_ERR_CANCELLED: return "WABI_ERR_CANCELLED";
    case WABI_ERR_BUSY: return "WABI_ERR_BUSY";
    case WABI_ERR_ALREADY_TAKEN: return "WABI_ERR_ALREADY_TAKEN";
    case WABI_ERR_UNSUPPORTED: return "WABI_ERR_UNSUPPORTED";
    case WABI_ERR_INTERNAL: return "WABI_ERR_INTERNAL";
    default: return "WABI_UNKNOWN_STATUS";
    }
}

WABI_API void WABI_FN(buffer_free)(wabi_buffer b) {
    if (!b.ptr) return;
    void *t = current();
    if (t) wabi_jvm_buffer_free(t, b.ptr);
}

WABI_API wabi_status WABI_FN(call_sync)(wabi_object_handle obj, uint32_t method, wabi_bytes req, wabi_buffer *out) {
    if (!out) return WABI_ERR_INVALID_ARGUMENT;
    out->ptr = NULL;
    out->len = 0;
    if (req.len > 0 && !req.ptr) return WABI_ERR_INVALID_ARGUMENT;
    ENTER(WABI_ERR_INTERNAL);
    return wabi_jvm_call_sync(t, obj, method, req.ptr, req.len, &out->ptr, &out->len);
}

WABI_API wabi_status WABI_FN(call_async)(wabi_object_handle obj, uint32_t method, wabi_bytes req, wabi_future_handle *out) {
    if (!out) return WABI_ERR_INVALID_ARGUMENT;
    *out = 0;
    if (req.len > 0 && !req.ptr) return WABI_ERR_INVALID_ARGUMENT;
    ENTER(WABI_ERR_INTERNAL);
    return wabi_jvm_call_async(t, obj, method, req.ptr, req.len, out);
}

WABI_API wabi_status WABI_FN(object_drop)(wabi_object_handle obj) { ENTER(WABI_ERR_INTERNAL); return wabi_jvm_object_drop(t, obj); }

WABI_API wabi_status WABI_FN(future_subscribe)(wabi_future_handle f, wabi_future_callback cb, void *ud) {
    if (!cb) return WABI_ERR_INVALID_ARGUMENT;
    ENTER(WABI_ERR_INTERNAL);
    return wabi_jvm_future_subscribe(t, f, cb, ud);
}

WABI_API wabi_status WABI_FN(future_poll)(wabi_future_handle f) { ENTER(WABI_ERR_INTERNAL); return wabi_jvm_future_poll(t, f); }

WABI_API wabi_status WABI_FN(future_take)(wabi_future_handle f, wabi_buffer *out) {
    if (!out) return WABI_ERR_INVALID_ARGUMENT;
    out->ptr = NULL;
    out->len = 0;
    ENTER(WABI_ERR_INTERNAL);
    return wabi_jvm_future_take(t, f, &out->ptr, &out->len);
}

WABI_API wabi_status WABI_FN(future_cancel)(wabi_future_handle f) { ENTER(WABI_ERR_INTERNAL); return wabi_jvm_future_cancel(t, f); }
WABI_API wabi_status WABI_FN(future_drop)(wabi_future_handle f) { ENTER(WABI_ERR_INTERNAL); return wabi_jvm_future_drop(t, f); }

WABI_API wabi_status WABI_FN(stream_next)(wabi_stream_handle s, wabi_future_handle *out) {
    if (!out) return WABI_ERR_INVALID_ARGUMENT;
    *out = 0;
    ENTER(WABI_ERR_INTERNAL);
    return wabi_jvm_stream_next(t, s, out);
}

WABI_API wabi_status WABI_FN(stream_cancel)(wabi_stream_handle s) { ENTER(WABI_ERR_INTERNAL); return wabi_jvm_stream_cancel(t, s); }
WABI_API wabi_status WABI_FN(stream_drop)(wabi_stream_handle s) { ENTER(WABI_ERR_INTERNAL); return wabi_jvm_stream_cancel(t, s); }

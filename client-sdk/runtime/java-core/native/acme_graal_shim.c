/*
 * Generic C shim of a Java core built with GraalVM native-image
 * (runtime/java-core). Compiled once per WIT package with its symbol prefix
 * and linked into the same shared library as the native image:
 *
 *   cc -c -fPIC -DACME_PREFIX=gematik_pki -Iabi acme_graal_shim.c
 *
 * It exports the ABI functions under <prefix>_* (ABI.md §1.1), attaches the
 * calling thread to the core's isolate (created lazily, once per process) and
 * forwards to the acme_jvm_* entry points of NativeExports.java. Struct
 * arguments (acme_bytes/acme_buffer by value) are unpacked here because
 * GraalVM entry points cannot take structs by value.
 */
#include "acme_abi_functions.h" /* the prototypes this file must match */

#include <pthread.h>
#include <stddef.h>

/* GraalVM C API (graal_isolate.h), declared here so the shim builds before the image. */
int graal_create_isolate(void *params, void **isolate, void **thread);
int graal_attach_thread(void *isolate, void **thread);

/* Entry points of NativeExports.java (first parameter: the isolate thread). */
const char *acme_jvm_contract_id(void *t);
const char *acme_jvm_core_info(void *t);
void acme_jvm_buffer_free(void *t, void *ptr);
int acme_jvm_call_sync(void *t, uint64_t obj, uint32_t method, const uint8_t *req, size_t len, uint8_t **out_ptr, size_t *out_len);
int acme_jvm_call_async(void *t, uint64_t obj, uint32_t method, const uint8_t *req, size_t len, uint64_t *out_future);
int acme_jvm_object_drop(void *t, uint64_t obj);
int acme_jvm_future_subscribe(void *t, uint64_t future, acme_future_callback cb, void *user_data);
int acme_jvm_future_poll(void *t, uint64_t future);
int acme_jvm_future_take(void *t, uint64_t future, uint8_t **out_ptr, size_t *out_len);
int acme_jvm_future_cancel(void *t, uint64_t future);
int acme_jvm_future_drop(void *t, uint64_t future);
int acme_jvm_stream_next(void *t, uint64_t stream, uint64_t *out_future);
int acme_jvm_stream_cancel(void *t, uint64_t stream);

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

ACME_API uint32_t ACME_FN(abi_version)(void) { return ACME_ABI_VERSION; }

/* sync + async + streams + cancellation + threaded callbacks + msgpack v1 */
ACME_API uint64_t ACME_FN(abi_capabilities)(void) {
    return ACME_CAP_SYNC_CALLS | ACME_CAP_ASYNC_CALLS | ACME_CAP_STREAMS | ACME_CAP_CANCELLATION |
           ACME_CAP_THREADED_CALLBACK | ACME_CAP_PAYLOAD_MSGPACK_1;
}

ACME_API const char *ACME_FN(contract_id)(void) { ENTER(""); return acme_jvm_contract_id(t); }
ACME_API const char *ACME_FN(core_info)(void) { ENTER(""); return acme_jvm_core_info(t); }

ACME_API const char *ACME_FN(status_name)(acme_status s) {
    switch (s) {
    case ACME_OK: return "ACME_OK";
    case ACME_PENDING: return "ACME_PENDING";
    case ACME_READY: return "ACME_READY";
    case ACME_STREAM_END: return "ACME_STREAM_END";
    case ACME_ERR_INVALID_HANDLE: return "ACME_ERR_INVALID_HANDLE";
    case ACME_ERR_INVALID_ARGUMENT: return "ACME_ERR_INVALID_ARGUMENT";
    case ACME_ERR_UNKNOWN_METHOD: return "ACME_ERR_UNKNOWN_METHOD";
    case ACME_ERR_CANCELLED: return "ACME_ERR_CANCELLED";
    case ACME_ERR_BUSY: return "ACME_ERR_BUSY";
    case ACME_ERR_ALREADY_TAKEN: return "ACME_ERR_ALREADY_TAKEN";
    case ACME_ERR_UNSUPPORTED: return "ACME_ERR_UNSUPPORTED";
    case ACME_ERR_INTERNAL: return "ACME_ERR_INTERNAL";
    default: return "ACME_UNKNOWN_STATUS";
    }
}

ACME_API void ACME_FN(buffer_free)(acme_buffer b) {
    if (!b.ptr) return;
    void *t = current();
    if (t) acme_jvm_buffer_free(t, b.ptr);
}

ACME_API acme_status ACME_FN(call_sync)(acme_object_handle obj, uint32_t method, acme_bytes req, acme_buffer *out) {
    if (!out) return ACME_ERR_INVALID_ARGUMENT;
    out->ptr = NULL;
    out->len = 0;
    if (req.len > 0 && !req.ptr) return ACME_ERR_INVALID_ARGUMENT;
    ENTER(ACME_ERR_INTERNAL);
    return acme_jvm_call_sync(t, obj, method, req.ptr, req.len, &out->ptr, &out->len);
}

ACME_API acme_status ACME_FN(call_async)(acme_object_handle obj, uint32_t method, acme_bytes req, acme_future_handle *out) {
    if (!out) return ACME_ERR_INVALID_ARGUMENT;
    *out = 0;
    if (req.len > 0 && !req.ptr) return ACME_ERR_INVALID_ARGUMENT;
    ENTER(ACME_ERR_INTERNAL);
    return acme_jvm_call_async(t, obj, method, req.ptr, req.len, out);
}

ACME_API acme_status ACME_FN(object_drop)(acme_object_handle obj) { ENTER(ACME_ERR_INTERNAL); return acme_jvm_object_drop(t, obj); }

ACME_API acme_status ACME_FN(future_subscribe)(acme_future_handle f, acme_future_callback cb, void *ud) {
    if (!cb) return ACME_ERR_INVALID_ARGUMENT;
    ENTER(ACME_ERR_INTERNAL);
    return acme_jvm_future_subscribe(t, f, cb, ud);
}

ACME_API acme_status ACME_FN(future_poll)(acme_future_handle f) { ENTER(ACME_ERR_INTERNAL); return acme_jvm_future_poll(t, f); }

ACME_API acme_status ACME_FN(future_take)(acme_future_handle f, acme_buffer *out) {
    if (!out) return ACME_ERR_INVALID_ARGUMENT;
    out->ptr = NULL;
    out->len = 0;
    ENTER(ACME_ERR_INTERNAL);
    return acme_jvm_future_take(t, f, &out->ptr, &out->len);
}

ACME_API acme_status ACME_FN(future_cancel)(acme_future_handle f) { ENTER(ACME_ERR_INTERNAL); return acme_jvm_future_cancel(t, f); }
ACME_API acme_status ACME_FN(future_drop)(acme_future_handle f) { ENTER(ACME_ERR_INTERNAL); return acme_jvm_future_drop(t, f); }

ACME_API acme_status ACME_FN(stream_next)(acme_stream_handle s, acme_future_handle *out) {
    if (!out) return ACME_ERR_INVALID_ARGUMENT;
    *out = 0;
    ENTER(ACME_ERR_INTERNAL);
    return acme_jvm_stream_next(t, s, out);
}

ACME_API acme_status ACME_FN(stream_cancel)(acme_stream_handle s) { ENTER(ACME_ERR_INTERNAL); return acme_jvm_stream_cancel(t, s); }
ACME_API acme_status ACME_FN(stream_drop)(acme_stream_handle s) { ENTER(ACME_ERR_INTERNAL); return acme_jvm_stream_cancel(t, s); }

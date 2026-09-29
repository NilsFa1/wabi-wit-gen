/*
 * libacme_jni — generic JNI shim for the Acme Java SDK (Android, JDK 17+).
 *
 * Hand-written, contract-agnostic: it knows the 17 ABI functions of
 * abi/acme_abi_functions.h and nothing about customers, clients or method ids.
 * The package's symbol prefix ("acme_client_") is passed in at run time, so
 * one shim serves every WIT package. New WIT functions never require changes.
 *
 *   Java (JniAbi)  --JNI-->  libacme_jni  --dlopen/function table-->  libacme_client (Rust/Go/Zig/...)
 *
 * Memory: request byte[]s are pinned/copied only for the duration of a call
 * (the core copies what it keeps, ABI.md §7); result buffers are copied into
 * new byte[]s and released with the core's own acme_buffer_free right here,
 * so native memory never escapes to Java.
 *
 * Threads: completion callbacks arrive on core threads. The shim attaches
 * such a thread to the JVM once (as daemon) and detaches it when the thread
 * exits (pthread key destructor; per call on Windows). Android aborts if a
 * thread exits while still attached, which this avoids.
 */
#include "acme_abi.h" /* types only; symbols are resolved by name at run time */

#include <jni.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#  include <windows.h>
#  define LIB_OPEN(p) ((void *)LoadLibraryA(p))
#  define LIB_SYM(l, n) ((void *)GetProcAddress((HMODULE)(l), n))
#  define LIB_CLOSE(l) FreeLibrary((HMODULE)(l))
#else
#  include <dlfcn.h>
#  include <pthread.h>
#  define LIB_OPEN(p) dlopen(p, RTLD_NOW | RTLD_LOCAL)
#  define LIB_SYM(l, n) dlsym(l, n)
#  define LIB_CLOSE(l) dlclose(l)
#endif

#ifdef __ANDROID__
typedef JNIEnv *attach_env_t; /* Android's jni.h uses JNIEnv** for AttachCurrentThread */
#else
typedef void *attach_env_t;
#endif

/* ------------------------------------------------------------ function table */

typedef struct {
    void *handle;
    uint32_t (*abi_version)(void);
    uint64_t (*abi_capabilities)(void);
    const char *(*contract_id)(void);
    const char *(*core_info)(void);
    const char *(*status_name)(acme_status);
    void (*buffer_free)(acme_buffer);
    acme_status (*call_sync)(acme_object_handle, uint32_t, acme_bytes, acme_buffer *);
    acme_status (*call_async)(acme_object_handle, uint32_t, acme_bytes, acme_future_handle *);
    acme_status (*object_drop)(acme_object_handle);
    acme_status (*future_subscribe)(acme_future_handle, acme_future_callback, void *);
    acme_status (*future_poll)(acme_future_handle);
    acme_status (*future_take)(acme_future_handle, acme_buffer *);
    acme_status (*future_cancel)(acme_future_handle);
    acme_status (*future_drop)(acme_future_handle);
    acme_status (*stream_next)(acme_stream_handle, acme_future_handle *);
    acme_status (*stream_cancel)(acme_stream_handle);
    acme_status (*stream_drop)(acme_stream_handle);
} acme_lib;

#define L(lib) ((acme_lib *)(intptr_t)(lib))

/* ------------------------------------------------------------ JVM plumbing */

static JavaVM *g_vm;
static jclass g_cls;          /* com.acme.client.runtime.JniAbi (global ref) */
static jmethodID g_on_ready;  /* static void onReady(long key) */

#ifndef _WIN32
static pthread_key_t g_attached;
static void detach_at_thread_exit(void *unused) {
    (void)unused;
    (*g_vm)->DetachCurrentThread(g_vm);
}
#endif

JNIEXPORT jint JNICALL JNI_OnLoad(JavaVM *vm, void *reserved) {
    (void)reserved;
    JNIEnv *env;
    g_vm = vm;
    if ((*vm)->GetEnv(vm, (void **)&env, JNI_VERSION_1_6) != JNI_OK) return JNI_ERR;
    /* Resolve here: FindClass from a core thread would use the system class loader. */
    jclass cls = (*env)->FindClass(env, "com/acme/client/runtime/JniAbi");
    if (cls == NULL) return JNI_ERR;
    g_cls = (jclass)(*env)->NewGlobalRef(env, cls);
    g_on_ready = (*env)->GetStaticMethodID(env, g_cls, "onReady", "(J)V");
    if (g_on_ready == NULL) return JNI_ERR;
#ifndef _WIN32
    pthread_key_create(&g_attached, detach_at_thread_exit);
#endif
    return JNI_VERSION_1_6;
}

/* The completion callback handed to the core. Runs on a core thread; returns quickly. */
static void on_ready(void *user_data, acme_future_handle future) {
    (void)future;
    JNIEnv *env = NULL;
    int attached_now = 0;
    if ((*g_vm)->GetEnv(g_vm, (void **)&env, JNI_VERSION_1_6) == JNI_EDETACHED) {
        if ((*g_vm)->AttachCurrentThreadAsDaemon(g_vm, (attach_env_t *)&env, NULL) != JNI_OK) return;
        attached_now = 1;
#ifndef _WIN32
        pthread_setspecific(g_attached, (void *)1);
#endif
    }
    (*env)->CallStaticVoidMethod(env, g_cls, g_on_ready, (jlong)(intptr_t)user_data);
    if ((*env)->ExceptionCheck(env)) (*env)->ExceptionClear(env); /* never unwind into the core */
#ifdef _WIN32
    if (attached_now) (*g_vm)->DetachCurrentThread(g_vm);
#else
    (void)attached_now;
#endif
}

static void set_status(JNIEnv *env, jintArray out, acme_status st) {
    jint v = st;
    (*env)->SetIntArrayRegion(env, out, 0, 1, &v);
}

/* Copies a core-owned buffer into a new byte[] and frees it with the core's allocator. */
static jbyteArray consume(JNIEnv *env, acme_lib *lib, acme_buffer buf) {
    jbyteArray arr = (*env)->NewByteArray(env, (jsize)buf.len);
    if (arr != NULL && buf.len > 0) (*env)->SetByteArrayRegion(env, arr, 0, (jsize)buf.len, (const jbyte *)buf.ptr);
    lib->buffer_free(buf);
    return arr;
}

/* Borrow a Java byte[] for the duration of one call. */
typedef struct {
    jbyteArray array;
    jbyte *data;
    acme_bytes bytes;
} borrowed;

static borrowed borrow(JNIEnv *env, jbyteArray array) {
    borrowed b = {array, NULL, {NULL, 0}};
    jsize n = array ? (*env)->GetArrayLength(env, array) : 0;
    if (n > 0) {
        b.data = (*env)->GetByteArrayElements(env, array, NULL);
        b.bytes.ptr = (const uint8_t *)b.data;
        b.bytes.len = (size_t)n;
    }
    return b;
}

static void unborrow(JNIEnv *env, borrowed *b) {
    if (b->data) (*env)->ReleaseByteArrayElements(env, b->array, b->data, JNI_ABORT); /* read-only */
}

/* ------------------------------------------------------------ natives */

#define JNI_FN(ret, name) JNIEXPORT ret JNICALL Java_com_acme_client_runtime_JniAbi_##name

JNI_FN(jlong, nOpen)(JNIEnv *env, jclass c, jstring jpath, jstring jprefix, jboolean metadata_only, jobjectArray error) {
    (void)c;
    char msg[512];
    char sym[256];
    const char *path = (*env)->GetStringUTFChars(env, jpath, NULL);
    const char *prefix = (*env)->GetStringUTFChars(env, jprefix, NULL);
    acme_lib *lib = calloc(1, sizeof *lib);
    lib->handle = LIB_OPEN(path);
    if (!lib->handle) {
#ifdef _WIN32
        snprintf(msg, sizeof msg, "cannot load native core %s", path);
#else
        snprintf(msg, sizeof msg, "cannot load native core %s: %s", path, dlerror());
#endif
        goto fail;
    }
/* <prefix><name>, e.g. "acme_client_" + "call_sync" (ABI.md §1.1) */
#define SYM(field, name)                                                                   \
    snprintf(sym, sizeof sym, "%s%s", prefix, name);                                       \
    if (!(*(void **)&lib->field = LIB_SYM(lib->handle, sym))) {                             \
        snprintf(msg, sizeof msg, "%s is not a compatible core for this package: missing symbol %s", path, sym); \
        goto fail;                                                                         \
    }
    SYM(abi_version, "abi_version")
    SYM(abi_capabilities, "abi_capabilities")
    SYM(contract_id, "contract_id")
    SYM(core_info, "core_info")
    if (!metadata_only) {
        SYM(status_name, "status_name")
        SYM(buffer_free, "buffer_free")
        SYM(call_sync, "call_sync")
        SYM(call_async, "call_async")
        SYM(object_drop, "object_drop")
        SYM(future_subscribe, "future_subscribe")
        SYM(future_poll, "future_poll")
        SYM(future_take, "future_take")
        SYM(future_cancel, "future_cancel")
        SYM(future_drop, "future_drop")
        SYM(stream_next, "stream_next")
        SYM(stream_cancel, "stream_cancel")
        SYM(stream_drop, "stream_drop")
    }
#undef SYM
    (*env)->ReleaseStringUTFChars(env, jpath, path);
    (*env)->ReleaseStringUTFChars(env, jprefix, prefix);
    return (jlong)(intptr_t)lib;
fail:
    if (lib->handle) LIB_CLOSE(lib->handle);
    free(lib);
    (*env)->ReleaseStringUTFChars(env, jpath, path);
    (*env)->ReleaseStringUTFChars(env, jprefix, prefix);
    (*env)->SetObjectArrayElement(env, error, 0, (*env)->NewStringUTF(env, msg));
    return 0;
}

/* Only for probe(): a core in use must never be unloaded (ABI.md §6). */
JNI_FN(void, nClose)(JNIEnv *env, jclass c, jlong lib) {
    (void)env; (void)c;
    LIB_CLOSE(L(lib)->handle);
    free(L(lib));
}

JNI_FN(jint, nAbiVersion)(JNIEnv *env, jclass c, jlong lib) { (void)env; (void)c; return (jint)L(lib)->abi_version(); }
JNI_FN(jlong, nCapabilities)(JNIEnv *env, jclass c, jlong lib) { (void)env; (void)c; return (jlong)L(lib)->abi_capabilities(); }
JNI_FN(jstring, nContractId)(JNIEnv *env, jclass c, jlong lib) { (void)c; return (*env)->NewStringUTF(env, L(lib)->contract_id()); }
JNI_FN(jstring, nCoreInfo)(JNIEnv *env, jclass c, jlong lib) { (void)c; return (*env)->NewStringUTF(env, L(lib)->core_info()); }
JNI_FN(jstring, nStatusName)(JNIEnv *env, jclass c, jlong lib, jint st) { (void)c; return (*env)->NewStringUTF(env, L(lib)->status_name(st)); }

JNI_FN(jbyteArray, nCallSync)(JNIEnv *env, jclass c, jlong lib, jlong obj, jint method, jbyteArray req, jintArray status) {
    (void)c;
    borrowed b = borrow(env, req);
    acme_buffer out = {NULL, 0};
    acme_status st = L(lib)->call_sync((uint64_t)obj, (uint32_t)method, b.bytes, &out);
    unborrow(env, &b);
    set_status(env, status, st);
    return consume(env, L(lib), out);
}

JNI_FN(jlong, nCallAsync)(JNIEnv *env, jclass c, jlong lib, jlong obj, jint method, jbyteArray req, jintArray status) {
    (void)c;
    borrowed b = borrow(env, req);
    acme_future_handle fut = 0;
    acme_status st = L(lib)->call_async((uint64_t)obj, (uint32_t)method, b.bytes, &fut);
    unborrow(env, &b);
    set_status(env, status, st);
    return (jlong)fut;
}

JNI_FN(jint, nObjectDrop)(JNIEnv *env, jclass c, jlong lib, jlong obj) { (void)env; (void)c; return L(lib)->object_drop((uint64_t)obj); }

JNI_FN(jint, nSubscribe)(JNIEnv *env, jclass c, jlong lib, jlong fut, jlong key) {
    (void)env; (void)c;
    return L(lib)->future_subscribe((uint64_t)fut, on_ready, (void *)(intptr_t)key);
}

JNI_FN(jbyteArray, nTake)(JNIEnv *env, jclass c, jlong lib, jlong fut, jintArray status) {
    (void)c;
    acme_buffer out = {NULL, 0};
    acme_status st = L(lib)->future_take((uint64_t)fut, &out);
    set_status(env, status, st);
    return consume(env, L(lib), out);
}

JNI_FN(jint, nCancel)(JNIEnv *env, jclass c, jlong lib, jlong fut) { (void)env; (void)c; return L(lib)->future_cancel((uint64_t)fut); }
JNI_FN(jint, nFutureDrop)(JNIEnv *env, jclass c, jlong lib, jlong fut) { (void)env; (void)c; return L(lib)->future_drop((uint64_t)fut); }

JNI_FN(jlong, nStreamNext)(JNIEnv *env, jclass c, jlong lib, jlong stream, jintArray status) {
    (void)c;
    acme_future_handle fut = 0;
    acme_status st = L(lib)->stream_next((uint64_t)stream, &fut);
    set_status(env, status, st);
    return (jlong)fut;
}

JNI_FN(jint, nStreamCancel)(JNIEnv *env, jclass c, jlong lib, jlong s) { (void)env; (void)c; return L(lib)->stream_cancel((uint64_t)s); }
JNI_FN(jint, nStreamDrop)(JNIEnv *env, jclass c, jlong lib, jlong s) { (void)env; (void)c; return L(lib)->stream_drop((uint64_t)s); }

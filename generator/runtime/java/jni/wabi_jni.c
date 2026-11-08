/*
 * libwabi_jni — JNI shim of a Wabi Java SDK (Android, JDK 17+).
 *
 * Hand-written, contract-agnostic: it knows the 17 ABI functions of
 * abi/wabi_abi_functions.h and nothing about customers, clients or method ids.
 * The package's symbol prefix ("@LIBRARY@_") is passed in at run time; new
 * WIT functions never require changes. wabi-wit-gen copies it into every
 * package (java/jni) with the package's library name and JNI class,
 * so the shims of two SDKs can be loaded into one process.
 *
 *   Java (JniAbi)  --JNI-->  libwabi_jni  --dlopen/function table-->  lib@LIBRARY@ (Rust/Go/...)
 *
 * Memory: request byte[]s are pinned/copied only for the duration of a call
 * (the core copies what it keeps, ABI.md §7); result buffers are copied into
 * new byte[]s and released with the core's own wabi_buffer_free right here,
 * so native memory never escapes to Java.
 *
 * Threads: completion callbacks arrive on core threads. The shim attaches
 * such a thread to the JVM once (as daemon) and detaches it when the thread
 * exits (pthread key destructor; per call on Windows). Android aborts if a
 * thread exits while still attached, which this avoids.
 */
#include "wabi_abi.h" /* types only; symbols are resolved by name at run time */

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
    const char *(*status_name)(wabi_status);
    void (*buffer_free)(wabi_buffer);
    wabi_status (*call_sync)(wabi_object_handle, uint32_t, wabi_bytes, wabi_buffer *);
    wabi_status (*call_async)(wabi_object_handle, uint32_t, wabi_bytes, wabi_future_handle *);
    wabi_status (*object_drop)(wabi_object_handle);
    wabi_status (*future_subscribe)(wabi_future_handle, wabi_future_callback, void *);
    wabi_status (*future_poll)(wabi_future_handle);
    wabi_status (*future_take)(wabi_future_handle, wabi_buffer *);
    wabi_status (*future_cancel)(wabi_future_handle);
    wabi_status (*future_drop)(wabi_future_handle);
    wabi_status (*stream_next)(wabi_stream_handle, wabi_future_handle *);
    wabi_status (*stream_cancel)(wabi_stream_handle);
    wabi_status (*stream_drop)(wabi_stream_handle);
    /* ABI 1.2, optional (WABI_CAP_HOST_CALLS_2): NULL if the core has no host calls */
    wabi_status (*host_register2)(wabi_host_handler2, wabi_host_cancel, void *);
    wabi_status (*host_reply)(uint64_t, wabi_status, wabi_bytes);
} wabi_lib;

#define L(lib) ((wabi_lib *)(intptr_t)(lib))

/* ------------------------------------------------------------ JVM plumbing */

static JavaVM *g_vm;
static jclass g_cls;          /* com.wabi.client.runtime.JniAbi (global ref) */
static jmethodID g_on_ready;  /* static void onReady(long key) */
static jmethodID g_on_host_call;   /* static void onHostCall(long object, int methodId, byte[] request, long call) */
static jmethodID g_on_host_cancel; /* static void onHostCancel(long call) */

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
    jclass cls = (*env)->FindClass(env, "com/wabi/client/runtime/JniAbi");
    if (cls == NULL) return JNI_ERR;
    g_cls = (jclass)(*env)->NewGlobalRef(env, cls);
    g_on_ready = (*env)->GetStaticMethodID(env, g_cls, "onReady", "(J)V");
    if (g_on_ready == NULL) return JNI_ERR;
    g_on_host_call = (*env)->GetStaticMethodID(env, g_cls, "onHostCall", "(JI[BJ)V");
    if (g_on_host_call == NULL) return JNI_ERR;
    g_on_host_cancel = (*env)->GetStaticMethodID(env, g_cls, "onHostCancel", "(J)V");
    if (g_on_host_cancel == NULL) return JNI_ERR;
#ifndef _WIN32
    pthread_key_create(&g_attached, detach_at_thread_exit);
#endif
    return JNI_VERSION_1_6;
}

/* JNIEnv of the current (core) thread; attaches it once as daemon. */
static JNIEnv *attach(int *attached_now) {
    JNIEnv *env = NULL;
    *attached_now = 0;
    if ((*g_vm)->GetEnv(g_vm, (void **)&env, JNI_VERSION_1_6) == JNI_EDETACHED) {
        if ((*g_vm)->AttachCurrentThreadAsDaemon(g_vm, (attach_env_t *)&env, NULL) != JNI_OK) return NULL;
        *attached_now = 1;
#ifndef _WIN32
        pthread_setspecific(g_attached, (void *)1);
#endif
    }
    return env;
}

static void release(JNIEnv *env, int attached_now) {
    if ((*env)->ExceptionCheck(env)) (*env)->ExceptionClear(env); /* never unwind into the core */
#ifdef _WIN32
    if (attached_now) (*g_vm)->DetachCurrentThread(g_vm);
#else
    (void)attached_now;
#endif
}

/* The completion callback handed to the core. Runs on a core thread; returns quickly. */
static void on_ready(void *user_data, wabi_future_handle future) {
    (void)future;
    int attached_now;
    JNIEnv *env = attach(&attached_now);
    if (!env) return;
    (*env)->CallStaticVoidMethod(env, g_cls, g_on_ready, (jlong)(intptr_t)user_data);
    release(env, attached_now);
}

/* ABI 1.2 host-call handler: copies the request into a byte[] and hands it to
 * Java, which answers later through nHostReply (request stays valid until then). */
static void on_host_call(void *user_data, uint64_t object, uint32_t method_id, const uint8_t *request, size_t len, uint64_t call) {
    (void)user_data;
    int attached_now;
    JNIEnv *env = attach(&attached_now);
    if (!env) return;
    jbyteArray arr = (*env)->NewByteArray(env, (jsize)len);
    if (arr != NULL) {
        if (len > 0) (*env)->SetByteArrayRegion(env, arr, 0, (jsize)len, (const jbyte *)request);
        (*env)->CallStaticVoidMethod(env, g_cls, g_on_host_call, (jlong)object, (jint)method_id, arr, (jlong)call);
        (*env)->DeleteLocalRef(env, arr);
    }
    release(env, attached_now);
}

/* ABI 1.2: the core gave up on a host call. */
static void on_host_cancel(void *user_data, uint64_t call) {
    (void)user_data;
    int attached_now;
    JNIEnv *env = attach(&attached_now);
    if (!env) return;
    (*env)->CallStaticVoidMethod(env, g_cls, g_on_host_cancel, (jlong)call);
    release(env, attached_now);
}

static void set_status(JNIEnv *env, jintArray out, wabi_status st) {
    jint v = st;
    (*env)->SetIntArrayRegion(env, out, 0, 1, &v);
}

/* Copies a core-owned buffer into a new byte[] and frees it with the core's allocator. */
static jbyteArray consume(JNIEnv *env, wabi_lib *lib, wabi_buffer buf) {
    jbyteArray arr = (*env)->NewByteArray(env, (jsize)buf.len);
    if (arr != NULL && buf.len > 0) (*env)->SetByteArrayRegion(env, arr, 0, (jsize)buf.len, (const jbyte *)buf.ptr);
    lib->buffer_free(buf);
    return arr;
}

/* Borrow a Java byte[] for the duration of one call. */
typedef struct {
    jbyteArray array;
    jbyte *data;
    wabi_bytes bytes;
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

#define JNI_FN(ret, name) JNIEXPORT ret JNICALL Java_@JNI_CLASS@_##name

JNI_FN(jlong, nOpen)(JNIEnv *env, jclass c, jstring jpath, jstring jprefix, jboolean metadata_only, jobjectArray error) {
    (void)c;
    char msg[512];
    char sym[256];
    const char *path = (*env)->GetStringUTFChars(env, jpath, NULL);
    const char *prefix = (*env)->GetStringUTFChars(env, jprefix, NULL);
    wabi_lib *lib = calloc(1, sizeof *lib);
    lib->handle = LIB_OPEN(path);
    if (!lib->handle) {
#ifdef _WIN32
        snprintf(msg, sizeof msg, "cannot load native core %s", path);
#else
        snprintf(msg, sizeof msg, "cannot load native core %s: %s", path, dlerror());
#endif
        goto fail;
    }
/* <prefix><name>, e.g. "@LIBRARY@_" + "call_sync" (ABI.md §1.1) */
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
        /* optional (ABI 1.2): stay NULL for cores without host calls */
        snprintf(sym, sizeof sym, "%shost_register2", prefix);
        *(void **)&lib->host_register2 = LIB_SYM(lib->handle, sym);
        snprintf(sym, sizeof sym, "%shost_reply", prefix);
        *(void **)&lib->host_reply = LIB_SYM(lib->handle, sym);
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
    wabi_buffer out = {NULL, 0};
    wabi_status st = L(lib)->call_sync((uint64_t)obj, (uint32_t)method, b.bytes, &out);
    unborrow(env, &b);
    set_status(env, status, st);
    return consume(env, L(lib), out);
}

JNI_FN(jlong, nCallAsync)(JNIEnv *env, jclass c, jlong lib, jlong obj, jint method, jbyteArray req, jintArray status) {
    (void)c;
    borrowed b = borrow(env, req);
    wabi_future_handle fut = 0;
    wabi_status st = L(lib)->call_async((uint64_t)obj, (uint32_t)method, b.bytes, &fut);
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
    wabi_buffer out = {NULL, 0};
    wabi_status st = L(lib)->future_take((uint64_t)fut, &out);
    set_status(env, status, st);
    return consume(env, L(lib), out);
}

JNI_FN(jint, nCancel)(JNIEnv *env, jclass c, jlong lib, jlong fut) { (void)env; (void)c; return L(lib)->future_cancel((uint64_t)fut); }
JNI_FN(jint, nFutureDrop)(JNIEnv *env, jclass c, jlong lib, jlong fut) { (void)env; (void)c; return L(lib)->future_drop((uint64_t)fut); }

JNI_FN(jlong, nStreamNext)(JNIEnv *env, jclass c, jlong lib, jlong stream, jintArray status) {
    (void)c;
    wabi_future_handle fut = 0;
    wabi_status st = L(lib)->stream_next((uint64_t)stream, &fut);
    set_status(env, status, st);
    return (jlong)fut;
}

JNI_FN(jint, nStreamCancel)(JNIEnv *env, jclass c, jlong lib, jlong s) { (void)env; (void)c; return L(lib)->stream_cancel((uint64_t)s); }
JNI_FN(jint, nStreamDrop)(JNIEnv *env, jclass c, jlong lib, jlong s) { (void)env; (void)c; return L(lib)->stream_drop((uint64_t)s); }

JNI_FN(jint, nHostRegister)(JNIEnv *env, jclass c, jlong lib, jboolean install) {
    (void)env; (void)c;
    if (!L(lib)->host_register2 || !L(lib)->host_reply) return WABI_ERR_UNSUPPORTED;
    return L(lib)->host_register2(install ? on_host_call : NULL, install ? on_host_cancel : NULL, NULL);
}

JNI_FN(jint, nHostReply)(JNIEnv *env, jclass c, jlong lib, jlong call, jint status, jbyteArray response) {
    (void)c;
    if (!L(lib)->host_reply) return WABI_ERR_UNSUPPORTED;
    borrowed b = borrow(env, response); /* the core copies the response */
    wabi_status st = L(lib)->host_reply((uint64_t)call, (wabi_status)status, b.bytes);
    unborrow(env, &b);
    return st;
}

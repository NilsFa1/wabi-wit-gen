/*
 * ABI conformance suite — language-neutral, runs against ANY core library:
 *
 *   ./conformance path/to/libwabi_client.so [--profile full|basic] [--prefix wabi_client]
 *
 * It only uses abi/wabi_abi.h and dlopen(), i.e. exactly what an SDK sees.
 * Symbols are looked up as <prefix>_<name> (ABI.md §1.1).
 * "basic" skips tests needing a live backend simulation (used for the
 * minimal Zig stub core).
 */
#define _GNU_SOURCE
#include "../../abi/wabi_abi.h"

#include <dlfcn.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

/* ---- function table (resolved with dlsym, like every SDK does) ---------- */
static struct {
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
} A;

static int failures = 0, checks = 0;
#define CHECK(cond, ...)                                                         \
    do {                                                                         \
        checks++;                                                                \
        if (!(cond)) {                                                           \
            failures++;                                                          \
            fprintf(stderr, "  FAIL %s:%d: %s — ", __FILE__, __LINE__, #cond); \
            fprintf(stderr, __VA_ARGS__);                                        \
            fprintf(stderr, "\n");                                               \
        }                                                                        \
    } while (0)
/* Evaluates each operand exactly once (operands are often ABI calls). */
#define EQ(a, b)                                                     \
    do {                                                             \
        long long eq_a_ = (long long)(a), eq_b_ = (long long)(b);    \
        CHECK(eq_a_ == eq_b_, "%lld != %lld", eq_a_, eq_b_);         \
    } while (0)

/* Method ids come from wit/abi-ids.lock (an SDK gets them generated). */
enum {
    M_CTOR = 0x0100, M_STATE = 0x0101, M_CONNECT = 0x0102, M_DISCONNECT = 0x0103,
    M_GET_CUSTOMER = 0x0104, M_CREATE_CUSTOMER = 0x0105, M_SEND_COMMAND = 0x0106, M_EVENTS = 0x0107,
};

/* ---- minimal MessagePack writer ------------------------------------------ */
typedef struct { uint8_t b[4096]; size_t n; } mp;
static void mp_u8(mp *m, uint8_t v) { m->b[m->n++] = v; }
static void mp_uint(mp *m, uint32_t v) {
    if (v < 0x80) mp_u8(m, (uint8_t)v);
    else { mp_u8(m, 0xce); for (int i = 3; i >= 0; i--) mp_u8(m, (uint8_t)(v >> (8 * i))); }
}
static void mp_raw(mp *m, uint8_t marker8, const char *s, size_t len) {
    mp_u8(m, marker8); mp_u8(m, (uint8_t)len); memcpy(m->b + m->n, s, len); m->n += len;
}
static void mp_str(mp *m, const char *s) { mp_raw(m, 0xd9, s, strlen(s)); }
static void mp_bin(mp *m, const char *s) { mp_raw(m, 0xc4, s, strlen(s)); }
static wabi_bytes mp_bytes(mp *m) { wabi_bytes b = {m->b, m->n}; return b; }

static wabi_bytes req_ctor(mp *m, const char *endpoint, const char *key) {
    m->n = 0; mp_u8(m, 0x91); mp_u8(m, 0x83);
    mp_uint(m, 1); mp_str(m, endpoint);
    mp_uint(m, 2); mp_str(m, key);
    mp_uint(m, 3); mp_uint(m, 10000);
    return mp_bytes(m);
}
static wabi_bytes req_empty(mp *m) { m->n = 0; mp_u8(m, 0x90); return mp_bytes(m); }
static wabi_bytes req_str(mp *m, const char *s) { m->n = 0; mp_u8(m, 0x91); mp_str(m, s); return mp_bytes(m); }
static wabi_bytes req_command(mp *m, const char *name, const char *payload) {
    m->n = 0; mp_u8(m, 0x91); mp_u8(m, 0x82);
    mp_uint(m, 1); mp_str(m, name);
    mp_uint(m, 2); mp_bin(m, payload);
    return mp_bytes(m);
}

/* ---- waiting for callbacks ----------------------------------------------- */
typedef struct {
    pthread_mutex_t mu;
    pthread_cond_t cv;
    atomic_int fired;
    wabi_future_handle seen;
    pthread_t thread;
} waiter;

static void waiter_init(waiter *w) {
    pthread_mutex_init(&w->mu, NULL); pthread_cond_init(&w->cv, NULL);
    atomic_init(&w->fired, 0); w->seen = 0;
}
static void on_done(void *ud, wabi_future_handle f) {
    waiter *w = ud;
    pthread_mutex_lock(&w->mu);
    w->seen = f; w->thread = pthread_self();
    atomic_fetch_add(&w->fired, 1);
    pthread_cond_broadcast(&w->cv);
    pthread_mutex_unlock(&w->mu);
}
static int wait_fired(waiter *w, int ms) {
    struct timespec ts; clock_gettime(CLOCK_REALTIME, &ts);
    ts.tv_sec += ms / 1000; ts.tv_nsec += (long)(ms % 1000) * 1000000L;
    if (ts.tv_nsec >= 1000000000L) { ts.tv_sec++; ts.tv_nsec -= 1000000000L; }
    pthread_mutex_lock(&w->mu);
    while (atomic_load(&w->fired) == 0)
        if (pthread_cond_timedwait(&w->cv, &w->mu, &ts) != 0) break;
    pthread_mutex_unlock(&w->mu);
    return atomic_load(&w->fired);
}
static double now_ms(void) {
    struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1e3 + ts.tv_nsec / 1e6;
}

/* Awaits a future (subscribe + wait) and takes its result. */
static wabi_status await_take(wabi_future_handle f, wabi_buffer *out) {
    waiter w; waiter_init(&w);
    wabi_status s = A.future_subscribe(f, on_done, &w);
    if (s == WABI_OK) CHECK(wait_fired(&w, 10000) == 1, "callback did not fire");
    else CHECK(s == WABI_READY, "subscribe: %s", A.status_name(s));
    return A.future_take(f, out);
}

/* Starts an async call and awaits it. */
static wabi_status call(wabi_object_handle o, uint32_t m, wabi_bytes req, wabi_buffer *out) {
    wabi_future_handle f = 0;
    wabi_status s = A.call_async(o, m, req, &f);
    if (s != WABI_OK) return s;
    s = await_take(f, out);
    EQ(A.future_drop(f), WABI_OK);
    return s;
}

static int has(wabi_buffer b, const char *needle) {
    return b.len > 0 && memmem(b.ptr, b.len, needle, strlen(needle)) != NULL;
}
static int starts(wabi_buffer b, const uint8_t *p, size_t n) { return b.len >= n && memcmp(b.ptr, p, n) == 0; }
static const uint8_t RESULT_OK_UNIT[] = {0x92, 0x00, 0xc0};
static const uint8_t RESULT_OK[] = {0x92, 0x00};
static const uint8_t RESULT_NOT_FOUND[] = {0x92, 0x01, 0x91, 0x01};

static wabi_object_handle new_client(const char *key) {
    mp m; wabi_buffer out;
    wabi_status s = A.call_sync(0, M_CTOR, req_ctor(&m, "mem://conformance", key), &out);
    EQ(s, WABI_OK);
    uint64_t h = 0;
    if (out.len == 9 && out.ptr[0] == 0xcf) for (int i = 1; i < 9; i++) h = (h << 8) | out.ptr[i];
    else if (out.len >= 1 && out.ptr[0] < 0x80) h = out.ptr[0];
    CHECK(h != 0, "constructor did not return a handle (len %zu)", out.len);
    A.buffer_free(out);
    return h;
}

static wabi_object_handle connected_client(void) {
    mp m; wabi_buffer out;
    wabi_object_handle c = new_client("demo-key");
    EQ(call(c, M_CONNECT, req_empty(&m), &out), WABI_OK);
    CHECK(starts(out, RESULT_OK_UNIT, 3), "connect result");
    A.buffer_free(out);
    return c;
}

/* ================================================================ tests ==== */

static void t_metadata(void) {
    uint32_t v = A.abi_version();
    EQ(v >> 16, WABI_ABI_VERSION_MAJOR);
    uint64_t need = WABI_CAP_SYNC_CALLS | WABI_CAP_ASYNC_CALLS | WABI_CAP_STREAMS | WABI_CAP_PAYLOAD_MSGPACK_1;
    CHECK((A.abi_capabilities() & need) == need, "capabilities %llx", (unsigned long long)A.abi_capabilities());
    CHECK(strncmp(A.contract_id(), "wabi:client@1.", 14) == 0, "contract %s", A.contract_id());
    CHECK(strcmp(A.status_name(WABI_ERR_CANCELLED), "WABI_ERR_CANCELLED") == 0, "status name");
    printf("  core: %s, contract %s, abi %u.%u\n", A.core_info(), A.contract_id(), v >> 16, v & 0xffff);
}

static void t_invalid_handles(void) {
    mp m; wabi_buffer out; wabi_future_handle f;
    waiter w; waiter_init(&w);
    EQ(A.object_drop(0), WABI_ERR_INVALID_HANDLE);
    EQ(A.object_drop(0xdeadbeef), WABI_ERR_INVALID_HANDLE);
    EQ(A.future_drop(12345), WABI_ERR_INVALID_HANDLE);
    EQ(A.future_cancel(12345), WABI_ERR_INVALID_HANDLE);
    EQ(A.future_poll(12345), WABI_ERR_INVALID_HANDLE);
    EQ(A.future_subscribe(12345, on_done, &w), WABI_ERR_INVALID_HANDLE);
    EQ(A.future_take(12345, &out), WABI_ERR_INVALID_HANDLE); A.buffer_free(out);
    EQ(A.stream_next(12345, &f), WABI_ERR_INVALID_HANDLE);
    EQ(A.stream_drop(12345), WABI_ERR_INVALID_HANDLE);
    EQ(A.call_sync(777, M_STATE, req_empty(&m), &out), WABI_ERR_INVALID_HANDLE); A.buffer_free(out);
    EQ(A.call_async(777, M_CONNECT, req_empty(&m), &f), WABI_ERR_INVALID_HANDLE);
    /* NULL out-pointers are rejected, not dereferenced */
    EQ(A.call_sync(0, M_CTOR, req_empty(&m), NULL), WABI_ERR_INVALID_ARGUMENT);
    EQ(A.call_async(0, M_CONNECT, req_empty(&m), NULL), WABI_ERR_INVALID_ARGUMENT);
    CHECK(atomic_load(&w.fired) == 0, "callback fired for invalid subscribe");
}

static void t_sync_call_and_errors(void) {
    mp m; wabi_buffer out; wabi_future_handle f;
    wabi_object_handle c = new_client("demo-key");
    EQ(A.call_sync(c, M_STATE, req_empty(&m), &out), WABI_OK);
    CHECK(out.len == 1 && out.ptr[0] == 1, "state should be disconnected(1)");
    A.buffer_free(out);
    /* wrong kind of call / unknown method / malformed payload */
    EQ(A.call_sync(c, M_CONNECT, req_empty(&m), &out), WABI_ERR_UNSUPPORTED); A.buffer_free(out);
    EQ(A.call_async(c, M_STATE, req_empty(&m), &f), WABI_ERR_UNSUPPORTED);
    EQ(A.call_sync(c, 0x7fff, req_empty(&m), &out), WABI_ERR_UNKNOWN_METHOD); A.buffer_free(out);
    wabi_bytes garbage = {(const uint8_t *)"\xc1\xc1", 2};
    EQ(A.call_sync(0, M_CTOR, garbage, &out), WABI_ERR_INVALID_ARGUMENT);
    CHECK(out.len > 0, "diagnostic message expected"); A.buffer_free(out);
    wabi_bytes bad_ptr = {NULL, 5};
    EQ(A.call_sync(c, M_STATE, bad_ptr, &out), WABI_ERR_INVALID_ARGUMENT); A.buffer_free(out);
    EQ(A.object_drop(c), WABI_OK);
}

static void t_async_call_and_error(void) {
    mp m; wabi_buffer out;
    wabi_object_handle c = connected_client();
    EQ(call(c, M_GET_CUSTOMER, req_str(&m, "4711"), &out), WABI_OK);
    CHECK(starts(out, RESULT_OK, 2) && has(out, "Erika Mustermann"), "customer payload");
    A.buffer_free(out);
    EQ(call(c, M_GET_CUSTOMER, req_str(&m, "does-not-exist"), &out), WABI_OK);
    CHECK(starts(out, RESULT_NOT_FOUND, 4), "domain error must be an encoded result, not a status");
    A.buffer_free(out);
    /* unauthorized is also a domain error */
    wabi_object_handle bad = new_client("wrong-key");
    EQ(call(bad, M_CONNECT, req_empty(&m), &out), WABI_OK);
    const uint8_t unauthorized[] = {0x92, 0x01, 0x91, 0x02};
    CHECK(starts(out, unauthorized, 4), "unauthorized");
    A.buffer_free(out);
    EQ(A.object_drop(bad), WABI_OK);
    EQ(A.object_drop(c), WABI_OK);
}

static void t_future_protocol(void) {
    mp m; wabi_buffer out; wabi_future_handle f;
    wabi_object_handle c = connected_client();
    EQ(A.call_async(c, M_GET_CUSTOMER, req_str(&m, "4711"), &f), WABI_OK);
    waiter w; waiter_init(&w);
    wabi_status s = A.future_subscribe(f, on_done, &w);
    CHECK(s == WABI_OK || s == WABI_READY, "subscribe");
    EQ(A.future_subscribe(f, on_done, &w), WABI_ERR_BUSY); /* at most once */
    if (s == WABI_OK) {
        EQ(wait_fired(&w, 5000), 1);
        CHECK(w.seen == f, "callback got wrong handle");
        CHECK(!pthread_equal(w.thread, pthread_self()), "callback ran on host thread");
    }
    EQ(A.future_poll(f), WABI_READY);
    EQ(A.future_take(f, &out), WABI_OK); A.buffer_free(out);
    EQ(A.future_take(f, &out), WABI_ERR_ALREADY_TAKEN); A.buffer_free(out);
    EQ(A.future_cancel(f), WABI_OK); /* cancel after completion: no-op */
    EQ(A.future_drop(f), WABI_OK);
    EQ(A.future_drop(f), WABI_ERR_INVALID_HANDLE); /* double drop is detected */
    usleep(50000);
    EQ(atomic_load(&w.fired), s == WABI_OK ? 1 : 0); /* exactly once */
    EQ(A.object_drop(c), WABI_OK);
    EQ(A.object_drop(c), WABI_ERR_INVALID_HANDLE);
}

static void t_cancellation(void) {
    mp m; wabi_buffer out; wabi_future_handle f;
    wabi_object_handle c = connected_client();
    EQ(A.call_async(c, M_SEND_COMMAND, req_command(&m, "sleep", "5000"), &f), WABI_OK);
    waiter w; waiter_init(&w);
    EQ(A.future_subscribe(f, on_done, &w), WABI_OK);
    usleep(20000);
    EQ(A.future_poll(f), WABI_PENDING);
    double t0 = now_ms();
    EQ(A.future_cancel(f), WABI_OK);
    CHECK(atomic_load(&w.fired) == 0 || !pthread_equal(w.thread, pthread_self()), "callback inside cancel()");
    EQ(wait_fired(&w, 2000), 1);
    CHECK(now_ms() - t0 < 1000, "cancel took %.0f ms", now_ms() - t0);
    EQ(A.future_take(f, &out), WABI_ERR_CANCELLED); A.buffer_free(out);
    EQ(A.future_cancel(f), WABI_OK); /* idempotent */
    EQ(A.future_drop(f), WABI_OK);
    /* the client is still usable afterwards */
    EQ(call(c, M_GET_CUSTOMER, req_str(&m, "4711"), &out), WABI_OK); A.buffer_free(out);
    EQ(A.object_drop(c), WABI_OK);
}

/* Cancel racing with completion: whichever happens first wins; the callback
 * still fires exactly once and take() reports a consistent outcome. */
static void t_cancel_race(void) {
    mp m; wabi_buffer out;
    wabi_object_handle c = connected_client();
    int ok = 0, cancelled = 0;
    for (int i = 0; i < 200; i++) {
        wabi_future_handle f;
        EQ(A.call_async(c, M_GET_CUSTOMER, req_str(&m, "4711"), &f), WABI_OK);
        waiter w; waiter_init(&w);
        wabi_status s = A.future_subscribe(f, on_done, &w);
        usleep((useconds_t)((i % 10) * 40)); /* spread cancel over the completion window */
        A.future_cancel(f);
        if (s == WABI_OK) EQ(wait_fired(&w, 2000), 1);
        wabi_status t = A.future_take(f, &out);
        CHECK(t == WABI_OK || t == WABI_ERR_CANCELLED, "race outcome %d", t);
        if (t == WABI_OK) { ok++; CHECK(has(out, "Erika"), "value intact"); } else cancelled++;
        A.buffer_free(out);
        EQ(A.future_drop(f), WABI_OK);
        usleep(200);
        EQ(atomic_load(&w.fired), s == WABI_OK ? 1 : 0);
    }
    printf("  race: %d completed first, %d cancelled first\n", ok, cancelled);
    EQ(A.object_drop(c), WABI_OK);
}

/* Drop while pending ("callback during dispose"): the callback still fires
 * exactly once so the host can free user_data; the handle is already gone. */
static void t_drop_pending(void) {
    mp m; wabi_buffer out; wabi_future_handle f;
    wabi_object_handle c = connected_client();
    EQ(A.call_async(c, M_SEND_COMMAND, req_command(&m, "sleep", "3000"), &f), WABI_OK);
    waiter w; waiter_init(&w);
    EQ(A.future_subscribe(f, on_done, &w), WABI_OK);
    EQ(A.future_drop(f), WABI_OK);
    EQ(wait_fired(&w, 2000), 1);
    EQ(A.future_take(f, &out), WABI_ERR_INVALID_HANDLE); A.buffer_free(out);
    /* dropping the object with work in flight completes that work */
    EQ(A.call_async(c, M_SEND_COMMAND, req_command(&m, "sleep", "3000"), &f), WABI_OK);
    waiter w2; waiter_init(&w2);
    EQ(A.future_subscribe(f, on_done, &w2), WABI_OK);
    EQ(A.object_drop(c), WABI_OK);
    EQ(wait_fired(&w2, 2000), 1);
    wabi_status s = A.future_take(f, &out);
    const uint8_t disconnected[] = {0x92, 0x01, 0x91, 0x03};
    CHECK((s == WABI_OK && starts(out, disconnected, 4)) || s == WABI_ERR_CANCELLED, "in-flight after object drop: %d", s);
    A.buffer_free(out);
    EQ(A.future_drop(f), WABI_OK);
}

static void t_stream(void) {
    mp m; wabi_buffer out; wabi_future_handle f, f2;
    wabi_object_handle c = connected_client();
    EQ(A.call_sync(c, M_EVENTS, req_empty(&m), &out), WABI_OK);
    wabi_stream_handle s = 0;
    if (out.len == 9 && out.ptr[0] == 0xcf) for (int i = 1; i < 9; i++) s = (s << 8) | out.ptr[i];
    else if (out.len == 1) s = out.ptr[0];
    A.buffer_free(out);
    CHECK(s != 0, "events() returned no stream handle");

    /* one pull at a time; checked while the pull is still pending (nothing
     * buffered yet), otherwise the first pull may already have completed */
    EQ(A.stream_next(s, &f), WABI_OK);
    EQ(A.stream_next(s, &f2), WABI_ERR_BUSY);
    EQ(call(c, M_SEND_COMMAND, req_command(&m, "broadcast", "hello \xf0\x9f\x91\x8b"), &out), WABI_OK);
    A.buffer_free(out);
    EQ(await_take(f, &out), WABI_OK);
    const uint8_t msg_case[] = {0x92, 0x03};
    CHECK(starts(out, msg_case, 2) && has(out, "hello \xf0\x9f\x91\x8b"), "message-received event");
    A.buffer_free(out);
    EQ(A.future_drop(f), WABI_OK);

    /* cancelling a single pull does not lose items */
    EQ(A.stream_next(s, &f), WABI_OK);
    EQ(A.future_cancel(f), WABI_OK);
    EQ(await_take(f, &out), WABI_ERR_CANCELLED); A.buffer_free(out);
    EQ(A.future_drop(f), WABI_OK);
    usleep(20000);
    EQ(call(c, M_SEND_COMMAND, req_command(&m, "broadcast", "second"), &out), WABI_OK); A.buffer_free(out);
    EQ(A.stream_next(s, &f), WABI_OK);
    EQ(await_take(f, &out), WABI_OK);
    CHECK(has(out, "second"), "item after cancelled pull");
    A.buffer_free(out);
    EQ(A.future_drop(f), WABI_OK);

    /* stream cancel: pending pull -> cancelled, later pulls -> end */
    EQ(A.stream_next(s, &f), WABI_OK);
    EQ(A.stream_cancel(s), WABI_OK);
    EQ(await_take(f, &out), WABI_ERR_CANCELLED); A.buffer_free(out);
    EQ(A.future_drop(f), WABI_OK);
    usleep(20000);
    EQ(A.stream_next(s, &f), WABI_OK);
    EQ(await_take(f, &out), WABI_STREAM_END); A.buffer_free(out);
    EQ(A.future_drop(f), WABI_OK);
    EQ(A.stream_drop(s), WABI_OK);
    EQ(A.stream_drop(s), WABI_ERR_INVALID_HANDLE);

    /* dropping the object ends its streams */
    EQ(A.call_sync(c, M_EVENTS, req_empty(&m), &out), WABI_OK);
    s = 0;
    if (out.len == 9) for (int i = 1; i < 9; i++) s = (s << 8) | out.ptr[i];
    else if (out.len == 1) s = out.ptr[0];
    A.buffer_free(out);
    EQ(A.object_drop(c), WABI_OK);
    int end = 0;
    for (int i = 0; i < 10 && !end; i++) { /* may first see the final state-change event */
        EQ(A.stream_next(s, &f), WABI_OK);
        end = await_take(f, &out) == WABI_STREAM_END;
        A.buffer_free(out);
        A.future_drop(f);
    }
    CHECK(end, "stream did not end after object drop");
    EQ(A.stream_drop(s), WABI_OK);
}

static wabi_object_handle shared_client;
static atomic_int concurrent_ok;
static void *worker(void *arg) {
    (void)arg;
    mp m; wabi_buffer out;
    for (int i = 0; i < 50; i++) {
        if (call(shared_client, M_GET_CUSTOMER, req_str(&m, "4711"), &out) == WABI_OK && has(out, "Erika"))
            atomic_fetch_add(&concurrent_ok, 1);
        A.buffer_free(out);
    }
    return NULL;
}

static void t_concurrent(void) {
    shared_client = connected_client();
    pthread_t th[16];
    for (int i = 0; i < 16; i++) pthread_create(&th[i], NULL, worker, NULL);
    for (int i = 0; i < 16; i++) pthread_join(th[i], NULL);
    EQ(atomic_load(&concurrent_ok), 16 * 50);
    EQ(A.object_drop(shared_client), WABI_OK);
}

static void t_large_and_unicode(void) {
    wabi_buffer out;
    wabi_object_handle c = connected_client();
    /* 1 MiB command payload through ABI + core + backend and back */
    size_t n = 1 << 20;
    uint8_t *buf = malloc(n + 16);
    size_t k = 0;
    buf[k++] = 0x91; buf[k++] = 0x82;
    buf[k++] = 0x01; buf[k++] = 0xa4; memcpy(buf + k, "echo", 4); k += 4;
    buf[k++] = 0x02; buf[k++] = 0xc6;
    buf[k++] = (uint8_t)(n >> 24); buf[k++] = (uint8_t)(n >> 16); buf[k++] = (uint8_t)(n >> 8); buf[k++] = (uint8_t)n;
    for (size_t i = 0; i < n; i++) buf[k + i] = (uint8_t)(i * 31);
    wabi_bytes req = {buf, k + n};
    EQ(call(c, M_SEND_COMMAND, req, &out), WABI_OK);
    CHECK(out.len > n && memmem(out.ptr, out.len, buf + k, n) != NULL, "1 MiB echo mismatch (len %zu)", out.len);
    A.buffer_free(out);
    free(buf);

    /* Unicode: create a customer with non-ASCII name, read it back */
    mp m; m.n = 0;
    const char *name = "J\xc3\xbcrgen \xe5\xbc\xa0\xe4\xbc\x9f \xf0\x9f\x9a\x80";
    mp_u8(&m, 0x92); mp_str(&m, name); mp_u8(&m, 0xc0);
    EQ(call(c, M_CREATE_CUSTOMER, mp_bytes(&m), &out), WABI_OK);
    CHECK(starts(out, RESULT_OK, 2) && has(out, name), "unicode name roundtrip");
    A.buffer_free(out);
    EQ(A.object_drop(c), WABI_OK);
}

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <core library> [--profile full|basic] [--prefix wabi_client]\n", argv[0]);
        return 2;
    }
    int full = 1;
    const char *prefix = "wabi_client";
    for (int i = 2; i + 1 < argc; i += 2) {
        if (strcmp(argv[i], "--profile") == 0) full = strcmp(argv[i + 1], "basic") != 0;
        else if (strcmp(argv[i], "--prefix") == 0) prefix = argv[i + 1];
    }
    void *lib = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL);
    if (!lib) { fprintf(stderr, "dlopen: %s\n", dlerror()); return 2; }
    char sym[256];
#define SYM(field, name)                                                               \
    snprintf(sym, sizeof sym, "%s_%s", prefix, name);                                  \
    if (!(*(void **)&A.field = dlsym(lib, sym))) { fprintf(stderr, "missing symbol %s\n", sym); return 2; }
    SYM(abi_version, "abi_version") SYM(abi_capabilities, "abi_capabilities")
    SYM(contract_id, "contract_id") SYM(core_info, "core_info") SYM(status_name, "status_name")
    SYM(buffer_free, "buffer_free") SYM(call_sync, "call_sync") SYM(call_async, "call_async")
    SYM(object_drop, "object_drop") SYM(future_subscribe, "future_subscribe")
    SYM(future_poll, "future_poll") SYM(future_take, "future_take")
    SYM(future_cancel, "future_cancel") SYM(future_drop, "future_drop")
    SYM(stream_next, "stream_next") SYM(stream_cancel, "stream_cancel") SYM(stream_drop, "stream_drop")

    struct { const char *name; void (*fn)(void); int needs_full; } tests[] = {
        {"metadata", t_metadata, 0},
        {"invalid handles / double drop", t_invalid_handles, 0},
        {"sync call + ABI errors", t_sync_call_and_errors, 0},
        {"async call + async domain error", t_async_call_and_error, 0},
        {"future protocol (subscribe/take/drop)", t_future_protocol, 0},
        {"cancellation", t_cancellation, 1},
        {"cancel vs. completion race", t_cancel_race, 0},
        {"drop while pending / object drop", t_drop_pending, 1},
        {"stream + stream cancellation", t_stream, 1},
        {"concurrent calls (16 threads)", t_concurrent, 0},
        {"large payload + unicode", t_large_and_unicode, 1},
    };
    for (size_t i = 0; i < sizeof tests / sizeof *tests; i++) {
        if (tests[i].needs_full && !full) { printf("SKIP %s (basic profile)\n", tests[i].name); continue; }
        int before = failures;
        tests[i].fn();
        printf("%s %s\n", failures == before ? "PASS" : "FAIL", tests[i].name);
    }
    printf("%d checks, %d failures\n", checks, failures);
    return failures ? 1 : 0;
}

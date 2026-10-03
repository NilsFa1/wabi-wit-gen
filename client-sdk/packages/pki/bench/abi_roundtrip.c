/*
 * Cost of one async round trip through the Wabi ABI (call_async -> core thread
 * -> completion callback -> future_take), without any SDK runtime.
 *
 *   abi_roundtrip pki  <libgematik_pki.so> <tsl.xml>   rejected verify (no profiles): Java core (GraalVM)
 *   abi_roundtrip wabi <libwabi_client.so>             get-customer on the mem:// backend: Rust core
 */
#include "wabi_abi.h"
#include <dlfcn.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

typedef wabi_status (*call_fn)(uint64_t, uint32_t, wabi_bytes, void *);
static call_fn call_sync_, call_async_;
static wabi_status (*subscribe_)(uint64_t, wabi_future_callback, void *);
static wabi_status (*take_)(uint64_t, wabi_buffer *);
static wabi_status (*drop_)(uint64_t);
static void (*free_)(wabi_buffer);

static void *sym(void *lib, const char *prefix, const char *name) {
    char s[128];
    snprintf(s, sizeof s, "%s%s", prefix, name);
    void *p = dlsym(lib, s);
    if (!p) { fprintf(stderr, "missing %s\n", s); exit(1); }
    return p;
}

typedef struct { pthread_mutex_t m; pthread_cond_t c; int done; } waiter;
static void on_ready(void *ud, uint64_t f) {
    (void)f;
    waiter *w = ud;
    pthread_mutex_lock(&w->m);
    w->done = 1;
    pthread_cond_signal(&w->c);
    pthread_mutex_unlock(&w->m);
}

static double now_us(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1e6 + t.tv_nsec / 1e3;
}

static int cmp(const void *a, const void *b) { double x = *(const double *)a, y = *(const double *)b; return x < y ? -1 : x > y; }

/* one async call, waiting for the callback like an SDK does */
static void roundtrip(uint64_t obj, uint32_t method, wabi_bytes req) {
    uint64_t f;
    if (call_async_(obj, method, req, &f) != WABI_OK) { fprintf(stderr, "call_async failed\n"); exit(1); }
    waiter w = {PTHREAD_MUTEX_INITIALIZER, PTHREAD_COND_INITIALIZER, 0};
    if (subscribe_(f, on_ready, &w) == WABI_OK) {
        pthread_mutex_lock(&w.m);
        while (!w.done) pthread_cond_wait(&w.c, &w.m);
        pthread_mutex_unlock(&w.m);
    }
    wabi_buffer out;
    take_(f, &out);
    free_(out);
    drop_(f);
}

static uint64_t handle_of(wabi_buffer b) {
    uint64_t h = 0;
    if (b.len == 9) for (int i = 1; i < 9; i++) h = h << 8 | b.ptr[i];
    return h;
}

int main(int argc, char **argv) {
    if (argc < 3) return 2;
    int pki = strcmp(argv[1], "pki") == 0;
    const char *prefix = pki ? "gematik_pki_" : "wabi_client_";
    void *lib = dlopen(argv[2], RTLD_NOW);
    if (!lib) { fprintf(stderr, "%s\n", dlerror()); return 1; }
    call_sync_ = (call_fn)sym(lib, prefix, "call_sync");
    call_async_ = (call_fn)sym(lib, prefix, "call_async");
    subscribe_ = sym(lib, prefix, "future_subscribe");
    take_ = sym(lib, prefix, "future_take");
    drop_ = sym(lib, prefix, "future_drop");
    free_ = sym(lib, prefix, "buffer_free");

    static uint8_t buf[65536];
    size_t n = 0;
    uint64_t obj;
    wabi_buffer out;
    uint32_t method;
    static uint8_t req[64];
    size_t reqlen;
    if (pki) { /* ctor ["bench", bin(tsl)] */
        FILE *f = fopen(argv[3], "rb");
        uint8_t tsl[60000];
        size_t tl = fread(tsl, 1, sizeof tsl, f);
        fclose(f);
        buf[n++] = 0x92; buf[n++] = 0xa5; memcpy(buf + n, "bench", 5); n += 5;
        buf[n++] = 0xc5; buf[n++] = tl >> 8; buf[n++] = tl & 0xff; memcpy(buf + n, tsl, tl); n += tl;
        call_sync_(0, 0x0100, (wabi_bytes){buf, n}, &out);
        obj = handle_of(out); free_(out);
        /* verify [bin(1 byte), [], false] -> rejected before any crypto */
        const uint8_t r[] = {0x93, 0xc4, 0x01, 0x00, 0x90, 0xc2};
        memcpy(req, r, sizeof r); reqlen = sizeof r; method = 0x0101;
    } else { /* ctor [{1:"mem://b",2:"demo-key",3:5000}] + connect, then get-customer "4711" */
        const uint8_t c[] = {0x91, 0x83, 0x01, 0xa7, 'm','e','m',':','/','/','b', 0x02, 0xa8, 'd','e','m','o','-','k','e','y', 0x03, 0xcd, 0x13, 0x88};
        call_sync_(0, 0x0100, (wabi_bytes){c, sizeof c}, &out);
        obj = handle_of(out); free_(out);
        const uint8_t none[] = {0x90};
        roundtrip(obj, 0x0102, (wabi_bytes){none, 1});
        const uint8_t r[] = {0x91, 0xa4, '4', '7', '1', '1'};
        memcpy(req, r, sizeof r); reqlen = sizeof r; method = 0x0104;
    }
    enum { WARM = 5000, N = 20000 };
    static double t[N];
    for (int i = 0; i < WARM; i++) roundtrip(obj, method, (wabi_bytes){req, reqlen});
    for (int i = 0; i < N; i++) {
        double s = now_us();
        roundtrip(obj, method, (wabi_bytes){req, reqlen});
        t[i] = now_us() - s;
    }
    qsort(t, N, sizeof t[0], cmp);
    double sum = 0;
    for (int i = 0; i < N; i++) sum += t[i];
    printf("%-44s mean %6.1f us   p50 %6.1f us   p99 %6.1f us\n",
           pki ? "ABI async round trip, Java core (GraalVM)" : "ABI async round trip, Rust core (tokio)", sum / N, t[N / 2], t[N * 99 / 100]);
    return 0;
}

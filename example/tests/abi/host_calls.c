/*
 * Host calls (ABI 1.1 and 1.2, ABI.md §4a), language-neutral, against any core of the
 * example contract. This C program plays the APP: it owns an EC P-256 key
 * (OpenSSL stands in for the Secure Enclave / Android Keystore) and answers
 * the core's `host-keys` calls ASYNCHRONOUSLY from its own threads.
 *
 * It checks the protocol (no handler, async replies, domain errors, error
 * statuses, unknown/double replies, the instance a call belongs to,
 * cancellation while a host call is open and the notification about it).
 * That the signatures really are the app's is checked by the backend: it
 * verifies every request and reports the fingerprint of the signing key.
 *
 *   cc host_calls.c -I sdk/c -I sdk/abi -L<dir> -ldemo_client -lcrypto -lpthread
 *   WABI_TEST_ENDPOINT=http://127.0.0.1:9001 ./host_calls      (default: mem://)
 */
#define _GNU_SOURCE
#include <openssl/ec.h>
#include <openssl/evp.h>
#include <openssl/sha.h>
#include <openssl/x509.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "demo_client.h" /* sdk/c: demo_client_* prototypes */

static atomic_int failures, checks;
#define CHECK(c, ...) do { checks++; if (!(c)) { failures++; fprintf(stderr, "FAIL %s:%d: ", __FILE__, __LINE__); fprintf(stderr, __VA_ARGS__); fputc('\n', stderr); } } while (0)

/* wit/abi-ids.lock */
enum { M_PUBLIC_KEY = 0x0101, M_SIGN = 0x0102 };
enum { M_CTOR = 0x0200, M_CONNECT = 0x0202, M_KEY_FINGERPRINT = 0x0204, M_GET_CUSTOMER = 0x0205, M_SEND_COMMAND = 0x0207 };
enum { E_UNAUTHORIZED = 2, E_KEY_ERROR = 6 };

static EVP_PKEY *app_key, *other_key;

/* ------------------------------------------------------------ msgpack bits */

typedef struct { uint8_t b[4096]; size_t n; } buf;
static void put(buf *w, const void *p, size_t n) { memcpy(w->b + w->n, p, n); w->n += n; }
static void put_u8(buf *w, uint8_t v) { w->b[w->n++] = v; }
static void put_str(buf *w, const char *s) {
    size_t n = strlen(s);
    if (n < 32) put_u8(w, 0xa0 | (uint8_t)n); else { put_u8(w, 0xd9); put_u8(w, (uint8_t)n); }
    put(w, s, n);
}
static void put_bin(buf *w, const uint8_t *p, size_t n) { put_u8(w, 0xc4); put_u8(w, (uint8_t)n); put(w, p, n); }

typedef struct { const uint8_t *p; size_t n, i; int bad; } rd;
static uint8_t get_u8(rd *r) { if (r->i >= r->n) { r->bad = 1; return 0; } return r->p[r->i++]; }
static size_t get_len(rd *r, int bytes) { size_t v = 0; while (bytes--) v = v << 8 | get_u8(r); return v; }
/* returns pointer to string/bin bytes and sets *len */
static const uint8_t *get_blob(rd *r, size_t *len) {
    uint8_t m = get_u8(r);
    if (m >= 0xa0 && m <= 0xbf) *len = m & 0x1f;
    else if (m == 0xd9 || m == 0xc4) *len = get_len(r, 1);
    else if (m == 0xda || m == 0xc5) *len = get_len(r, 2);
    else { r->bad = 1; *len = 0; return NULL; }
    if (r->i + *len > r->n) { r->bad = 1; return NULL; }
    const uint8_t *s = r->p + r->i;
    r->i += *len;
    return s;
}

/* ------------------------------------------------------------ the "app" */

typedef struct { uint32_t method; uint8_t *req; size_t len; uint64_t call; } job;

static atomic_int handler_calls, held_calls, signatures, cancels;
static atomic_uint_fast64_t held_call; /* a call the app answers only when told to */
static atomic_uint_fast64_t last_object, cancelled_call;
static int reply_status_override = 0;   /* != 0: answer with this ABI status */

static void answer(uint32_t method, const uint8_t *req, size_t len, uint64_t call) {
    rd r = {req, len, 0, 0};
    uint8_t arity = get_u8(&r) & 0x0f;
    size_t klen;
    const uint8_t *kid = get_blob(&r, &klen);
    char key_id[64] = {0};
    if (kid && klen < sizeof key_id) memcpy(key_id, kid, klen);
    int forged = strcmp(key_id, "forged-key") == 0; /* announces the app key, signs with another one */
    buf w = {.n = 0};
    if (reply_status_override) {
        const char *msg = "keystore exploded";
        demo_client_host_reply(call, reply_status_override, (wabi_bytes){(const uint8_t *)msg, strlen(msg)});
        return;
    }
    if (strcmp(key_id, "app-key") != 0 && !forged) { /* result err: client-error.key-error(key-id) */
        put_u8(&w, 0x92); put_u8(&w, 1); put_u8(&w, 0x92); put_u8(&w, E_KEY_ERROR); put_str(&w, key_id);
    } else if (method == M_PUBLIC_KEY && arity == 1) {
        uint8_t *der = NULL;
        int n = i2d_PUBKEY(app_key, &der); /* SubjectPublicKeyInfo */
        put_u8(&w, 0x92); put_u8(&w, 0); put_bin(&w, der, (size_t)n);
        OPENSSL_free(der);
    } else if (method == M_SIGN && arity == 2) {
        size_t dlen;
        const uint8_t *digest = get_blob(&r, &dlen);
        uint8_t sig[128];
        size_t slen = sizeof sig;
        EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new(forged ? other_key : app_key, NULL);
        int ok = digest && dlen == 32 && EVP_PKEY_sign_init(ctx) == 1 && EVP_PKEY_sign(ctx, sig, &slen, digest, dlen) == 1;
        EVP_PKEY_CTX_free(ctx);
        CHECK(ok, "app could not sign the digest");
        atomic_fetch_add(&signatures, 1);
        put_u8(&w, 0x92); put_u8(&w, 0); put_bin(&w, sig, slen); /* ASN.1 DER, like platform keystores */
    } else {
        demo_client_host_reply(call, WABI_ERR_UNKNOWN_METHOD, (wabi_bytes){NULL, 0});
        return;
    }
    CHECK(demo_client_host_reply(call, WABI_OK, (wabi_bytes){w.b, w.n}) == WABI_OK, "host_reply");
    CHECK(demo_client_host_reply(call, WABI_OK, (wabi_bytes){w.b, w.n}) == WABI_ERR_INVALID_HANDLE,
          "a second reply to the same call must be rejected");
}

static void *worker(void *arg) {
    job *j = arg;
    /* answer LATER, from another thread, after a random 0-5 ms: with many calls
     * open at once the replies arrive in a different order than the requests */
    unsigned seed = (unsigned)j->call;
    usleep((useconds_t)(rand_r(&seed) % 5000));
    answer(j->method, j->req, j->len, j->call);
    free(j->req);
    free(j);
    return NULL;
}

/* The handler (ABI 1.2): runs on a core thread, must return quickly. `object`
 * is the client on whose behalf the core asks. */
static void on_host_call(void *user_data, uint64_t object, uint32_t method, const uint8_t *req, size_t len, uint64_t call) {
    CHECK(user_data == (void *)&app_key, "user_data passed through");
    atomic_store(&last_object, object);
    atomic_fetch_add(&handler_calls, 1);
    if (len > 4 && memmem(req, len, "slow-key", 8)) { /* keep it open; the request stays valid until the reply */
        atomic_store(&held_call, call);
        atomic_fetch_add(&held_calls, 1);
        return;
    }
    job *j = malloc(sizeof *j);
    *j = (job){method, malloc(len), len, call};
    memcpy(j->req, req, len);
    pthread_t t;
    pthread_create(&t, NULL, worker, j);
    pthread_detach(t);
}

/* ABI 1.2: the core gave up on a call. */
static void on_host_cancel(void *user_data, uint64_t call) {
    CHECK(user_data == (void *)&app_key, "user_data passed through (cancel)");
    atomic_store(&cancelled_call, call);
    atomic_fetch_add(&cancels, 1);
}

/* The ABI 1.1 handler: no object. */
static void on_host_call_v1(void *user_data, uint32_t method, const uint8_t *req, size_t len, uint64_t call) {
    on_host_call(user_data, 0, method, req, len, call);
}

/* ------------------------------------------------------------ helpers */

/* A client of the mock backend; `key_id` NULL: requests are not signed. */
static uint64_t new_client(const char *key_id) {
    const char *endpoint = getenv("WABI_TEST_ENDPOINT");
    buf w = {.n = 0};
    put_u8(&w, 0x91); put_u8(&w, key_id ? 0x84 : 0x83);
    put_u8(&w, 1); put_str(&w, endpoint ? endpoint : "mem://host-calls");
    put_u8(&w, 2); put_str(&w, "demo-key");
    put_u8(&w, 3); put_u8(&w, 0xcd); put_u8(&w, 0x27); put_u8(&w, 0x10); /* 10000 ms */
    if (key_id) { put_u8(&w, 4); put_str(&w, key_id); }
    wabi_buffer out;
    CHECK(demo_client_call_sync(0, M_CTOR, (wabi_bytes){w.b, w.n}, &out) == WABI_OK, "ctor");
    uint64_t h = 0;
    if (out.len == 9 && out.ptr[0] == 0xcf) for (int i = 1; i < 9; i++) h = h << 8 | out.ptr[i];
    else if (out.len == 1) h = out.ptr[0];
    demo_client_buffer_free(out);
    CHECK(h != 0, "ctor returns a handle");
    return h;
}

/* Waits for a future; returns the status, copies the payload. */
static wabi_status await_future(uint64_t f, buf *res) {
    for (int i = 0; i < 10000 && demo_client_future_poll(f) == WABI_PENDING; i++) usleep(1000);
    wabi_buffer out;
    wabi_status st = demo_client_future_take(f, &out);
    res->n = 0;
    if (out.len < sizeof res->b) put(res, out.ptr, out.len);
    demo_client_buffer_free(out);
    demo_client_future_drop(f);
    return st;
}

static wabi_status await_call(uint64_t obj, uint32_t method, buf *req, buf *res) {
    uint64_t f = 0;
    wabi_status st = demo_client_call_async(obj, method, (wabi_bytes){req->b, req->n}, &f);
    return st != WABI_OK ? st : await_future(f, res);
}

/* result<T, client-error>: 0 for ok (the reader then stands at T), else the error tag. */
static int result_tag(buf *res, rd *r) {
    *r = (rd){res->b, res->n, 0, 0};
    if (get_u8(r) != 0x92) return -1;
    if (get_u8(r) == 0) return 0;
    get_u8(r); /* variant array header */
    return get_u8(r);
}

static int connect_client(uint64_t client) {
    buf none = {.b = {0x90}, .n = 1}, res;
    rd r;
    CHECK(await_call(client, M_CONNECT, &none, &res) == WABI_OK, "connect completes");
    return result_tag(&res, &r);
}

static buf echo_request(const char *payload) {
    buf w = {.n = 0};
    put_u8(&w, 0x91); put_u8(&w, 0x82);
    put_u8(&w, 1); put_str(&w, "echo");
    put_u8(&w, 2); put_bin(&w, (const uint8_t *)payload, strlen(payload));
    return w;
}

int main(void) {
    app_key = EVP_EC_gen("P-256");
    other_key = EVP_EC_gen("P-256");
    buf none = {.b = {0x90}, .n = 1}, res;
    rd r;

    /* metadata */
    CHECK(demo_client_abi_version() == ((1u << 16) | 2u), "abi 1.2");
    CHECK(demo_client_abi_capabilities() & WABI_CAP_HOST_CALLS, "capability HOST_CALLS");
    CHECK(demo_client_abi_capabilities() & WABI_CAP_HOST_CALLS_2, "capability HOST_CALLS_2");
    CHECK(strncmp(demo_client_contract_id(), "demo:client@1.", 14) == 0, "contract id");

    /* 1. no handler registered: a domain error, never a hang */
    uint64_t client = new_client("app-key");
    CHECK(connect_client(client) == E_KEY_ERROR, "no handler -> client-error.key-error");

    CHECK(demo_client_host_register2(on_host_call, on_host_cancel, (void *)&app_key) == WABI_OK, "host_register2");

    /* 2. a client without a key never calls the app */
    uint64_t plain = new_client(NULL);
    CHECK(connect_client(plain) == 0, "connect without a key");
    CHECK(atomic_load(&handler_calls) == 0, "no host call without a key-id");
    CHECK(await_call(plain, M_KEY_FINGERPRINT, &none, &res) == WABI_OK && result_tag(&res, &r) == E_UNAUTHORIZED,
          "an unsigned session has no key fingerprint");
    demo_client_object_drop(plain);

    /* 3. the session is opened with the app's key: the backend verified the
     *    signatures and names the key (hex SHA-256 of its SubjectPublicKeyInfo) */
    CHECK(connect_client(client) == 0, "connect with the app's key");
    uint8_t *der = NULL, digest[32];
    int der_len = i2d_PUBKEY(app_key, &der);
    SHA256(der, (size_t)der_len, digest);
    OPENSSL_free(der);
    char want[65];
    for (int i = 0; i < 32; i++) snprintf(want + 2 * i, 3, "%02x", digest[i]);
    CHECK(await_call(client, M_KEY_FINGERPRINT, &none, &res) == WABI_OK, "key-fingerprint");
    size_t n = 0;
    const uint8_t *fp = result_tag(&res, &r) == 0 ? get_blob(&r, &n) : NULL;
    CHECK(fp && n == 64 && memcmp(fp, want, 64) == 0, "backend names the app's key: %.*s, expected %s", (int)n, fp ? (const char *)fp : "", want);

    /* 4. one signature per request */
    int before = atomic_load(&signatures);
    buf id = {.n = 0};
    put_u8(&id, 0x91); put_str(&id, "4711");
    CHECK(await_call(client, M_GET_CUSTOMER, &id, &res) == WABI_OK && result_tag(&res, &r) == 0, "signed get-customer");
    CHECK(memmem(res.b, res.n, "Erika Mustermann", 16) != NULL, "customer payload");
    CHECK(atomic_load(&signatures) == before + 1, "exactly one signature for one request");
    CHECK(atomic_load(&last_object) == client, "the handler learns which client asks");
    uint64_t second = new_client("app-key");
    CHECK(connect_client(second) == 0, "connect a second client");
    CHECK(atomic_load(&last_object) == second, "... and tells two clients apart");
    demo_client_object_drop(second);

    /* 5. concurrency: 64 requests at once, each with its own host call, replies
     *    arriving out of order from many app threads. Every signature must end
     *    up on its own request (else the backend answers `unauthorized`) and
     *    every result must belong to its own request. */
    enum { PAR = 64 };
    uint64_t fs[PAR];
    for (int i = 0; i < PAR; i++) {
        char payload[32];
        snprintf(payload, sizeof payload, "payload-%d-end", i);
        buf req = echo_request(payload);
        CHECK(demo_client_call_async(client, M_SEND_COMMAND, (wabi_bytes){req.b, req.n}, &fs[i]) == WABI_OK, "async %d", i);
    }
    int matched = 0;
    for (int i = 0; i < PAR; i++) {
        char payload[32];
        snprintf(payload, sizeof payload, "payload-%d-end", i);
        CHECK(await_future(fs[i], &res) == WABI_OK, "concurrent take %d", i);
        int ok = result_tag(&res, &r) == 0 && memmem(res.b, res.n, payload, strlen(payload)) != NULL;
        CHECK(ok, "call %d got its own, correctly signed answer", i);
        matched += ok;
    }
    CHECK(matched == PAR, "%d of %d parallel requests matched", matched, PAR);

    /* 6. the app's domain error travels back to the caller */
    uint64_t unknown = new_client("no-such-key");
    buf none2 = none;
    CHECK(await_call(unknown, M_CONNECT, &none2, &res) == WABI_OK, "unknown key call");
    CHECK(result_tag(&res, &r) == E_KEY_ERROR && memmem(res.b, res.n, "no-such-key", 11) != NULL, "the app's key-error is propagated");

    /* 7. the app answers with an ABI error status */
    uint64_t broken = new_client("app-key");
    reply_status_override = WABI_ERR_INTERNAL;
    CHECK(connect_client(broken) == E_KEY_ERROR, "error status -> key-error");
    reply_status_override = 0;

    /* 8. a signature that does not belong to the announced key is rejected by the backend */
    uint64_t forged = new_client("forged-key");
    CHECK(connect_client(forged) == E_UNAUTHORIZED, "forged signature -> unauthorized");

    /* 9. unknown call ids */
    CHECK(demo_client_host_reply(0x7fffffff, WABI_OK, (wabi_bytes){NULL, 0}) == WABI_ERR_INVALID_HANDLE, "unknown call id");

    /* 10. cancel while a host call is open: the app is told once, and its late
     *     reply is accepted and discarded */
    CHECK(atomic_load(&cancels) == 0, "no cancel notification for calls that were answered");
    uint64_t slow = new_client("slow-key"), f = 0;
    CHECK(demo_client_call_async(slow, M_CONNECT, (wabi_bytes){none.b, none.n}, &f) == WABI_OK, "slow call");
    for (int i = 0; i < 5000 && atomic_load(&held_calls) == 0; i++) usleep(1000);
    CHECK(atomic_load(&held_calls) == 1, "host call is open");
    CHECK(demo_client_future_cancel(f) == WABI_OK, "cancel");
    for (int i = 0; i < 5000 && demo_client_future_poll(f) == WABI_PENDING; i++) usleep(1000);
    wabi_buffer out;
    CHECK(demo_client_future_take(f, &out) == WABI_ERR_CANCELLED, "cancelled while the app was busy");
    demo_client_buffer_free(out);
    demo_client_future_drop(f);
    for (int i = 0; i < 5000 && atomic_load(&cancels) == 0; i++) usleep(1000);
    CHECK(atomic_load(&cancels) == 1 && atomic_load(&cancelled_call) == atomic_load(&held_call),
          "the app is told which call was abandoned (%d notifications)", atomic_load(&cancels));
    uint8_t late[] = {0x92, 0x01, 0x92, E_KEY_ERROR, 0xa1, 'x'};
    CHECK(demo_client_host_reply(atomic_load(&held_call), WABI_OK, (wabi_bytes){late, sizeof late}) == WABI_OK,
          "late reply after cancellation is accepted (frees the request)");

    demo_client_object_drop(slow);
    demo_client_object_drop(forged);
    demo_client_object_drop(broken);
    demo_client_object_drop(unknown);
    demo_client_object_drop(client);
    usleep(20000);
    CHECK(atomic_load(&cancels) == 1, "exactly one cancel notification");

    /* 11. a host of ABI 1.1 (host_register): same calls, no object, no notification */
    CHECK(demo_client_host_register(on_host_call_v1, (void *)&app_key) == WABI_OK, "host_register (1.1)");
    uint64_t legacy = new_client("app-key");
    CHECK(connect_client(legacy) == 0, "connect through the 1.1 handler");
    CHECK(atomic_load(&last_object) == 0, "the 1.1 handler has no object");
    demo_client_object_drop(legacy);

    /* 12. after unregistering, the handler is never invoked again */
    CHECK(demo_client_host_register2(NULL, NULL, NULL) == WABI_OK, "unregister");
    int calls = atomic_load(&handler_calls);
    uint64_t late_client = new_client("app-key");
    CHECK(connect_client(late_client) == E_KEY_ERROR, "no handler again -> key-error");
    CHECK(atomic_load(&handler_calls) == calls, "no handler invocation after unregister");
    demo_client_object_drop(late_client);
    usleep(20000); /* let detached app threads finish */
    EVP_PKEY_free(app_key);
    EVP_PKEY_free(other_key);
    printf("host calls: %d checks, %d failures\n", atomic_load(&checks), atomic_load(&failures));
    return atomic_load(&failures) != 0;
}

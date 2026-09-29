/*
 * Host calls (ABI 1.1, ABI.md §4a), language-neutral, against the gematik:zero
 * PoC core (Go + libzero). This C program plays the APP: it owns an EC P-256
 * key (OpenSSL stands in for the Secure Enclave / Android Keystore) and answers
 * the core's `host-keys` calls ASYNCHRONOUSLY from its own threads.
 *
 * It checks the protocol (no handler, async replies, domain errors, error
 * statuses, unknown/double replies, cancellation while a host call is open)
 * and verifies the resulting DPoP proof and JWK thumbprint independently.
 *
 *   cc host_calls.c -I generated/c -I abi -L<dir> -lgematik_zero -lcrypto -lpthread
 */
#include <openssl/bn.h>
#include <openssl/core_names.h>
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

#include "gematik_zero.h" /* generated/c: gematik_zero_* prototypes */

static atomic_int failures, checks;
#define CHECK(c, ...) do { checks++; if (!(c)) { failures++; fprintf(stderr, "FAIL %s:%d: ", __FILE__, __LINE__); fprintf(stderr, __VA_ARGS__); fputc('\n', stderr); } } while (0)

enum { M_PUBLIC_KEY = 0x0101, M_SIGN = 0x0102, M_CTOR = 0x0200, M_THUMBPRINT = 0x0201, M_CREATE_PROOF = 0x0202 };
enum { ZERO_KEY_NOT_FOUND = 1, ZERO_CRYPTO = 2, ZERO_INVALID_INPUT = 3 };

static EVP_PKEY *app_key;

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

static atomic_int handler_calls, held_calls;
static atomic_uint_fast64_t held_call; /* a call the app answers only when told to */
static int reply_status_override = 0;   /* != 0: answer with this ABI status */

static void answer(uint32_t method, const uint8_t *req, size_t len, uint64_t call) {
    rd r = {req, len, 0, 0};
    uint8_t arity = get_u8(&r) & 0x0f;
    size_t klen;
    const uint8_t *kid = get_blob(&r, &klen);
    char key_id[64] = {0};
    if (kid && klen < sizeof key_id) memcpy(key_id, kid, klen);
    buf w = {.n = 0};
    if (reply_status_override) {
        const char *msg = "keystore exploded";
        gematik_zero_host_reply(call, reply_status_override, (acme_bytes){(const uint8_t *)msg, strlen(msg)});
        return;
    }
    if (strcmp(key_id, "dpop-key") != 0) { /* result err: zero-error.key-not-found(key-id) */
        put_u8(&w, 0x92); put_u8(&w, 1); put_u8(&w, 0x92); put_u8(&w, ZERO_KEY_NOT_FOUND); put_str(&w, key_id);
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
        EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new(app_key, NULL);
        int ok = digest && dlen == 32 && EVP_PKEY_sign_init(ctx) == 1 && EVP_PKEY_sign(ctx, sig, &slen, digest, dlen) == 1;
        EVP_PKEY_CTX_free(ctx);
        CHECK(ok, "app could not sign the digest");
        put_u8(&w, 0x92); put_u8(&w, 0); put_bin(&w, sig, slen); /* ASN.1 DER, like platform keystores */
    } else {
        gematik_zero_host_reply(call, ACME_ERR_UNKNOWN_METHOD, (acme_bytes){NULL, 0});
        return;
    }
    CHECK(gematik_zero_host_reply(call, ACME_OK, (acme_bytes){w.b, w.n}) == ACME_OK, "host_reply");
    CHECK(gematik_zero_host_reply(call, ACME_OK, (acme_bytes){w.b, w.n}) == ACME_ERR_INVALID_HANDLE,
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

/* The handler: runs on a core thread, must return quickly. */
static void on_host_call(void *user_data, uint32_t method, const uint8_t *req, size_t len, uint64_t call) {
    CHECK(user_data == (void *)&app_key, "user_data passed through");
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

/* ------------------------------------------------------------ helpers */

static uint64_t new_signer(const char *key_id) {
    buf w = {.n = 0};
    put_u8(&w, 0x91); put_str(&w, key_id);
    acme_buffer out;
    CHECK(gematik_zero_call_sync(0, M_CTOR, (acme_bytes){w.b, w.n}, &out) == ACME_OK, "ctor");
    uint64_t h = 0;
    if (out.len == 9 && out.ptr[0] == 0xcf) for (int i = 1; i < 9; i++) h = h << 8 | out.ptr[i];
    gematik_zero_buffer_free(out);
    CHECK(h != 0, "ctor returns a handle");
    return h;
}

/* Runs an async call and waits; returns the status, copies the payload. */
static acme_status await_call(uint64_t obj, uint32_t method, buf *req, buf *res) {
    uint64_t f = 0;
    acme_status st = gematik_zero_call_async(obj, method, (acme_bytes){req->b, req->n}, &f);
    if (st != ACME_OK) return st;
    for (int i = 0; i < 5000 && gematik_zero_future_poll(f) == ACME_PENDING; i++) usleep(1000);
    acme_buffer out;
    st = gematik_zero_future_take(f, &out);
    res->n = 0;
    if (out.len < sizeof res->b) put(res, out.ptr, out.len);
    gematik_zero_buffer_free(out);
    gematik_zero_future_drop(f);
    return st;
}

/* result<string, zero-error>: returns the ok string (NUL-terminated) or NULL and the error tag. */
static char *result_string(buf *res, int *err_tag) {
    static char s[4096];
    rd r = {res->b, res->n, 0, 0};
    *err_tag = 0;
    if (get_u8(&r) != 0x92) return NULL;
    if (get_u8(&r) != 0) {
        get_u8(&r); /* variant array header */
        *err_tag = get_u8(&r);
        return NULL;
    }
    size_t n;
    const uint8_t *p = get_blob(&r, &n);
    if (!p || r.bad || n >= sizeof s) return NULL;
    memcpy(s, p, n);
    s[n] = 0;
    return s;
}

static size_t b64url_decode(const char *in, size_t n, uint8_t *out) {
    char tmp[4096] = {0};
    size_t m = 0;
    for (size_t i = 0; i < n; i++) tmp[m++] = in[i] == '-' ? '+' : in[i] == '_' ? '/' : in[i];
    while (m % 4) tmp[m++] = '=';
    int len = EVP_DecodeBlock(out, (const uint8_t *)tmp, (int)m);
    int pad = 0;
    for (size_t i = m; i > 0 && tmp[i - 1] == '='; i--) pad++;
    return len < 0 ? 0 : (size_t)len - (size_t)pad;
}

static void b64url_encode(const uint8_t *in, size_t n, char *out) {
    int m = EVP_EncodeBlock((uint8_t *)out, in, (int)n);
    for (int i = 0; i < m; i++) out[i] = out[i] == '+' ? '-' : out[i] == '/' ? '_' : out[i];
    while (m > 0 && out[m - 1] == '=') out[--m] = 0;
}

/* RFC 7638 thumbprint of the app key, computed here independently of libzero. */
static void expected_thumbprint(char *out) {
    BIGNUM *x = NULL, *y = NULL;
    EVP_PKEY_get_bn_param(app_key, OSSL_PKEY_PARAM_EC_PUB_X, &x);
    EVP_PKEY_get_bn_param(app_key, OSSL_PKEY_PARAM_EC_PUB_Y, &y);
    uint8_t xb[32], yb[32];
    BN_bn2binpad(x, xb, 32);
    BN_bn2binpad(y, yb, 32);
    char xs[64], ys[64], json[256], digest_b64[64];
    b64url_encode(xb, 32, xs);
    b64url_encode(yb, 32, ys);
    snprintf(json, sizeof json, "{\"crv\":\"P-256\",\"kty\":\"EC\",\"x\":\"%s\",\"y\":\"%s\"}", xs, ys);
    uint8_t d[32];
    SHA256((const uint8_t *)json, strlen(json), d);
    b64url_encode(d, 32, digest_b64);
    strcpy(out, digest_b64);
    BN_free(x);
    BN_free(y);
}

/* Verifies an ES256 JWS (r||s) with the app's public key. */
static int verify_jws(const char *jwt) {
    const char *dot2 = strrchr(jwt, '.');
    if (!dot2) return 0;
    uint8_t rs[128];
    size_t n = b64url_decode(dot2 + 1, strlen(dot2 + 1), rs);
    if (n != 64) return 0;
    ECDSA_SIG *sig = ECDSA_SIG_new();
    ECDSA_SIG_set0(sig, BN_bin2bn(rs, 32, NULL), BN_bin2bn(rs + 32, 32, NULL));
    uint8_t *der = NULL;
    int dlen = i2d_ECDSA_SIG(sig, &der);
    uint8_t digest[32];
    SHA256((const uint8_t *)jwt, (size_t)(dot2 - jwt), digest);
    EVP_PKEY_CTX *ctx = EVP_PKEY_CTX_new(app_key, NULL);
    int ok = EVP_PKEY_verify_init(ctx) == 1 && EVP_PKEY_verify(ctx, der, (size_t)dlen, digest, 32) == 1;
    EVP_PKEY_CTX_free(ctx);
    OPENSSL_free(der);
    ECDSA_SIG_free(sig);
    return ok;
}

static char *jwt_part(const char *jwt, int idx) {
    static char out[2][2048];
    const char *start = jwt;
    for (int i = 0; i < idx; i++) start = strchr(start, '.') + 1;
    const char *end = strchr(start, '.');
    uint8_t tmp[2048];
    size_t n = b64url_decode(start, (size_t)(end - start), tmp);
    memcpy(out[idx], tmp, n);
    out[idx][n] = 0;
    return out[idx];
}

static buf proof_request(const char *token, const char *nonce) {
    buf w = {.n = 0};
    put_u8(&w, 0x94);
    put_str(&w, "POST");
    put_str(&w, "https://zero.example/token");
    if (token) put_str(&w, token); else put_u8(&w, 0xc0);
    if (nonce) put_str(&w, nonce); else put_u8(&w, 0xc0);
    return w;
}

int main(void) {
    app_key = EVP_EC_gen("P-256");
    buf none = {.b = {0x90}, .n = 1}, res;
    int tag;

    /* metadata */
    CHECK(gematik_zero_abi_version() == ((1u << 16) | 1u), "abi 1.1");
    CHECK(gematik_zero_abi_capabilities() & ACME_CAP_HOST_CALLS, "capability HOST_CALLS");
    CHECK(strcmp(gematik_zero_contract_id(), "gematik:zero@0.1.0") == 0, "contract id");

    uint64_t signer = new_signer("dpop-key");

    /* 1. no handler registered: a domain error, never a hang */
    CHECK(await_call(signer, M_THUMBPRINT, &none, &res) == ACME_OK, "thumbprint without handler");
    CHECK(!result_string(&res, &tag) && tag == ZERO_CRYPTO, "no handler -> zero-error.crypto (tag %d)", tag);

    CHECK(gematik_zero_host_register(on_host_call, (void *)&app_key) == ACME_OK, "host_register");

    /* 2. thumbprint (libzero) matches an independent RFC 7638 computation */
    char want[64];
    expected_thumbprint(want);
    CHECK(await_call(signer, M_THUMBPRINT, &none, &res) == ACME_OK, "thumbprint");
    char *tp = result_string(&res, &tag);
    CHECK(tp && strcmp(tp, want) == 0, "thumbprint %s, expected %s", tp ? tp : "(error)", want);

    /* 3. DPoP proof: the signature was made by the app's key */
    buf req = proof_request("access-token-123", "server-nonce");
    CHECK(await_call(signer, M_CREATE_PROOF, &req, &res) == ACME_OK, "create-proof");
    char *jwt = result_string(&res, &tag);
    CHECK(jwt != NULL, "create-proof ok (err tag %d)", tag);
    if (jwt) {
        char *copy = strdup(jwt);
        CHECK(verify_jws(copy), "DPoP proof signature verifies with the app key");
        char *hdr = jwt_part(copy, 0), *payload = jwt_part(copy, 1);
        CHECK(strstr(hdr, "\"typ\":\"dpop+jwt\"") && strstr(hdr, "\"alg\":\"ES256\"") && strstr(hdr, "\"jwk\""), "header: %s", hdr);
        CHECK(strstr(payload, "\"htm\":\"POST\"") && strstr(payload, "\"htu\":\"https://zero.example/token\"") &&
                  strstr(payload, "\"nonce\":\"server-nonce\"") && strstr(payload, "\"ath\":") && strstr(payload, "\"jti\":"),
              "payload: %s", payload);
        /* ath = b64url(sha256(token)) */
        uint8_t d[32];
        char ath[64];
        SHA256((const uint8_t *)"access-token-123", 16, d);
        b64url_encode(d, 32, ath);
        CHECK(strstr(payload, ath) != NULL, "ath claim");
        free(copy);
    }

    /* 4. concurrency: 64 operations at once, each doing its own host calls
     *    (public-key + sign), replies arriving out of order from many app
     *    threads. Every result must belong to its own request: the proof names
     *    its own URL and is signed over exactly its own content; every 4th call
     *    uses an unknown key and must get exactly that error. */
    enum { PAR = 64 };
    uint64_t fs[PAR];
    uint64_t stranger = new_signer("no-such-key-parallel");
    for (int i = 0; i < PAR; i++) {
        buf r = {.n = 0};
        char url[64];
        snprintf(url, sizeof url, "https://zero.example/r/%d", i);
        put_u8(&r, 0x94); put_str(&r, "POST"); put_str(&r, url); put_u8(&r, 0xc0); put_u8(&r, 0xc0);
        CHECK(gematik_zero_call_async(i % 4 == 3 ? stranger : signer, M_CREATE_PROOF, (acme_bytes){r.b, r.n}, &fs[i]) == ACME_OK,
              "async %d", i);
    }
    int matched = 0;
    for (int i = 0; i < PAR; i++) {
        for (int k = 0; k < 10000 && gematik_zero_future_poll(fs[i]) == ACME_PENDING; k++) usleep(1000);
        acme_buffer out;
        CHECK(gematik_zero_future_take(fs[i], &out) == ACME_OK, "concurrent take %d", i);
        buf b = {.n = 0};
        put(&b, out.ptr, out.len);
        gematik_zero_buffer_free(out);
        char *j = result_string(&b, &tag);
        if (i % 4 == 3) {
            CHECK(!j && tag == ZERO_KEY_NOT_FOUND, "call %d (unknown key) got its own error (tag %d)", i, tag);
        } else if (j) {
            char *copy = strdup(j), want[80];
            snprintf(want, sizeof want, "\"htu\":\"https://zero.example/r/%d\"", i);
            int ok = verify_jws(copy) && strstr(jwt_part(copy, 1), want) != NULL;
            CHECK(ok, "call %d got its own, correctly signed proof", i);
            matched += ok;
            free(copy);
        } else {
            CHECK(0, "call %d failed (tag %d)", i, tag);
        }
        gematik_zero_future_drop(fs[i]);
    }
    CHECK(matched == PAR - PAR / 4, "%d of %d parallel proofs matched their request", matched, PAR - PAR / 4);
    gematik_zero_object_drop(stranger);

    /* 5. the app's domain error travels back to the caller */
    uint64_t unknown = new_signer("no-such-key");
    CHECK(await_call(unknown, M_CREATE_PROOF, &req, &res) == ACME_OK, "unknown key call");
    CHECK(!result_string(&res, &tag) && tag == ZERO_KEY_NOT_FOUND, "key-not-found propagated (tag %d)", tag);

    /* 6. the app answers with an ABI error status */
    reply_status_override = ACME_ERR_INTERNAL;
    CHECK(await_call(signer, M_THUMBPRINT, &none, &res) == ACME_OK, "status override call");
    CHECK(!result_string(&res, &tag) && tag == ZERO_CRYPTO, "error status -> crypto error (tag %d)", tag);
    reply_status_override = 0;

    /* 7. core-side argument validation needs no host call */
    int before = atomic_load(&handler_calls);
    buf bad = {.n = 0};
    put_u8(&bad, 0x94); put_str(&bad, ""); put_str(&bad, "ftp://x"); put_u8(&bad, 0xc0); put_u8(&bad, 0xc0);
    CHECK(await_call(signer, M_CREATE_PROOF, &bad, &res) == ACME_OK, "invalid input call");
    CHECK(!result_string(&res, &tag) && tag == ZERO_INVALID_INPUT, "invalid-input (tag %d)", tag);
    CHECK(atomic_load(&handler_calls) == before, "no host call for invalid input");

    /* 8. unknown call ids */
    CHECK(gematik_zero_host_reply(0x7fffffff, ACME_OK, (acme_bytes){NULL, 0}) == ACME_ERR_INVALID_HANDLE, "unknown call id");

    /* 9. cancel while a host call is open; the late reply is accepted and discarded */
    uint64_t slow = new_signer("slow-key"), f = 0;
    CHECK(gematik_zero_call_async(slow, M_THUMBPRINT, (acme_bytes){none.b, none.n}, &f) == ACME_OK, "slow call");
    for (int i = 0; i < 5000 && atomic_load(&held_calls) == 0; i++) usleep(1000);
    CHECK(atomic_load(&held_calls) == 1, "host call is open");
    CHECK(gematik_zero_future_cancel(f) == ACME_OK, "cancel");
    for (int i = 0; i < 5000 && gematik_zero_future_poll(f) == ACME_PENDING; i++) usleep(1000);
    acme_buffer out;
    CHECK(gematik_zero_future_take(f, &out) == ACME_ERR_CANCELLED, "cancelled while the app was busy");
    gematik_zero_buffer_free(out);
    gematik_zero_future_drop(f);
    uint8_t late[] = {0x92, 0x01, 0x92, 0x01, 0xa1, 'x'};
    CHECK(gematik_zero_host_reply(atomic_load(&held_call), ACME_OK, (acme_bytes){late, sizeof late}) == ACME_OK,
          "late reply after cancellation is accepted (frees the request)");

    gematik_zero_object_drop(slow);
    gematik_zero_object_drop(unknown);
    gematik_zero_object_drop(signer);
    CHECK(gematik_zero_host_register(NULL, NULL) == ACME_OK, "unregister");
    usleep(20000); /* let detached app threads finish */
    EVP_PKEY_free(app_key);
    printf("host calls: %d checks, %d failures\n", atomic_load(&checks), atomic_load(&failures));
    return atomic_load(&failures) != 0;
}

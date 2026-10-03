/* A deterministic host-register race: retain the callback pointer, let Dart
 * unregister/replace its dispatcher, then invoke the retained pointer.
 * This is a test fixture, not a PKI/DPoP implementation. */
#define WABI_PREFIX gematik_zero
#include "wabi_abi_functions.h"
#include <stdatomic.h>
#include <stdint.h>

static wabi_host_handler current, retained;
static void *current_ud, *retained_ud;
static atomic_int replies, status_seen, byte_seen;
uint32_t gematik_zero_abi_version(void) { return (1u << 16) | 1u; }
uint64_t gematik_zero_abi_capabilities(void) { return 0x7fu; }
const char *gematik_zero_contract_id(void) { return "gematik:zero@0.1.0"; }
const char *gematik_zero_core_info(void) { return "host-lifetime-probe"; }
const char *gematik_zero_status_name(wabi_status status) { (void)status; return "probe"; }
void gematik_zero_buffer_free(wabi_buffer buffer) { (void)buffer; }
wabi_status gematik_zero_call_sync(uint64_t h, uint32_t m, wabi_bytes b, wabi_buffer *out) {
    (void)h; (void)m; (void)b;
    if (!out) return WABI_ERR_INVALID_ARGUMENT;
    *out = (wabi_buffer){0}; return WABI_ERR_UNSUPPORTED;
}
wabi_status gematik_zero_call_async(uint64_t h, uint32_t m, wabi_bytes b, uint64_t *out) {
    (void)h; (void)m; (void)b;
    if (!out) return WABI_ERR_INVALID_ARGUMENT;
    *out = 0; return WABI_ERR_UNSUPPORTED;
}
wabi_status gematik_zero_future_take(uint64_t h, wabi_buffer *out) {
    (void)h;
    if (!out) return WABI_ERR_INVALID_ARGUMENT;
    *out = (wabi_buffer){0}; return WABI_ERR_INVALID_HANDLE;
}
wabi_status gematik_zero_future_subscribe(uint64_t h, wabi_future_callback cb, void *ud) {
    (void)h; (void)cb; (void)ud; return WABI_ERR_INVALID_HANDLE;
}
wabi_status gematik_zero_stream_next(uint64_t h, uint64_t *out) {
    (void)h;
    if (!out) return WABI_ERR_INVALID_ARGUMENT;
    *out = 0; return WABI_ERR_INVALID_HANDLE;
}
#define INVALID(name) wabi_status gematik_zero_##name(uint64_t h) { (void)h; return WABI_ERR_INVALID_HANDLE; }
INVALID(object_drop)
INVALID(future_poll)
INVALID(future_cancel)
INVALID(future_drop)
INVALID(stream_cancel)
INVALID(stream_drop)
wabi_status gematik_zero_host_register(wabi_host_handler h, void *ud) {
    current = h; current_ud = ud; return WABI_OK;
}
wabi_status gematik_zero_host_reply(uint64_t call, wabi_status status, wabi_bytes response) {
    (void)call;
    atomic_store(&status_seen, status);
    atomic_store(&byte_seen, response.len ? response.ptr[0] : -1);
    atomic_fetch_add(&replies, 1);
    return WABI_OK;
}
void probe_snapshot(void) {
    retained = current; retained_ud = current_ud;
    atomic_store(&replies, 0);
}
void probe_call_retained(void) { retained(retained_ud, 0, NULL, 0, 1); }
int probe_replies(void) { return atomic_load(&replies); }
int probe_status(void) { return atomic_load(&status_seen); }
int probe_byte(void) { return atomic_load(&byte_seen); }

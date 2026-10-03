/*
 * Two WIT packages, two cores, ONE binary — the reason for per-package symbol
 * prefixes (ABI.md §1.1). Statically links:
 *
 *   libwabi_client.a  (Rust core, package wabi:client, symbols wabi_client_*)
 *   libgeo_lib.a      (Zig core built with prefix geo_lib, symbols geo_lib_*)
 *
 * and drives both through the ABI. Without prefixes both archives would define
 * wabi_call_sync & co. and the link would fail (or, with shared libraries,
 * one core would silently shadow the other).
 */
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "wabi_client.h" /* generated/c: wabi_client_* prototypes */

/* A second package, declared straight from the normative header. */
#define WABI_PREFIX geo_lib
#include "wabi_abi_functions.h"
#undef WABI_PREFIX

static int failures = 0;
#define CHECK(c, ...) do { if (!(c)) { failures++; fprintf(stderr, "FAIL: " __VA_ARGS__); fputc('\n', stderr); } } while (0)

/* ctor args: [ {1: endpoint, 2: api-key, 3: timeout} ] */
static const uint8_t CTOR[] = {0x91, 0x83, 0x01, 0xa6, 'm', 'e', 'm', ':', '/', '/', 0x02, 0xa8,
                               'd', 'e', 'm', 'o', '-', 'k', 'e', 'y', 0x03, 0xcd, 0x13, 0x88};
static const uint8_t NO_ARGS[] = {0x90};
static const uint8_t GET_4711[] = {0x91, 0xa4, '4', '7', '1', '1'};

/* Generic driver, instantiated for each prefix: create client, connect, get-customer 4711. */
#define DRIVE(P)                                                                              \
    static int drive_##P(void) {                                                               \
        wabi_buffer out;                                                                       \
        wabi_bytes ctor = {CTOR, sizeof CTOR}, none = {NO_ARGS, 1}, get = {GET_4711, sizeof GET_4711}; \
        if (P##_call_sync(0, 0x0100, ctor, &out) != WABI_OK) return 0;                         \
        uint64_t client = out.len == 1 ? out.ptr[0] : 0;                                       \
        if (out.len == 9) for (int i = 1; i < 9; i++) client = (client << 8) | out.ptr[i];     \
        P##_buffer_free(out);                                                                  \
        const wabi_bytes *steps[] = {&none, &get};                                             \
        const uint32_t methods[] = {0x0102, 0x0104};                                           \
        int found = 0;                                                                         \
        for (int s = 0; s < 2; s++) {                                                          \
            uint64_t f = 0;                                                                    \
            if (P##_call_async(client, methods[s], *steps[s], &f) != WABI_OK) return 0;        \
            while (P##_future_poll(f) == WABI_PENDING) usleep(1000);                           \
            wabi_status st = P##_future_take(f, &out);                                         \
            if (s == 1) found = st == WABI_OK && out.len > 2 && out.ptr[1] == 0 &&             \
                                memmem(out.ptr, out.len, "Erika", 5) != NULL;                  \
            P##_buffer_free(out);                                                              \
            P##_future_drop(f);                                                                \
        }                                                                                      \
        P##_object_drop(client);                                                               \
        return found;                                                                          \
    }

DRIVE(wabi_client)
DRIVE(geo_lib)

int main(void) {
    printf("wabi_client: %s, contract %s\n", wabi_client_core_info(), wabi_client_contract_id());
    printf("geo_lib:     %s, contract %s\n", geo_lib_core_info(), geo_lib_contract_id());
    CHECK(strstr(wabi_client_core_info(), "rust") != NULL, "wabi_client_* must resolve to the Rust core");
    CHECK(strstr(geo_lib_core_info(), "zig") != NULL, "geo_lib_* must resolve to the Zig core");
    CHECK(drive_wabi_client(), "request through wabi_client_* failed");
    CHECK(drive_geo_lib(), "request through geo_lib_* failed");
    printf("%s: two packages statically linked into one binary\n", failures ? "FAIL" : "PASS");
    return failures ? 1 : 0;
}

/*
 * Two WIT packages, two cores, ONE binary — the reason for per-package symbol
 * prefixes (ABI.md §1.1). Statically links:
 *
 *   libacme_client.a  (Rust core, package acme:client, symbols acme_client_*)
 *   libgeo_lib.a      (Zig core built with prefix geo_lib, symbols geo_lib_*)
 *
 * and drives both through the ABI. Without prefixes both archives would define
 * acme_call_sync & co. and the link would fail (or, with shared libraries,
 * one core would silently shadow the other).
 */
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "acme_client.h" /* generated/c: acme_client_* prototypes */

/* A second package, declared straight from the normative header. */
#define ACME_PREFIX geo_lib
#include "acme_abi_functions.h"
#undef ACME_PREFIX

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
        acme_buffer out;                                                                       \
        acme_bytes ctor = {CTOR, sizeof CTOR}, none = {NO_ARGS, 1}, get = {GET_4711, sizeof GET_4711}; \
        if (P##_call_sync(0, 0x0100, ctor, &out) != ACME_OK) return 0;                         \
        uint64_t client = out.len == 1 ? out.ptr[0] : 0;                                       \
        if (out.len == 9) for (int i = 1; i < 9; i++) client = (client << 8) | out.ptr[i];     \
        P##_buffer_free(out);                                                                  \
        const acme_bytes *steps[] = {&none, &get};                                             \
        const uint32_t methods[] = {0x0102, 0x0104};                                           \
        int found = 0;                                                                         \
        for (int s = 0; s < 2; s++) {                                                          \
            uint64_t f = 0;                                                                    \
            if (P##_call_async(client, methods[s], *steps[s], &f) != ACME_OK) return 0;        \
            while (P##_future_poll(f) == ACME_PENDING) usleep(1000);                           \
            acme_status st = P##_future_take(f, &out);                                         \
            if (s == 1) found = st == ACME_OK && out.len > 2 && out.ptr[1] == 0 &&             \
                                memmem(out.ptr, out.len, "Erika", 5) != NULL;                  \
            P##_buffer_free(out);                                                              \
            P##_future_drop(f);                                                                \
        }                                                                                      \
        P##_object_drop(client);                                                               \
        return found;                                                                          \
    }

DRIVE(acme_client)
DRIVE(geo_lib)

int main(void) {
    printf("acme_client: %s, contract %s\n", acme_client_core_info(), acme_client_contract_id());
    printf("geo_lib:     %s, contract %s\n", geo_lib_core_info(), geo_lib_contract_id());
    CHECK(strstr(acme_client_core_info(), "rust") != NULL, "acme_client_* must resolve to the Rust core");
    CHECK(strstr(geo_lib_core_info(), "zig") != NULL, "geo_lib_* must resolve to the Zig core");
    CHECK(drive_acme_client(), "request through acme_client_* failed");
    CHECK(drive_geo_lib(), "request through geo_lib_* failed");
    printf("%s: two packages statically linked into one binary\n", failures ? "FAIL" : "PASS");
    return failures ? 1 : 0;
}

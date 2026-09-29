/* Deliberately incompatible "cores" for the version-mismatch tests.
 * Built three times with different -D flags (see run.sh). They implement the
 * metadata functions only — an SDK must reject them before calling anything else. */
#define ACME_BUILDING_CORE
#define ACME_PREFIX acme_client
#include "../../abi/acme_abi_functions.h" /* prototypes under the package prefix */

#ifndef FAKE_ABI_VERSION
#define FAKE_ABI_VERSION ACME_ABI_VERSION
#endif
#ifndef FAKE_CONTRACT
#define FAKE_CONTRACT "acme:client@1.0.0"
#endif
#ifndef FAKE_CAPS
#define FAKE_CAPS (ACME_CAP_SYNC_CALLS | ACME_CAP_ASYNC_CALLS | ACME_CAP_STREAMS | ACME_CAP_CANCELLATION | ACME_CAP_PAYLOAD_MSGPACK_1)
#endif

uint32_t ACME_FN(abi_version)(void) { return FAKE_ABI_VERSION; }
uint64_t ACME_FN(abi_capabilities)(void) { return FAKE_CAPS; }
const char *ACME_FN(contract_id)(void) { return FAKE_CONTRACT; }
const char *ACME_FN(core_info)(void) { return "fake-core (test fixture)"; }

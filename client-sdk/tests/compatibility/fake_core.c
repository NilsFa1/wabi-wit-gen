/* Deliberately incompatible "cores" for the version-mismatch tests.
 * Built three times with different -D flags (see run.sh). They implement the
 * metadata functions only — an SDK must reject them before calling anything else. */
#define WABI_BUILDING_CORE
#define WABI_PREFIX wabi_client
#include "../../abi/wabi_abi_functions.h" /* prototypes under the package prefix */

#ifndef FAKE_ABI_VERSION
#define FAKE_ABI_VERSION WABI_ABI_VERSION
#endif
#ifndef FAKE_CONTRACT
#define FAKE_CONTRACT "wabi:client@1.0.0"
#endif
#ifndef FAKE_CAPS
#define FAKE_CAPS (WABI_CAP_SYNC_CALLS | WABI_CAP_ASYNC_CALLS | WABI_CAP_STREAMS | WABI_CAP_CANCELLATION | WABI_CAP_PAYLOAD_MSGPACK_1)
#endif

uint32_t WABI_FN(abi_version)(void) { return FAKE_ABI_VERSION; }
uint64_t WABI_FN(abi_capabilities)(void) { return FAKE_CAPS; }
const char *WABI_FN(contract_id)(void) { return FAKE_CONTRACT; }
const char *WABI_FN(core_info)(void) { return "fake-core (test fixture)"; }

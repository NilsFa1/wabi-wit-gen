// Hand-written, generic: identity/version of a loaded core and the compatibility rule.
package com.acme.client.runtime;

import java.nio.file.Path;

public record CoreInfo(int abiVersion, long capabilities, String contractId, String description) {
    /** Required capabilities; CANCELLATION / THREADED_CALLBACK are informative (cancel is always callable). */
    public static final long REQUIRED = AbiConstants.ACME_CAP_SYNC_CALLS | AbiConstants.ACME_CAP_ASYNC_CALLS
            | AbiConstants.ACME_CAP_STREAMS | AbiConstants.ACME_CAP_PAYLOAD_MSGPACK_1;

    public int abiMajor() {
        return abiVersion >>> 16;
    }

    public int abiMinor() {
        return abiVersion & 0xffff;
    }

    @Override
    public String toString() {
        return description + " (abi " + abiMajor() + "." + abiMinor() + ", " + contractId + ")";
    }

    /** ABI: same major, core minor >= SDK minor. Contract: same package + major, core minor >= SDK minor. */
    public void ensureCompatible() {
        if (abiMajor() != AbiConstants.ABI_MAJOR || abiMinor() < AbiConstants.ABI_MINOR) {
            throw new AcmeIncompatibleLibraryException("native core '" + description + "' implements ABI "
                    + abiMajor() + "." + abiMinor() + ", this SDK needs " + AbiConstants.ABI_MAJOR + "." + AbiConstants.ABI_MINOR + "+");
        }
        long required = REQUIRED | (AbiConstants.REQUIRES_HOST_CALLS ? AbiConstants.ACME_CAP_HOST_CALLS : 0);
        long missing = required & ~capabilities;
        if (missing != 0) {
            throw new AcmeIncompatibleLibraryException("native core '" + description
                    + "' lacks capabilities 0x" + Long.toHexString(missing));
        }
        int at = contractId.indexOf('@');
        String pkg = at < 0 ? contractId : contractId.substring(0, at);
        String[] ver = at < 0 ? new String[0] : contractId.substring(at + 1).split("\\.");
        boolean ok = pkg.equals(Contract.PACKAGE) && ver.length >= 2;
        try {
            ok = ok && Integer.parseInt(ver[0]) == Contract.MAJOR && Integer.parseInt(ver[1]) >= Contract.MINOR;
        } catch (NumberFormatException e) {
            ok = false;
        }
        if (!ok) {
            throw new AcmeIncompatibleLibraryException("native core implements contract '" + contractId
                    + "', this SDK was generated for '" + Contract.ID + "'");
        }
    }

    /** Reads the metadata of an arbitrary core library without binding the SDK to it. */
    public static CoreInfo probe(Path library) {
        return NativeBackend.probe(library);
    }
}

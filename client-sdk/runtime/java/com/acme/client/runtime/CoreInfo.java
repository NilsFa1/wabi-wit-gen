// Hand-written, generic: identity/version of a loaded core and the compatibility rule.
package com.acme.client.runtime;

import java.lang.foreign.Arena;
import java.lang.foreign.SymbolLookup;
import java.nio.file.Path;

public record CoreInfo(int abiVersion, long capabilities, String contractId, String description) {
    /** Required capabilities; CANCELLATION / THREADED_CALLBACK are informative (cancel is always callable). */
    public static final long REQUIRED = NativeAbi.ACME_CAP_SYNC_CALLS | NativeAbi.ACME_CAP_ASYNC_CALLS
            | NativeAbi.ACME_CAP_STREAMS | NativeAbi.ACME_CAP_PAYLOAD_MSGPACK_1;

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
        if (abiMajor() != NativeAbi.ABI_MAJOR || abiMinor() < NativeAbi.ABI_MINOR) {
            throw new AcmeIncompatibleLibraryException("native core '" + description + "' implements ABI "
                    + abiMajor() + "." + abiMinor() + ", this SDK needs " + NativeAbi.ABI_MAJOR + "." + NativeAbi.ABI_MINOR + "+");
        }
        long missing = REQUIRED & ~capabilities;
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

    static CoreInfo read(NativeAbi abi) {
        return new CoreInfo(abi.acme_abi_version(), abi.acme_abi_capabilities(),
                NativeBackend.cString(abi.acme_contract_id()), NativeBackend.cString(abi.acme_core_info()));
    }

    /** Reads the metadata of an arbitrary core library without binding the SDK to it. */
    public static CoreInfo probe(Path library) {
        try (var arena = Arena.ofConfined()) {
            var lookup = SymbolLookup.libraryLookup(library, arena);
            var linker = java.lang.foreign.Linker.nativeLinker();
            try {
                int version = (int) linker.downcallHandle(lookup.find("acme_abi_version").orElseThrow(),
                        java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_INT)).invokeExact();
                long caps = (long) linker.downcallHandle(lookup.find("acme_abi_capabilities").orElseThrow(),
                        java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.JAVA_LONG)).invokeExact();
                var str = java.lang.foreign.FunctionDescriptor.of(java.lang.foreign.ValueLayout.ADDRESS);
                var contract = (java.lang.foreign.MemorySegment) linker.downcallHandle(lookup.find("acme_contract_id").orElseThrow(), str).invokeExact();
                var info = (java.lang.foreign.MemorySegment) linker.downcallHandle(lookup.find("acme_core_info").orElseThrow(), str).invokeExact();
                return new CoreInfo(version, caps, NativeBackend.cString(contract), NativeBackend.cString(info));
            } catch (Throwable t) {
                throw new AcmeIncompatibleLibraryException("cannot probe " + library + ": " + t);
            }
        }
    }
}

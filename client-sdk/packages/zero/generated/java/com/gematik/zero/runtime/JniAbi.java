// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: AbiCalls via JNI, for Android (no Panama) and JDK 17+.
//
// The C side is generated/java/jni/gematik_zero_jni.c of this package: a small,
// contract-agnostic shim (libgematik_zero_jni, named after the package so two SDKs can
// share a process) that dlopen()s the core and forwards the 17 ABI functions.
// Native memory never reaches Java: the shim copies results into byte[] and
// frees the core's buffers itself. New WIT methods need no change here.
package com.gematik.zero.runtime;

final class JniAbi implements AbiCalls {
    /** System property with an explicit path to libgematik_zero_jni (tests); default: System.loadLibrary("gematik_zero_jni"). */
    public static final String SHIM_PROPERTY = "com.gematik.zero.runtime.jniLib";

    public static final class Loader implements AbiCalls.Loader {
        public Loader() {
            loadShim();
        }

        @Override
        public AbiCalls open(String path) {
            if (path == null) {
                // Android: make sure the core from jniLibs is loaded into the app's
                // linker namespace, then let the shim dlopen it by name.
                try {
                    System.loadLibrary(AbiConstants.LIBRARY);
                } catch (UnsatisfiedLinkError ignored) {
                    // desktop: resolved by the dynamic linker search path instead
                }
            }
            return new JniAbi(openLib(path, false));
        }

        @Override
        public CoreInfo probe(String path) {
            long lib = openLib(path, true);
            try {
                return new CoreInfo(nAbiVersion(lib), nCapabilities(lib), nContractId(lib), nCoreInfo(lib));
            } finally {
                nClose(lib);
            }
        }

        private static long openLib(String path, boolean metadataOnly) {
            var error = new String[1];
            long lib = nOpen(path != null ? path : System.mapLibraryName(AbiConstants.LIBRARY),
                    AbiConstants.SYMBOL_PREFIX, metadataOnly, error);
            if (lib == 0) throw new WabiIncompatibleLibraryException(error[0]);
            return lib;
        }
    }

    private static volatile boolean shimLoaded;

    private static synchronized void loadShim() {
        if (shimLoaded) return;
        String explicit = System.getProperty(SHIM_PROPERTY);
        try {
            if (explicit != null && !explicit.isEmpty()) System.load(explicit);
            else System.loadLibrary("gematik_zero_jni");
        } catch (UnsatisfiedLinkError e) {
            throw new WabiIncompatibleLibraryException("cannot load the JNI shim libgematik_zero_jni: " + e.getMessage());
        }
        shimLoaded = true;
    }

    /** Called by the shim from a core thread (attached as daemon); must not block. */
    @SuppressWarnings("unused")
    private static void onReady(long key) {
        try {
            NativeBackend.onReady(key);
        } catch (Throwable ignored) {
            // the shim clears pending exceptions anyway
        }
    }

    /** Host call (ABI 1.1), called by the shim from a core thread with a copy of the request. */
    @SuppressWarnings("unused")
    private static void onHostCall(int methodId, byte[] request, long call) {
        try {
            HostCalls.onHostCall(methodId, request, call);
        } catch (Throwable ignored) {
            // the shim clears pending exceptions anyway
        }
    }

    private final long lib;
    private final CoreInfo info;

    private JniAbi(long lib) {
        this.lib = lib;
        this.info = new CoreInfo(nAbiVersion(lib), nCapabilities(lib), nContractId(lib), nCoreInfo(lib));
    }

    @Override
    public CoreInfo info() {
        return info;
    }

    @Override
    public String statusName(int status) {
        return nStatusName(lib, status);
    }

    @Override
    public Taken callSync(long object, int methodId, byte[] request) {
        int[] status = new int[1];
        byte[] bytes = nCallSync(lib, object, methodId, request, status);
        return new Taken(status[0], bytes);
    }

    @Override
    public Started callAsync(long object, int methodId, byte[] request) {
        int[] status = new int[1];
        long future = nCallAsync(lib, object, methodId, request, status);
        return new Started(status[0], future);
    }

    @Override
    public int objectDrop(long object) {
        return nObjectDrop(lib, object);
    }

    @Override
    public int subscribe(long future, long key) {
        return nSubscribe(lib, future, key);
    }

    @Override
    public Taken take(long future) {
        int[] status = new int[1];
        byte[] bytes = nTake(lib, future, status);
        return new Taken(status[0], bytes);
    }

    @Override
    public int cancel(long future) {
        return nCancel(lib, future);
    }

    @Override
    public int futureDrop(long future) {
        return nFutureDrop(lib, future);
    }

    @Override
    public Started streamNext(long stream) {
        int[] status = new int[1];
        long future = nStreamNext(lib, stream, status);
        return new Started(status[0], future);
    }

    @Override
    public int streamCancel(long stream) {
        return nStreamCancel(lib, stream);
    }

    @Override
    public int streamDrop(long stream) {
        return nStreamDrop(lib, stream);
    }

    @Override
    public int hostRegister(boolean install) {
        return nHostRegister(lib, install);
    }

    @Override
    public int hostReply(long call, int status, byte[] response) {
        return nHostReply(lib, call, status, response);
    }

    // ----------------------------------------------------------- natives
    // `lib` is an opaque pointer to the shim's function table for one core.

    /** {@code symbolPrefix}: e.g. "wabi_client_" — the shim itself is package-agnostic. */
    static native long nOpen(String path, String symbolPrefix, boolean metadataOnly, String[] error);
    static native void nClose(long lib);
    static native int nAbiVersion(long lib);
    static native long nCapabilities(long lib);
    static native String nContractId(long lib);
    static native String nCoreInfo(long lib);
    static native String nStatusName(long lib, int status);
    static native byte[] nCallSync(long lib, long object, int methodId, byte[] request, int[] status);
    static native long nCallAsync(long lib, long object, int methodId, byte[] request, int[] status);
    static native int nObjectDrop(long lib, long object);
    static native int nSubscribe(long lib, long future, long key);
    static native byte[] nTake(long lib, long future, int[] status);
    static native int nCancel(long lib, long future);
    static native int nFutureDrop(long lib, long future);
    static native long nStreamNext(long lib, long stream, int[] status);
    static native int nStreamCancel(long lib, long stream);
    static native int nStreamDrop(long lib, long stream);
    /** ABI 1.1 (optional symbols): WABI_ERR_UNSUPPORTED if the core has no host calls. */
    static native int nHostRegister(long lib, boolean install);
    static native int nHostReply(long lib, long call, int status, byte[] response);
}

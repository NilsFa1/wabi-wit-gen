// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: the 17 ABI functions as seen from Java, independent of
// HOW they are called. Two implementations exist:
//   PanamaAbi  — java.lang.foreign (JDK 22+, desktop/server)
//   JniAbi     — a tiny generic C shim (Android, JDK 17+)
// Everything above this interface (futures, publishers, errors) is shared.
package com.demo.client.runtime;

interface AbiCalls {
    /** Status + owned bytes (already copied onto the Java heap, native buffer freed). */
    record Taken(int status, byte[] bytes) {}

    /** Status + new handle (0 unless status == OK). */
    record Started(int status, long handle) {}

    /** Creates an AbiCalls for a core library; implementations need a public no-arg constructor. */
    interface Loader {
        /** {@code path == null}: the platform default ("demo_client"). */
        AbiCalls open(String path);

        /** Reads only the four metadata symbols (works on incompatible libraries). */
        CoreInfo probe(String path);
    }

    CoreInfo info();

    String statusName(int status);

    Taken callSync(long object, int methodId, byte[] request);

    Started callAsync(long object, int methodId, byte[] request);

    int objectDrop(long object);

    /** Subscribes; on completion the implementation calls {@link NativeBackend#onReady(long)} with {@code key}. */
    int subscribe(long future, long key);

    Taken take(long future);

    int cancel(long future);

    int futureDrop(long future);

    Started streamNext(long stream);

    int streamCancel(long stream);

    int streamDrop(long stream);

    /**
     * ABI 1.2 (host_register2): installs (or removes) the binding's native host
     * handlers, which forward every host call to {@link HostCalls#onHostCall} and
     * every abandoned one to {@link HostCalls#onHostCancel}. Returns
     * WABI_ERR_UNSUPPORTED if the core lacks the host-call symbols.
     */
    int hostRegister(boolean install);

    /** ABI 1.1: answers a host call (exactly once per call). */
    int hostReply(long call, int status, byte[] response);
}

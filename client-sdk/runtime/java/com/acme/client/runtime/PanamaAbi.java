// Hand-written, generic: AbiCalls via Panama FFM (JDK 22+). Not shipped in the
// Android jar. Loaded reflectively by NativeBackend, so nothing else in the
// runtime references java.lang.foreign.
package com.acme.client.runtime;

import static java.lang.foreign.ValueLayout.ADDRESS;
import static java.lang.foreign.ValueLayout.JAVA_BYTE;
import static java.lang.foreign.ValueLayout.JAVA_INT;
import static java.lang.foreign.ValueLayout.JAVA_LONG;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;

final class PanamaAbi implements AbiCalls {
    public static final class Loader implements AbiCalls.Loader {
        public Loader() {}

        @Override
        public AbiCalls open(String path) {
            return new PanamaAbi(lookup(path));
        }

        @Override
        public CoreInfo probe(String path) {
            var lookup = lookup(path);
            var linker = Linker.nativeLinker();
            try {
                String p = AbiConstants.SYMBOL_PREFIX; // acme_client_abi_version, ... (ABI.md §1.1)
                int version = (int) linker.downcallHandle(find(lookup, p + "abi_version"), FunctionDescriptor.of(JAVA_INT)).invokeExact();
                long caps = (long) linker.downcallHandle(find(lookup, p + "abi_capabilities"), FunctionDescriptor.of(JAVA_LONG)).invokeExact();
                var str = FunctionDescriptor.of(ADDRESS);
                var contract = (MemorySegment) linker.downcallHandle(find(lookup, p + "contract_id"), str).invokeExact();
                var info = (MemorySegment) linker.downcallHandle(find(lookup, p + "core_info"), str).invokeExact();
                return new CoreInfo(version, caps, cString(contract), cString(info));
            } catch (AcmeException e) {
                throw e;
            } catch (Throwable t) {
                throw new AcmeIncompatibleLibraryException("cannot probe " + path + ": " + t);
            }
        }

        private static MemorySegment find(SymbolLookup l, String name) {
            return l.find(name).orElseThrow(() -> new AcmeIncompatibleLibraryException("native core lacks symbol " + name));
        }

        private static SymbolLookup lookup(String path) {
            try {
                return path != null
                        ? SymbolLookup.libraryLookup(Path.of(path), Arena.global())
                        : SymbolLookup.libraryLookup(System.mapLibraryName(AbiConstants.LIBRARY), Arena.global());
            } catch (IllegalArgumentException e) {
                throw new AcmeIncompatibleLibraryException("cannot load native core " + path + ": " + e.getMessage());
            }
        }
    }

    private static final MemorySegment ON_READY;

    static {
        try {
            var target = MethodHandles.lookup().findStatic(PanamaAbi.class, "onReady",
                    MethodType.methodType(void.class, MemorySegment.class, long.class));
            ON_READY = Linker.nativeLinker().upcallStub(target, NativeAbi.CALLBACK, Arena.global());
        } catch (ReflectiveOperationException e) {
            throw new ExceptionInInitializerError(e);
        }
    }

    /** Upcall from a core thread; must not block or throw. */
    @SuppressWarnings("unused")
    private static void onReady(MemorySegment userData, long future) {
        try {
            NativeBackend.onReady(userData.address());
        } catch (Throwable ignored) {
            // never unwind into the core
        }
    }

    private final NativeAbi abi;
    private final CoreInfo info;

    private PanamaAbi(SymbolLookup lookup) {
        this.abi = new NativeAbi(lookup);
        this.info = new CoreInfo(abi.acme_abi_version(), abi.acme_abi_capabilities(),
                cString(abi.acme_contract_id()), cString(abi.acme_core_info()));
    }

    static String cString(MemorySegment p) {
        return p.equals(MemorySegment.NULL) ? "" : p.reinterpret(Long.MAX_VALUE).getString(0, StandardCharsets.UTF_8);
    }

    private static MemorySegment bytes(Arena arena, byte[] data) {
        var s = arena.allocate(NativeAbi.ACME_BYTES);
        if (data.length > 0) {
            var d = arena.allocate(data.length);
            MemorySegment.copy(data, 0, d, JAVA_BYTE, 0, data.length);
            s.set(ADDRESS, 0, d);
        } else {
            s.set(ADDRESS, 0, MemorySegment.NULL);
        }
        s.set(JAVA_LONG, 8, data.length);
        return s;
    }

    /** Copies a core-owned buffer onto the Java heap and frees it (always). */
    private byte[] consume(MemorySegment buffer) {
        try {
            long len = buffer.get(JAVA_LONG, 8);
            return len == 0 ? new byte[0] : buffer.get(ADDRESS, 0).reinterpret(len).toArray(JAVA_BYTE);
        } finally {
            abi.acme_buffer_free(buffer);
        }
    }

    @Override
    public CoreInfo info() {
        return info;
    }

    @Override
    public String statusName(int status) {
        return cString(abi.acme_status_name(status));
    }

    @Override
    public Taken callSync(long object, int methodId, byte[] request) {
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(NativeAbi.ACME_BUFFER);
            int status = abi.acme_call_sync(object, methodId, bytes(arena, request), out);
            return new Taken(status, consume(out));
        }
    }

    @Override
    public Started callAsync(long object, int methodId, byte[] request) {
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(JAVA_LONG);
            int status = abi.acme_call_async(object, methodId, bytes(arena, request), out);
            return new Started(status, out.get(JAVA_LONG, 0));
        }
    }

    @Override
    public int objectDrop(long object) {
        return abi.acme_object_drop(object);
    }

    @Override
    public int subscribe(long future, long key) {
        return abi.acme_future_subscribe(future, ON_READY, MemorySegment.ofAddress(key));
    }

    @Override
    public Taken take(long future) {
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(NativeAbi.ACME_BUFFER);
            int status = abi.acme_future_take(future, out);
            return new Taken(status, consume(out));
        }
    }

    @Override
    public int cancel(long future) {
        return abi.acme_future_cancel(future);
    }

    @Override
    public int futureDrop(long future) {
        return abi.acme_future_drop(future);
    }

    @Override
    public Started streamNext(long stream) {
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(JAVA_LONG);
            int status = abi.acme_stream_next(stream, out);
            return new Started(status, out.get(JAVA_LONG, 0));
        }
    }

    @Override
    public int streamCancel(long stream) {
        return abi.acme_stream_cancel(stream);
    }

    @Override
    public int streamDrop(long stream) {
        return abi.acme_stream_drop(stream);
    }
}

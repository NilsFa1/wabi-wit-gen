// Hand-written, generic: executes contract calls through the Acme native ABI
// using Panama FFM (Java 22+). No per-method code lives here.
package com.acme.client.runtime;

import static java.lang.foreign.ValueLayout.ADDRESS;
import static java.lang.foreign.ValueLayout.JAVA_BYTE;
import static java.lang.foreign.ValueLayout.JAVA_LONG;

import java.lang.foreign.Arena;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.invoke.MethodHandles;
import java.lang.invoke.MethodType;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.concurrent.CancellationException;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.Executor;
import java.util.concurrent.Flow;
import java.util.concurrent.ForkJoinPool;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.LongSupplier;

public final class NativeBackend implements ClientBackend {
    /** System property / environment variable with an explicit path to the core library. */
    public static final String LIBRARY_PROPERTY = "acme.native.lib";
    public static final String LIBRARY_ENV = "ACME_NATIVE_LIB";

    /**
     * Where completions are delivered. The native callback only hands off to this
     * executor and returns immediately; no Android/UI main thread is ever assumed —
     * apps dispatch to their UI thread themselves (e.g. thenAcceptAsync(fn, uiExecutor)).
     */
    static final Executor EXECUTOR = ForkJoinPool.commonPool();

    private static volatile NativeBackend shared;

    /** Pending subscriptions: user_data carries a key into this map (no Java pointers cross the ABI). */
    private static final ConcurrentHashMap<Long, CompletableFuture<Void>> WAITERS = new ConcurrentHashMap<>();
    private static final AtomicLong NEXT_KEY = new AtomicLong();
    private static final MemorySegment ON_READY;

    static {
        try {
            var target = MethodHandles.lookup().findStatic(NativeBackend.class, "onReady",
                    MethodType.methodType(void.class, MemorySegment.class, long.class));
            ON_READY = Linker.nativeLinker().upcallStub(target, NativeAbi.CALLBACK, Arena.global());
        } catch (ReflectiveOperationException e) {
            throw new ExceptionInInitializerError(e);
        }
    }

    private final NativeAbi abi;
    private final CoreInfo info;

    private NativeBackend(SymbolLookup lookup) {
        this.abi = new NativeAbi(lookup);
        this.info = CoreInfo.read(abi);
        info.ensureCompatible();
    }

    /** The process-wide backend; loads and validates the library on first use. */
    public static NativeBackend shared() {
        var s = shared;
        if (s == null) {
            synchronized (NativeBackend.class) {
                s = shared;
                if (s == null) shared = s = new NativeBackend(defaultLookup());
            }
        }
        return s;
    }

    /** A backend bound to an explicit library (e.g. to run two cores side by side). */
    public static NativeBackend load(Path library) {
        return new NativeBackend(SymbolLookup.libraryLookup(library, Arena.global()));
    }

    private static SymbolLookup defaultLookup() {
        String path = System.getProperty(LIBRARY_PROPERTY, System.getenv(LIBRARY_ENV));
        if (path != null && !path.isEmpty()) return SymbolLookup.libraryLookup(Path.of(path), Arena.global());
        return SymbolLookup.libraryLookup(System.mapLibraryName(NativeAbi.LIBRARY), Arena.global());
    }

    public CoreInfo info() {
        return info;
    }

    NativeAbi abi() {
        return abi;
    }

    @Override
    public String description() {
        return "native: " + info;
    }

    // ------------------------------------------------------------ buffers

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
            if (len == 0) return new byte[0];
            return buffer.get(ADDRESS, 0).reinterpret(len).toArray(JAVA_BYTE);
        } finally {
            abi.acme_buffer_free(buffer);
        }
    }

    AcmeNativeException error(int status, byte[] detail) {
        return new AcmeNativeException(status, cString(abi.acme_status_name(status)),
                detail == null || detail.length == 0 ? null : new String(detail, StandardCharsets.UTF_8));
    }

    // -------------------------------------------------------------- calls

    @Override
    public byte[] callSync(long object, int methodId, byte[] request) {
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(NativeAbi.ACME_BUFFER);
            int status = abi.acme_call_sync(object, methodId, bytes(arena, request), out);
            byte[] result = consume(out);
            if (status != NativeAbi.ACME_OK) throw error(status, result);
            return result;
        }
    }

    @Override
    public <T> CompletableFuture<T> callAsync(long object, int methodId, byte[] request, Decoder<T> decoder) {
        long future;
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(JAVA_LONG);
            int status = abi.acme_call_async(object, methodId, bytes(arena, request), out);
            if (status != NativeAbi.ACME_OK) return CompletableFuture.failedFuture(error(status, null));
            future = out.get(JAVA_LONG, 0);
        }
        var result = new NativeFuture<T>(abi, future);
        whenReady(future).whenComplete((ignored, failure) -> {
            try {
                if (failure != null) {
                    result.completeExceptionally(failure);
                    return;
                }
                var taken = take(future);
                switch (taken.status()) {
                    case NativeAbi.ACME_OK -> result.complete(decoder.decode(taken.bytes()));
                    case NativeAbi.ACME_ERR_CANCELLED -> result.completeExceptionally(new CancellationException("cancelled by the core"));
                    default -> result.completeExceptionally(error(taken.status(), taken.bytes()));
                }
            } catch (Throwable t) {
                result.completeExceptionally(t); // e.g. ClientException thrown by the decoder
            } finally {
                abi.acme_future_drop(future);
            }
        });
        return result;
    }

    record Taken(int status, byte[] bytes) {}

    Taken take(long future) {
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(NativeAbi.ACME_BUFFER);
            int status = abi.acme_future_take(future, out);
            return new Taken(status, consume(out));
        }
    }

    // ------------------------------------------------------------ futures

    /** Completes (on {@link #EXECUTOR}) once the native future is ready. */
    CompletableFuture<Void> whenReady(long future) {
        var ready = new CompletableFuture<Void>();
        long key = NEXT_KEY.incrementAndGet();
        WAITERS.put(key, ready);
        int status = abi.acme_future_subscribe(future, ON_READY, MemorySegment.ofAddress(key));
        if (status != NativeAbi.ACME_OK) {
            WAITERS.remove(key); // the callback will never run
            if (status == NativeAbi.ACME_READY) ready.complete(null);
            else ready.completeExceptionally(error(status, null));
        }
        return ready;
    }

    /**
     * Upcall from a core thread, exactly once per successful subscribe. Must not block
     * and must never throw into native code.
     */
    @SuppressWarnings("unused")
    private static void onReady(MemorySegment userData, long future) {
        try {
            var ready = WAITERS.remove(userData.address());
            if (ready != null) EXECUTOR.execute(() -> ready.complete(null));
        } catch (Throwable ignored) {
            // never unwind into the core
        }
    }

    /** A CompletableFuture whose cancel() also cancels the native operation. */
    static final class NativeFuture<T> extends CompletableFuture<T> {
        private final NativeAbi abi;
        private final long handle;

        NativeFuture(NativeAbi abi, long handle) {
            this.abi = abi;
            this.handle = handle;
        }

        @Override
        public boolean cancel(boolean mayInterruptIfRunning) {
            boolean cancelled = super.cancel(mayInterruptIfRunning);
            // Harmless if the native future already completed or was dropped:
            // handles are never reused, so this returns ACME_ERR_INVALID_HANDLE at worst.
            abi.acme_future_cancel(handle);
            return cancelled;
        }
    }

    // ------------------------------------------------------------ streams

    @Override
    public <T> Flow.Publisher<T> publisher(LongSupplier open, Decoder<T> decoder) {
        return new StreamPublisher<>(this, open, decoder);
    }

    long streamNext(long stream) {
        try (var arena = Arena.ofConfined()) {
            var out = arena.allocate(JAVA_LONG);
            int status = abi.acme_stream_next(stream, out);
            if (status != NativeAbi.ACME_OK) throw error(status, null);
            return out.get(JAVA_LONG, 0);
        }
    }

    // ------------------------------------------------------------ objects

    @Override
    public void dropObject(long object) {
        abi.acme_object_drop(object);
    }
}

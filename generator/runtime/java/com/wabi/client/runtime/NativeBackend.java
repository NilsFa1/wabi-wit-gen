// Hand-written, generic: executes contract calls through the Wabi native ABI.
// How the 17 ABI functions are invoked (Panama FFM or JNI) is hidden behind
// AbiCalls; futures, cancellation, publishers and errors live here once.
package com.wabi.client.runtime;

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
    public static final String LIBRARY_PROPERTY = "wabi.native.lib";
    public static final String LIBRARY_ENV = "WABI_NATIVE_LIB";
    /** System property selecting the binding: {@code panama} or {@code jni} (default: panama where available). */
    public static final String BINDING_PROPERTY = "wabi.binding";

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

    private final AbiCalls abi;
    private final String binding;

    private NativeBackend(AbiCalls abi, String binding) {
        this.abi = abi;
        this.binding = binding;
        abi.info().ensureCompatible();
    }

    /** The process-wide backend; loads and validates the library on first use. */
    public static NativeBackend shared() {
        var s = shared;
        if (s == null) {
            synchronized (NativeBackend.class) {
                s = shared;
                if (s == null) {
                    String path = System.getProperty(LIBRARY_PROPERTY, System.getenv(LIBRARY_ENV));
                    shared = s = open(defaultBinding(), path == null || path.isEmpty() ? null : path);
                }
            }
        }
        return s;
    }

    /** A backend bound to an explicit library (e.g. to run two cores side by side). */
    public static NativeBackend load(Path library) {
        return open(defaultBinding(), library.toString());
    }

    /** Like {@link #load(Path)} with an explicit binding ("panama" or "jni"). */
    public static NativeBackend load(Path library, String binding) {
        return open(binding, library.toString());
    }

    static CoreInfo probe(Path library) {
        return loader(defaultBinding()).probe(library.toString());
    }

    private static NativeBackend open(String binding, String path) {
        return new NativeBackend(loader(binding).open(path), binding);
    }

    /** Panama on JDK 22+ desktop/server JVMs; JNI on Android (ART) and older JVMs. */
    static String defaultBinding() {
        String explicit = System.getProperty(BINDING_PROPERTY);
        if (explicit != null && !explicit.isEmpty()) return explicit;
        boolean android = "Dalvik".equals(System.getProperty("java.vm.name"));
        if (!android && Runtime.version().feature() >= 22) {
            try {
                Class.forName("java.lang.foreign.Linker");
                return "panama";
            } catch (ClassNotFoundException ignored) {
                // fall through
            }
        }
        return "jni";
    }

    /** Loaded reflectively so that the JNI path never touches java.lang.foreign classes. */
    private static AbiCalls.Loader loader(String binding) {
        String cls = switch (binding) {
            case "panama" -> "com.wabi.client.runtime.PanamaAbi$Loader";
            case "jni" -> "com.wabi.client.runtime.JniAbi$Loader";
            default -> throw new IllegalArgumentException("unknown binding '" + binding + "' (panama|jni)");
        };
        try {
            return (AbiCalls.Loader) Class.forName(cls).getDeclaredConstructor().newInstance();
        } catch (ReflectiveOperationException e) {
            Throwable cause = e.getCause() != null ? e.getCause() : e;
            if (cause instanceof WabiException ae) throw ae;
            throw new WabiIncompatibleLibraryException("binding '" + binding + "' is not available: " + cause);
        }
    }

    public CoreInfo info() {
        return abi.info();
    }

    /** "panama" or "jni". */
    public String binding() {
        return binding;
    }

    AbiCalls abi() {
        return abi;
    }

    @Override
    public String description() {
        return "native[" + binding + "]: " + abi.info();
    }

    WabiNativeException error(int status, byte[] detail) {
        return new WabiNativeException(status, abi.statusName(status),
                detail == null || detail.length == 0 ? null : new String(detail, StandardCharsets.UTF_8));
    }

    // -------------------------------------------------------------- calls

    @Override
    public byte[] callSync(long object, int methodId, byte[] request) {
        var r = abi.callSync(object, methodId, request);
        if (r.status() != AbiConstants.WABI_OK) throw error(r.status(), r.bytes());
        return r.bytes();
    }

    @Override
    public <T> CompletableFuture<T> callAsync(long object, int methodId, byte[] request, Decoder<T> decoder) {
        var started = abi.callAsync(object, methodId, request);
        if (started.status() != AbiConstants.WABI_OK) return CompletableFuture.failedFuture(error(started.status(), null));
        long future = started.handle();
        var result = new NativeFuture<T>(abi, future);
        whenReady(future).whenComplete((ignored, failure) -> {
            try {
                if (failure != null) {
                    result.completeExceptionally(failure);
                    return;
                }
                var taken = abi.take(future);
                switch (taken.status()) {
                    case AbiConstants.WABI_OK -> result.complete(decoder.decode(taken.bytes()));
                    case AbiConstants.WABI_ERR_CANCELLED -> result.completeExceptionally(new CancellationException("cancelled by the core"));
                    default -> result.completeExceptionally(error(taken.status(), taken.bytes()));
                }
            } catch (Throwable t) {
                result.completeExceptionally(t); // a generated contract exception from the decoder
            } finally {
                abi.futureDrop(future);
            }
        });
        return result;
    }

    // ------------------------------------------------------------ futures

    /** Completes (on {@link #EXECUTOR}) once the native future is ready. */
    CompletableFuture<Void> whenReady(long future) {
        var ready = new CompletableFuture<Void>();
        // 31-bit keys: user_data is a void*, which is 32 bits on armeabi-v7a/x86 Android.
        // Keys are unique among *pending* waiters, which is all that matters; 0 is skipped.
        long key;
        do {
            key = NEXT_KEY.incrementAndGet() & 0x7fffffffL;
        } while (key == 0);
        WAITERS.put(key, ready);
        int status = abi.subscribe(future, key);
        if (status != AbiConstants.WABI_OK) {
            WAITERS.remove(key); // the callback will never run
            if (status == AbiConstants.WABI_READY) ready.complete(null);
            else ready.completeExceptionally(error(status, null));
        }
        return ready;
    }

    /** Called from a core thread (via Panama upcall or JNI), exactly once per successful subscribe. */
    static void onReady(long key) {
        var ready = WAITERS.remove(key);
        if (ready != null) EXECUTOR.execute(() -> ready.complete(null));
    }

    /** A CompletableFuture whose cancel() also cancels the native operation. */
    static final class NativeFuture<T> extends CompletableFuture<T> {
        private final AbiCalls abi;
        private final long handle;

        NativeFuture(AbiCalls abi, long handle) {
            this.abi = abi;
            this.handle = handle;
        }

        @Override
        public boolean cancel(boolean mayInterruptIfRunning) {
            boolean cancelled = super.cancel(mayInterruptIfRunning);
            // Harmless if the native future already completed or was dropped:
            // handles are never reused, so this returns WABI_ERR_INVALID_HANDLE at worst.
            abi.cancel(handle);
            return cancelled;
        }
    }

    // ------------------------------------------------------------ streams

    @Override
    public <T> Flow.Publisher<T> publisher(LongSupplier open, Decoder<T> decoder) {
        return new StreamPublisher<>(this, open, decoder);
    }

    long streamNext(long stream) {
        var started = abi.streamNext(stream);
        if (started.status() != AbiConstants.WABI_OK) throw error(started.status(), null);
        return started.handle();
    }

    // ------------------------------------------------------------ objects

    @Override
    public void dropObject(long object) {
        HostCalls.uninstall(this, object); // an implementation registered for this object goes with it
        abi.objectDrop(object);
    }
}

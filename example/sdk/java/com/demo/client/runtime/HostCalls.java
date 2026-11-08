// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: host calls (ABI 1.2, ABI.md §4a). The core calls app
// code for the functions of WIT *imports*; the generated <Package>Host class
// decodes the request and calls the app's interface implementation.
//
// An implementation is registered for the whole package or for one instance
// (object); the core names the instance with every call. If the core gives up
// on a call, the stage the app returned is cancelled.
package com.demo.client.runtime;

import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.concurrent.CancellationException;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutionException;

public final class HostCalls {
    private HostCalls() {}

    /** Generated per package: method id + encoded request -> encoded answer. */
    @FunctionalInterface
    public interface Dispatch {
        CompletionStage<byte[]> call(int methodId, byte[] request) throws Exception;
    }

    /** A call the app is working on. */
    private static final class Running {
        volatile CompletionStage<?> stage; // what the app returned
        volatile boolean cancelled;

        void cancel() {
            cancelled = true;
            var s = stage;
            if (s == null) return;
            try {
                s.toCompletableFuture().cancel(true);
            } catch (UnsupportedOperationException ignored) {
                // a stage that cannot be cancelled simply runs to its end
            }
        }
    }

    private static final Object GATE = new Object();
    private static volatile Dispatch dispatch;
    private static final Map<Long, Dispatch> PER_OBJECT = new ConcurrentHashMap<>();
    private static final Map<Long, Running> RUNNING = new ConcurrentHashMap<>();
    private static final ThreadLocal<Running> CURRENT = new ThreadLocal<>();
    private static volatile AbiCalls abi;
    private static boolean registered;

    /** Installs the dispatcher for all imports of this package (one per process; replaces a previous one). */
    public static void install(NativeBackend backend, Dispatch d) {
        if (d == null) throw new NullPointerException("dispatch");
        synchronized (GATE) {
            dispatch = d;
            sync(backend);
        }
    }

    /**
     * Installs the dispatcher for the calls the core makes on behalf of one
     * object; it takes precedence over the package-wide one and goes away with
     * the object.
     */
    public static void install(NativeBackend backend, long object, Dispatch d) {
        if (d == null) throw new NullPointerException("dispatch");
        synchronized (GATE) {
            PER_OBJECT.put(object, d);
            sync(backend);
        }
    }

    /** Removes the package-wide dispatcher; calls already running finish with it. */
    public static void uninstall(NativeBackend backend) {
        synchronized (GATE) {
            dispatch = null;
            sync(backend);
        }
    }

    /** Removes the dispatcher of one object (no-op if it has none). */
    public static void uninstall(NativeBackend backend, long object) {
        if (PER_OBJECT.isEmpty()) return; // the common case when objects are dropped
        synchronized (GATE) {
            if (PER_OBJECT.remove(object) != null) sync(backend);
        }
    }

    /** Registers with the core while there is any dispatcher, unregisters when the last one is gone. Caller holds GATE. */
    private static void sync(NativeBackend backend) {
        boolean wanted = dispatch != null || !PER_OBJECT.isEmpty();
        if (wanted == registered) return;
        if (!wanted) {
            // Returns once no handler invocation is running; later host calls fail inside the core.
            backend.abi().hostRegister(false);
            registered = false;
            return;
        }
        var info = backend.info();
        int status = (info.capabilities() & AbiConstants.WABI_CAP_HOST_CALLS_2) == 0 ? AbiConstants.WABI_ERR_UNSUPPORTED : AbiConstants.WABI_OK;
        if (status == AbiConstants.WABI_OK) {
            abi = backend.abi();
            status = abi.hostRegister(true);
        }
        if (status != AbiConstants.WABI_OK) {
            dispatch = null;
            PER_OBJECT.clear();
            if (status == AbiConstants.WABI_ERR_UNSUPPORTED) {
                throw new WabiIncompatibleLibraryException("native core '" + info.description() + "' does not support host calls (ABI 1.2)");
            }
            throw backend.error(status, null);
        }
        registered = true;
    }

    /**
     * Called by the binding (Panama upcall / JNI shim) on a core thread with a
     * copy of the request. Must return quickly: the app code runs on the
     * executor, and the answer goes back with host_reply exactly once.
     */
    static void onHostCall(long object, int methodId, byte[] request, long call) {
        var target = abi;
        if (target == null) return; // not installed through install(): cannot reply
        // The implementation is chosen now: a call keeps it even if it is replaced meanwhile.
        var own = PER_OBJECT.get(object);
        var d = own != null ? own : dispatch;
        var running = new Running();
        RUNNING.put(call, running); // before the core can cancel the call
        NativeBackend.EXECUTOR.execute(() -> run(target, d, running, methodId, request, call));
    }

    /** Called by the binding on a core thread: the core no longer waits for the call. */
    static void onHostCancel(long call) {
        var running = RUNNING.get(call);
        // Cancelling completes the app's stage, which runs app code: never on the core's thread.
        if (running != null) NativeBackend.EXECUTOR.execute(running::cancel);
    }

    /**
     * Called by the generated dispatcher with the stage the app returned, so
     * that it can be cancelled when the core gives up on the call.
     */
    public static <T> CompletionStage<T> attach(CompletionStage<T> stage) {
        var running = CURRENT.get();
        if (running != null) {
            running.stage = stage;
            if (running.cancelled) running.cancel(); // the core was faster
        }
        return stage;
    }

    private static void run(AbiCalls target, Dispatch d, Running running, int methodId, byte[] request, long call) {
        CompletionStage<byte[]> stage;
        CURRENT.set(running);
        try {
            if (d == null) throw new WabiNativeException(AbiConstants.WABI_ERR_UNSUPPORTED, "no host handler", null);
            stage = d.call(methodId, request);
        } catch (Throwable t) {
            stage = CompletableFuture.failedFuture(t);
        } finally {
            CURRENT.remove();
        }
        stage.whenComplete((bytes, failure) -> {
            RUNNING.remove(call);
            if (failure == null) {
                target.hostReply(call, AbiConstants.WABI_OK, bytes == null ? new byte[0] : bytes);
                return;
            }
            Throwable cause = failure;
            while ((cause instanceof CompletionException || cause instanceof ExecutionException) && cause.getCause() != null) {
                cause = cause.getCause();
            }
            int status = cause instanceof WabiNativeException ne ? ne.status()
                    : cause instanceof CancellationException ? AbiConstants.WABI_ERR_CANCELLED
                    : AbiConstants.WABI_ERR_INTERNAL;
            String text = cause.getClass().getSimpleName() + ": " + cause.getMessage();
            target.hostReply(call, status, text.getBytes(StandardCharsets.UTF_8)); // an app bug: reported, never lost
        });
    }
}

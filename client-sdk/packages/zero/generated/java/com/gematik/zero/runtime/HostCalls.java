// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: ABI 1.1 host calls (ABI.md §4a). The core calls app
// code for the functions of WIT *imports*; the generated <Package>Host class
// decodes the request and calls the app's interface implementation.
package com.gematik.zero.runtime;

import java.nio.charset.StandardCharsets;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.ExecutionException;

public final class HostCalls {
    private HostCalls() {}

    /** Generated per package: method id + encoded request -> encoded answer. */
    @FunctionalInterface
    public interface Dispatch {
        CompletionStage<byte[]> call(int methodId, byte[] request) throws Exception;
    }

    private static volatile Dispatch dispatch;
    private static volatile AbiCalls abi;

    /** Installs the dispatcher for all imports of this package (one per process; replaces a previous one). */
    public static void install(NativeBackend backend, Dispatch d) {
        if (d == null) throw new NullPointerException("dispatch");
        var info = backend.info();
        if ((info.capabilities() & AbiConstants.WABI_CAP_HOST_CALLS) == 0) {
            throw new WabiIncompatibleLibraryException("native core '" + info.description() + "' does not support host calls (ABI 1.1)");
        }
        dispatch = d;
        abi = backend.abi();
        int status = abi.hostRegister(true);
        if (status != AbiConstants.WABI_OK) throw backend.error(status, null);
    }

    /** Removes the handler; later host calls fail inside the core. */
    public static void uninstall(NativeBackend backend) {
        backend.abi().hostRegister(false);
        dispatch = null;
    }

    /**
     * Called by the binding (Panama upcall / JNI shim) on a core thread with a
     * copy of the request. Must return quickly: the app code runs on the
     * executor, and the answer goes back with host_reply exactly once.
     */
    static void onHostCall(int methodId, byte[] request, long call) {
        var target = abi;
        if (target == null) return; // not installed through install(): cannot reply
        NativeBackend.EXECUTOR.execute(() -> run(target, methodId, request, call));
    }

    private static void run(AbiCalls target, int methodId, byte[] request, long call) {
        CompletionStage<byte[]> stage;
        try {
            var d = dispatch;
            if (d == null) throw new WabiNativeException(AbiConstants.WABI_ERR_UNSUPPORTED, "no host handler", null);
            stage = d.call(methodId, request);
        } catch (Throwable t) {
            stage = CompletableFuture.failedFuture(t);
        }
        stage.whenComplete((bytes, failure) -> {
            if (failure == null) {
                target.hostReply(call, AbiConstants.WABI_OK, bytes == null ? new byte[0] : bytes);
                return;
            }
            Throwable cause = failure;
            while ((cause instanceof CompletionException || cause instanceof ExecutionException) && cause.getCause() != null) {
                cause = cause.getCause();
            }
            int status = cause instanceof WabiNativeException ne ? ne.status() : AbiConstants.WABI_ERR_INTERNAL;
            String text = cause.getClass().getSimpleName() + ": " + cause.getMessage();
            target.hostReply(call, status, text.getBytes(StandardCharsets.UTF_8)); // an app bug: reported, never lost
        });
    }
}

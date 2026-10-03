// Hand-written, generic runtime for Java cores (GraalVM native-image shared
// library, see NativeExports + native/wabi_graal_shim.c). Pure Java — the
// business logic and this class also run on a plain JVM (tests).
package wabi.core.runtime;

import java.nio.charset.StandardCharsets;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public final class CoreRuntime {
    private CoreRuntime() {}

    /** Core threads: async work and completion callbacks run here, never on host threads. */
    public static final ExecutorService EXECUTOR = Executors.newCachedThreadPool(r -> {
        var t = new Thread(r, "wabi-core");
        t.setDaemon(true);
        return t;
    });

    /** The package this core implements (generated binding). */
    public static final CorePackage PACKAGE = Binding.PACKAGE;

    static byte[] utf8(Throwable t) {
        return String.valueOf(t.getMessage()).getBytes(StandardCharsets.UTF_8);
    }

    /** wabi_call_sync: status + payload (payload = diagnostic on failure). */
    public static Object[] callSync(long object, int methodId, byte[] request) {
        try {
            return new Object[] {Status.OK, PACKAGE.dispatchSync(object, methodId, request)};
        } catch (AbiException e) { // incl. undecodable requests (INVALID_ARGUMENT, generated glue)
            return new Object[] {e.status(), utf8(e)};
        } catch (Throwable e) {
            return new Object[] {Status.INTERNAL, utf8(e)};
        }
    }

    /** wabi_call_async: status + future handle. */
    public static long[] callAsync(long object, int methodId, byte[] request) {
        final java.util.function.Supplier<java.util.concurrent.CompletionStage<byte[]>> run;
        try {
            run = PACKAGE.dispatchAsync(object, methodId, request);
        } catch (AbiException e) {
            return new long[] {e.status(), 0};
        } catch (Throwable e) {
            return new long[] {Status.INTERNAL, 0};
        }
        var f = new NativeFuture();
        var work = CompletableFuture.supplyAsync(run, EXECUTOR).thenCompose(s -> s);
        f.attach(work);
        work.whenComplete((bytes, failure) -> {
            if (failure == null) {
                f.complete(Status.OK, bytes);
            } else {
                Throwable cause = failure instanceof CompletionException && failure.getCause() != null ? failure.getCause() : failure;
                f.complete(Status.INTERNAL, utf8(cause));
            }
        });
        return new long[] {Status.OK, f.handle};
    }

    public static int objectDrop(long object) {
        var o = Handles.remove(object, CoreObject.class);
        if (o == null || (object >>> 56) != Handles.OBJECT) return Status.INVALID_HANDLE;
        o.close();
        return Status.OK;
    }

    public static NativeFuture future(long h) {
        return Handles.get(h, NativeFuture.class);
    }

    public static int futureDrop(long h) {
        var f = Handles.remove(h, NativeFuture.class);
        if (f == null) return Status.INVALID_HANDLE;
        f.cancelByHost();
        return Status.OK;
    }
}

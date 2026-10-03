// Hand-written, generic: the GraalVM native-image entry points of a Java core.
//
// GraalVM entry points always take an IsolateThread, and C structs cannot be
// passed by value, so the public ABI symbols (<prefix>_call_sync, ...) live in
// the tiny C shim native/wabi_graal_shim.c: it attaches the calling thread to
// the core's isolate, unpacks wabi_bytes/wabi_buffer and calls these
// wabi_jvm_* functions. Everything else is plain Java (CoreRuntime).
package wabi.core.runtime;

import org.graalvm.nativeimage.IsolateThread;
import org.graalvm.nativeimage.UnmanagedMemory;
import org.graalvm.nativeimage.c.function.CEntryPoint;
import org.graalvm.nativeimage.c.function.CFunctionPointer;
import org.graalvm.nativeimage.c.function.InvokeCFunctionPointer;
import org.graalvm.nativeimage.c.type.CCharPointer;
import org.graalvm.nativeimage.c.type.CCharPointerPointer;
import org.graalvm.nativeimage.c.type.CLongPointer;
import org.graalvm.nativeimage.c.type.CTypeConversion;
import org.graalvm.word.PointerBase;
import org.graalvm.word.WordFactory;

public final class NativeExports {
    private NativeExports() {}

    /** void (*wabi_future_callback)(void* user_data, uint64_t future) */
    interface FutureCallback extends CFunctionPointer {
        @InvokeCFunctionPointer
        void invoke(PointerBase userData, long future);
    }

    static {
        NativeFuture.callbacks = (cb, ud, handle) -> {
            FutureCallback f = WordFactory.pointer(cb);
            f.invoke(WordFactory.pointer(ud), handle);
        };
    }

    private static CTypeConversion.CCharPointerHolder contractId, coreInfo;

    private static byte[] in(CCharPointer p, long n) {
        var b = new byte[(int) n];
        if (n > 0) CTypeConversion.asByteBuffer(p, (int) n).get(b);
        return b;
    }

    private static void out(byte[] b, CCharPointerPointer outPtr, CLongPointer outLen) {
        if (b == null || b.length == 0) {
            outPtr.write(WordFactory.nullPointer());
            outLen.write(0);
            return;
        }
        CCharPointer m = UnmanagedMemory.malloc(b.length);
        CTypeConversion.asByteBuffer(m, b.length).put(b);
        outPtr.write(m);
        outLen.write(b.length);
    }

    @CEntryPoint(name = "wabi_jvm_contract_id")
    static synchronized CCharPointer contractId(IsolateThread thread) {
        if (contractId == null) contractId = CTypeConversion.toCString(CoreRuntime.PACKAGE.contractId()); // never freed
        return contractId.get();
    }

    @CEntryPoint(name = "wabi_jvm_core_info")
    static synchronized CCharPointer coreInfo(IsolateThread thread) {
        if (coreInfo == null) coreInfo = CTypeConversion.toCString(CoreRuntime.PACKAGE.coreInfo());
        return coreInfo.get();
    }

    @CEntryPoint(name = "wabi_jvm_buffer_free")
    static void bufferFree(IsolateThread thread, PointerBase ptr) {
        if (ptr.isNonNull()) UnmanagedMemory.free(ptr);
    }

    @CEntryPoint(name = "wabi_jvm_call_sync")
    static int callSync(IsolateThread thread, long object, int methodId, CCharPointer request, long length,
            CCharPointerPointer outPtr, CLongPointer outLen) {
        try {
            var r = CoreRuntime.callSync(object, methodId, in(request, length));
            out((byte[]) r[1], outPtr, outLen);
            return (int) r[0];
        } catch (Throwable t) {
            return Status.INTERNAL; // never unwind into native code
        }
    }

    @CEntryPoint(name = "wabi_jvm_call_async")
    static int callAsync(IsolateThread thread, long object, int methodId, CCharPointer request, long length,
            CLongPointer outFuture) {
        try {
            var r = CoreRuntime.callAsync(object, methodId, in(request, length));
            outFuture.write(r[1]);
            return (int) r[0];
        } catch (Throwable t) {
            return Status.INTERNAL;
        }
    }

    @CEntryPoint(name = "wabi_jvm_object_drop")
    static int objectDrop(IsolateThread thread, long object) {
        try {
            return CoreRuntime.objectDrop(object);
        } catch (Throwable t) {
            return Status.INTERNAL;
        }
    }

    @CEntryPoint(name = "wabi_jvm_future_subscribe")
    static int futureSubscribe(IsolateThread thread, long future, FutureCallback callback, PointerBase userData) {
        if (callback.isNull()) return Status.INVALID_ARGUMENT;
        var f = CoreRuntime.future(future);
        return f == null ? Status.INVALID_HANDLE : f.subscribe(callback.rawValue(), userData.rawValue());
    }

    @CEntryPoint(name = "wabi_jvm_future_poll")
    static int futurePoll(IsolateThread thread, long future) {
        var f = CoreRuntime.future(future);
        return f == null ? Status.INVALID_HANDLE : f.poll();
    }

    @CEntryPoint(name = "wabi_jvm_future_take")
    static int futureTake(IsolateThread thread, long future, CCharPointerPointer outPtr, CLongPointer outLen) {
        try {
            var f = CoreRuntime.future(future);
            if (f == null) {
                out(null, outPtr, outLen);
                return Status.INVALID_HANDLE;
            }
            var r = f.take();
            out((byte[]) r[1], outPtr, outLen);
            return (int) r[0];
        } catch (Throwable t) {
            return Status.INTERNAL;
        }
    }

    @CEntryPoint(name = "wabi_jvm_future_cancel")
    static int futureCancel(IsolateThread thread, long future) {
        var f = CoreRuntime.future(future);
        if (f == null) return Status.INVALID_HANDLE;
        f.cancelByHost();
        return Status.OK;
    }

    @CEntryPoint(name = "wabi_jvm_future_drop")
    static int futureDrop(IsolateThread thread, long future) {
        try {
            return CoreRuntime.futureDrop(future);
        } catch (Throwable t) {
            return Status.INTERNAL;
        }
    }

    // Streams: the Java core emitter does not generate stream methods yet, so
    // no stream handle can exist; the functions still validate their input.

    @CEntryPoint(name = "wabi_jvm_stream_next")
    static int streamNext(IsolateThread thread, long stream, CLongPointer outFuture) {
        outFuture.write(0);
        return Status.INVALID_HANDLE;
    }

    @CEntryPoint(name = "wabi_jvm_stream_cancel")
    static int streamCancel(IsolateThread thread, long stream) {
        return Status.INVALID_HANDLE;
    }
}

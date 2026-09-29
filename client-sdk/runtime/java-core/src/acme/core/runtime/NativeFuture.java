// Hand-written, generic: one future per async call. The first transition out
// of pending wins; the completion callback fires exactly once, on a core
// thread, never on the host thread that cancels.
package acme.core.runtime;

import java.util.concurrent.CompletableFuture;

public final class NativeFuture {
    /** Invokes a host callback (C function pointer); implemented by the GraalVM entry layer. */
    public interface Callbacks {
        void invoke(long callback, long userData, long future);
    }

    static volatile Callbacks callbacks;

    public final long handle;
    private boolean done, taken, subscribed;
    private int status;
    private byte[] payload;
    private long callback, userData;
    private volatile CompletableFuture<?> work;

    public NativeFuture() {
        handle = Handles.add(Handles.FUTURE, this);
    }

    void attach(CompletableFuture<?> w) {
        work = w;
    }

    /** From a core thread. */
    public boolean complete(int s, byte[] p) {
        long cb, ud;
        synchronized (this) {
            if (done) return false;
            done = true;
            status = s;
            payload = p;
            cb = callback;
            ud = userData;
            callback = 0;
        }
        if (cb != 0) callbacks.invoke(cb, ud, handle);
        return true;
    }

    /** From the host: the callback runs on a core thread, never inline. */
    public void cancelByHost() {
        long cb, ud;
        synchronized (this) {
            if (done) return;
            done = true;
            status = Status.CANCELLED;
            cb = callback;
            ud = userData;
            callback = 0;
        }
        var w = work;
        if (w != null) w.cancel(true);
        if (cb != 0) CoreRuntime.EXECUTOR.execute(() -> callbacks.invoke(cb, ud, handle));
    }

    public synchronized int subscribe(long cb, long ud) {
        if (subscribed) return Status.BUSY;
        subscribed = true;
        if (done) return Status.READY;
        callback = cb;
        userData = ud;
        return Status.OK;
    }

    public synchronized int poll() {
        return done ? Status.READY : Status.PENDING;
    }

    /** status + payload (null if none). */
    public synchronized Object[] take() {
        if (!done) return new Object[] {Status.PENDING, null};
        if (taken) return new Object[] {Status.ALREADY_TAKEN, null};
        taken = true;
        var p = payload;
        payload = null;
        return new Object[] {status, p};
    }
}

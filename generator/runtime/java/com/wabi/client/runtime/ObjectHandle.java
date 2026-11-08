// Hand-written, generic: owns one object handle, dropped exactly once
// (close() or, as a safety net, when the owner becomes unreachable).
//
// Uses a PhantomReference queue instead of java.lang.ref.Cleaner, which is
// only available on Android API 33+.
package com.wabi.client.runtime;

import java.lang.ref.PhantomReference;
import java.lang.ref.ReferenceQueue;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

public final class ObjectHandle implements AutoCloseable {
    private static final ReferenceQueue<Object> QUEUE = new ReferenceQueue<>();
    /** Keeps the phantom references themselves reachable until processed. */
    private static final Set<Ref> LIVE = ConcurrentHashMap.newKeySet();

    static {
        var t = new Thread(() -> {
            while (true) {
                try {
                    var ref = (Ref) QUEUE.remove();
                    ref.release();
                } catch (InterruptedException e) {
                    return;
                } catch (Throwable ignored) {
                    // keep the cleaner alive
                }
            }
        }, "wabi-object-cleaner");
        t.setDaemon(true);
        t.start();
    }

    private static final class Ref extends PhantomReference<Object> {
        final AtomicLong value;
        final ClientBackend backend;

        Ref(Object owner, ClientBackend backend, long value) {
            super(owner, QUEUE);
            this.backend = backend;
            this.value = new AtomicLong(value);
        }

        void release() {
            LIVE.remove(this);
            long v = value.getAndSet(0);
            if (v != 0) backend.dropObject(v); // ABI is thread-safe: fine from the cleaner thread
        }
    }

    private final Ref ref;

    public ObjectHandle(Object owner, ClientBackend backend, long value) {
        if (value == 0) throw new WabiProtocolException("core returned the null handle");
        this.ref = new Ref(owner, backend, value);
        LIVE.add(ref);
    }

    public long get() {
        long v = ref.value.get();
        if (v == 0) throw new IllegalStateException("native object already closed");
        return v;
    }

    /** Gives the handle away without dropping the object: it now belongs to someone else (the core). */
    public long detach() {
        long v = ref.value.getAndSet(0);
        if (v == 0) throw new IllegalStateException("native object already closed");
        ref.clear();
        ref.release(); // nothing left to drop
        return v;
    }

    @Override
    public void close() {
        ref.clear();
        ref.release();
    }
}

// Hand-written, generic: owns one object handle, dropped exactly once
// (close() or, as a safety net, when the owner becomes unreachable).
package com.acme.client.runtime;

import java.lang.ref.Cleaner;
import java.util.concurrent.atomic.AtomicLong;

public final class ObjectHandle implements AutoCloseable {
    private static final Cleaner CLEANER = Cleaner.create();

    private final State state;
    private final Cleaner.Cleanable cleanable;

    private static final class State implements Runnable {
        final AtomicLong value;
        final ClientBackend backend;

        State(ClientBackend backend, long value) {
            this.backend = backend;
            this.value = new AtomicLong(value);
        }

        @Override
        public void run() {
            long v = value.getAndSet(0);
            if (v != 0) backend.dropObject(v); // ABI is thread-safe: fine from the cleaner thread
        }
    }

    public ObjectHandle(Object owner, ClientBackend backend, long value) {
        if (value == 0) throw new AcmeProtocolException("core returned the null handle");
        this.state = new State(backend, value);
        this.cleanable = CLEANER.register(owner, state);
    }

    public long get() {
        long v = state.value.get();
        if (v == 0) throw new IllegalStateException("native object already closed");
        return v;
    }

    @Override
    public void close() {
        cleanable.clean();
    }
}

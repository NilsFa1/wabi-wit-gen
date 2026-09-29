// Hand-written, generic: maps an ABI stream handle onto java.util.concurrent.Flow.
//
// Backpressure is natural: the publisher only pulls (acme_stream_next) while the
// subscriber has outstanding demand, and at most one pull is in flight, so
// onNext/onComplete/onError are signalled serially (Reactive Streams §1.3).
package com.acme.client.runtime;

import java.util.concurrent.Flow;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.LongSupplier;

final class StreamPublisher<T> implements Flow.Publisher<T> {
    private final NativeBackend backend;
    private final LongSupplier open;
    private final ClientBackend.Decoder<T> decoder;

    StreamPublisher(NativeBackend backend, LongSupplier open, ClientBackend.Decoder<T> decoder) {
        this.backend = backend;
        this.open = open;
        this.decoder = decoder;
    }

    @Override
    public void subscribe(Flow.Subscriber<? super T> subscriber) {
        long stream;
        try {
            stream = open.getAsLong(); // cold: every subscriber gets its own native stream
        } catch (Throwable t) {
            subscriber.onSubscribe(new Flow.Subscription() {
                public void request(long n) {}
                public void cancel() {}
            });
            subscriber.onError(t);
            return;
        }
        subscriber.onSubscribe(new Subscription<>(backend, stream, decoder, subscriber));
    }

    private static final class Subscription<T> implements Flow.Subscription {
        private final NativeBackend backend;
        private final long stream;
        private final ClientBackend.Decoder<T> decoder;
        private final Flow.Subscriber<? super T> subscriber;
        private final AtomicLong demand = new AtomicLong();
        private final AtomicBoolean pulling = new AtomicBoolean();
        private final AtomicBoolean released = new AtomicBoolean();
        private volatile boolean done;

        Subscription(NativeBackend backend, long stream, ClientBackend.Decoder<T> decoder, Flow.Subscriber<? super T> s) {
            this.backend = backend;
            this.stream = stream;
            this.decoder = decoder;
            this.subscriber = s;
        }

        @Override
        public void request(long n) {
            if (n <= 0) {
                cancel();
                subscriber.onError(new IllegalArgumentException("request(n) requires n > 0 (§3.9)"));
                return;
            }
            demand.accumulateAndGet(n, (a, b) -> a + b < 0 ? Long.MAX_VALUE : a + b);
            schedule();
        }

        @Override
        public void cancel() {
            if (done) return;
            done = true;
            backend.abi().acme_stream_cancel(stream); // a pending pull completes as cancelled
            if (!pulling.get()) release();
        }

        private void release() {
            if (released.compareAndSet(false, true)) backend.abi().acme_stream_drop(stream);
        }

        /** Starts one pull if there is demand and none in flight (trampolined via the executor). */
        private void schedule() {
            if (done || demand.get() == 0 || !pulling.compareAndSet(false, true)) return;
            NativeBackend.EXECUTOR.execute(this::pull);
        }

        private void pull() {
            long future;
            try {
                future = backend.streamNext(stream);
            } catch (Throwable t) {
                finish(t);
                return;
            }
            backend.whenReady(future).whenComplete((ignored, failure) -> {
                NativeBackend.Taken taken = null;
                try {
                    if (failure == null) taken = backend.take(future);
                } finally {
                    backend.abi().acme_future_drop(future);
                }
                if (failure != null) {
                    finish(failure);
                } else if (taken.status() == NativeAbi.ACME_OK) {
                    if (done) {
                        pulling.set(false);
                        release();
                        return;
                    }
                    T item;
                    try {
                        item = decoder.decode(taken.bytes());
                    } catch (Throwable t) {
                        finish(t);
                        return;
                    }
                    demand.decrementAndGet();
                    subscriber.onNext(item);
                    pulling.set(false);
                    if (done) release();
                    else schedule();
                } else if (taken.status() == NativeAbi.ACME_STREAM_END || taken.status() == NativeAbi.ACME_ERR_CANCELLED) {
                    finish(null); // end of stream, or cancelled (by us or because the object was closed)
                } else {
                    finish(backend.error(taken.status(), taken.bytes()));
                }
            });
        }

        private void finish(Throwable error) {
            boolean wasDone = done;
            done = true;
            pulling.set(false);
            release();
            if (wasDone) return; // subscriber cancelled: no further signals (§1.8)
            if (error == null) subscriber.onComplete();
            else subscriber.onError(error);
        }
    }
}

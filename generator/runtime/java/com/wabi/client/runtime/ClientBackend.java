// Hand-written, generic: the seam between generated SDK code and whatever
// executes the contract (native core, WebAssembly component, test double).
package com.wabi.client.runtime;

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Flow;
import java.util.function.LongSupplier;

public interface ClientBackend {
    /** Decodes a result payload; may throw a generated contract exception. */
    @FunctionalInterface
    interface Decoder<T> {
        T decode(byte[] payload);
    }

    String description();

    /** Synchronous WIT function (never blocks on I/O). Returns the encoded result. */
    byte[] callSync(long object, int methodId, byte[] request);

    /**
     * Async WIT function. {@code cancel(true/false)} on the returned future cancels the
     * operation in the core (futures derived via thenApply etc. do not propagate cancel
     * upstream — standard CompletableFuture semantics).
     */
    <T> CompletableFuture<T> callAsync(long object, int methodId, byte[] request, Decoder<T> decoder);

    /** Cold publisher: every subscribe() calls {@code open} to obtain a fresh stream handle. */
    <T> Flow.Publisher<T> publisher(LongSupplier open, Decoder<T> decoder);

    void dropObject(long object);
}

// Hand-written, generic: one WIT package as seen by the generic exports.
// Implemented by the generated glue (wabi-wit-gen `java-core` target).
package wabi.core.runtime;

import java.util.concurrent.CompletionStage;
import java.util.function.Supplier;

public interface CorePackage {
    /** e.g. "gematik:pki@0.1.0" */
    String contractId();

    /** Free text identifying the core implementation. */
    String coreInfo();

    /** Sync method or constructor: the encoded result. */
    byte[] dispatchSync(long object, int methodId, byte[] request) throws AbiException;

    /** Async method: validates the call and returns the work to run on a core thread. */
    Supplier<CompletionStage<byte[]>> dispatchAsync(long object, int methodId, byte[] request) throws AbiException;
}

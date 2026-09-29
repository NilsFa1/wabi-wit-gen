// Hand-written, generic.
package com.acme.client.runtime;

/** A result error whose type is not a named contract type. */
public final class AcmeCallFailedException extends AcmeException {
    private final transient Object error;

    public AcmeCallFailedException(Object error) {
        super("call failed: " + error);
        this.error = error;
    }

    public Object error() {
        return error;
    }
}

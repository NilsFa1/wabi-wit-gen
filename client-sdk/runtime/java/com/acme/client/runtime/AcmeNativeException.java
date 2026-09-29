// Hand-written, generic.
package com.acme.client.runtime;

/** An ABI-level failure (never a domain error), e.g. an invalid handle or a core bug. */
public final class AcmeNativeException extends AcmeException {
    private final int status;

    public AcmeNativeException(int status, String statusName, String detail) {
        super(detail == null || detail.isEmpty() ? statusName : statusName + ": " + detail);
        this.status = status;
    }

    public int status() {
        return status;
    }
}

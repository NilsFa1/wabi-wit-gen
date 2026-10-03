// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic.
package com.gematik.zero.runtime;

/** An ABI-level failure (never a domain error), e.g. an invalid handle or a core bug. */
public final class WabiNativeException extends WabiException {
    private final int status;

    public WabiNativeException(int status, String statusName, String detail) {
        super(detail == null || detail.isEmpty() ? statusName : statusName + ": " + detail);
        this.status = status;
    }

    public int status() {
        return status;
    }
}

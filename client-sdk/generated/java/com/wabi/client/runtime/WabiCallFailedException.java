// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic.
package com.wabi.client.runtime;

/** A result error whose type is not a named contract type. */
public final class WabiCallFailedException extends WabiException {
    private final transient Object error;

    public WabiCallFailedException(Object error) {
        super("call failed: " + error);
        this.error = error;
    }

    public Object error() {
        return error;
    }
}

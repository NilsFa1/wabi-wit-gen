// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic.
package com.gematik.pki.runtime;

/** Base class of every exception thrown by an Wabi SDK. */
public class WabiException extends RuntimeException {
    public WabiException(String message) {
        super(message);
    }

    public WabiException(String message, Throwable cause) {
        super(message, cause);
    }
}

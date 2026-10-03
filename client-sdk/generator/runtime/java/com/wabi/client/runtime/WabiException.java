// Hand-written, generic.
package com.wabi.client.runtime;

/** Base class of every exception thrown by an Wabi SDK. */
public class WabiException extends RuntimeException {
    public WabiException(String message) {
        super(message);
    }

    public WabiException(String message, Throwable cause) {
        super(message, cause);
    }
}

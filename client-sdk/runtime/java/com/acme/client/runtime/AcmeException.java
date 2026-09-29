// Hand-written, generic.
package com.acme.client.runtime;

/** Base class of every exception thrown by an Acme SDK. */
public class AcmeException extends RuntimeException {
    public AcmeException(String message) {
        super(message);
    }

    public AcmeException(String message, Throwable cause) {
        super(message, cause);
    }
}

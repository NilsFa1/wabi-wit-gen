// Hand-written, generic.
package com.acme.client.runtime;

/** The core broke the ABI contract or returned an undecodable payload. */
public final class AcmeProtocolException extends AcmeException {
    public AcmeProtocolException(String message) {
        super(message);
    }
}

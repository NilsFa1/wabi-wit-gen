// Hand-written, generic.
package com.wabi.client.runtime;

/** The core broke the ABI contract or returned an undecodable payload. */
public final class WabiProtocolException extends WabiException {
    public WabiProtocolException(String message) {
        super(message);
    }
}

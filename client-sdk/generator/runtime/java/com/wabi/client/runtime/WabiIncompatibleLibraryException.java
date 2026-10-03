// Hand-written, generic.
package com.wabi.client.runtime;

/** The loaded native library does not implement a compatible ABI/contract version. */
public final class WabiIncompatibleLibraryException extends WabiException {
    public WabiIncompatibleLibraryException(String message) {
        super(message);
    }
}

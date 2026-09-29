// Hand-written, generic.
package com.acme.client.runtime;

/** The loaded native library does not implement a compatible ABI/contract version. */
public final class AcmeIncompatibleLibraryException extends AcmeException {
    public AcmeIncompatibleLibraryException(String message) {
        super(message);
    }
}

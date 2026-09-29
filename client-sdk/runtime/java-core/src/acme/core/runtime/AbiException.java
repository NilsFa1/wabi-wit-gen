// Hand-written, generic: an ABI-level failure (never a domain error).
package acme.core.runtime;

public final class AbiException extends Exception {
    private static final long serialVersionUID = 1L;
    private final int status;

    public AbiException(int status, String message) {
        super(message);
        this.status = status;
    }

    public int status() {
        return status;
    }
}

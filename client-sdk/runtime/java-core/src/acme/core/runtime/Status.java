// Hand-written, generic: ABI status codes (abi/acme_abi.h).
package acme.core.runtime;

public final class Status {
    private Status() {}

    public static final int OK = 0, PENDING = 1, READY = 2, STREAM_END = 3;
    public static final int INVALID_HANDLE = -1, INVALID_ARGUMENT = -2, UNKNOWN_METHOD = -3, CANCELLED = -4,
            BUSY = -5, ALREADY_TAKEN = -6, UNSUPPORTED = -7, INTERNAL = -8;
}

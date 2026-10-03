// Hand-written, generic: handle table. Handles are kind << 56 | serial and
// never reused, so stale handles are always detected.
package wabi.core.runtime;

import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

public final class Handles {
    private Handles() {}

    public static final long OBJECT = 1, FUTURE = 2, STREAM = 3;
    private static final AtomicLong SERIAL = new AtomicLong();
    private static final ConcurrentHashMap<Long, Object> TABLE = new ConcurrentHashMap<>();

    public static long add(long kind, Object value) {
        long h = kind << 56 | SERIAL.incrementAndGet();
        TABLE.put(h, value);
        return h;
    }

    public static <T> T get(long h, Class<T> type) {
        Object v = TABLE.get(h);
        return type.isInstance(v) ? type.cast(v) : null;
    }

    public static <T> T remove(long h, Class<T> type) {
        Object v = TABLE.get(h);
        return type.isInstance(v) && TABLE.remove(h, v) ? type.cast(v) : null;
    }

    /** A resource instance of the expected interface, or INVALID_HANDLE. */
    public static <T> T object(long h, Class<T> type) throws AbiException {
        T v = get(h, type);
        if (v == null || (h >>> 56) != OBJECT) throw new AbiException(Status.INVALID_HANDLE, "invalid handle");
        return v;
    }
}

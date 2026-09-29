// Hand-written, generic: a WIT resource instance living in the core.
package acme.core.runtime;

public interface CoreObject {
    /** Called when the host drops its handle; in-flight calls may still use the object. */
    default void close() {}
}

// Hand-written, generic: a do-nothing main class for build tools that insist on
// one (Gluon Substrate's sharedlib goal). The library itself is entered only
// through the @CEntryPoint functions in NativeExports; this is never called.
package acme.core.runtime;

public final class SharedLibMain {
    private SharedLibMain() {}

    public static void main(String[] args) {}
}

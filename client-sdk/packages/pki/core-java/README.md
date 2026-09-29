# gematik:pki — Java core (gemLibPki, GraalVM native-image)

A core written in **Java**: a small wrapper around gematik
[ref-GemLibPki](https://github.com/gematik/ref-GemLibPki) that offers one
operation, **TUC_PKI_018 certificate verification** against the CAs listed in a
TSL. GraalVM `native-image` compiles it (with gemLibPki, BouncyCastle, JAXB, …)
into a plain C shared library `libgematik_pki.so` that exports the Acme native
ABI. Apps in C#, Java/Android, Dart/Flutter and Swift use it through the
generated SDKs, **without a JVM**.

```
wit/pki.wit                        contract: resource certificate-verifier { constructor(product-type, tsl); verify(...) }
core-java/
  src/main/java/com/gematik/pki/   hand-written: Core (factory), GemLibPkiVerifier (the wrapper)
  src/generated/java/              acme-wit-gen `java-core`: resource interface, Implementation, dispatcher
  src/main/resources/META-INF/native-image/…   reachability metadata (recorded with the tracing agent)
  src/test/java/                   JVM tests + TestPki (test CA, TSL, certificates)
  build-native.sh                  mvn package → cc shim → native-image --shared
runtime/java-core/                 generic Java core runtime (shared by every Java core)
  src/acme/core/runtime/           handles, futures, CoreRuntime (plain Java), NativeExports (@CEntryPoint)
  native/acme_graal_shim.c         exports <prefix>_* and attaches callers to the GraalVM isolate
```

## Why a C shim?

GraalVM entry points (`@CEntryPoint`) always take an `IsolateThread` as first
parameter and cannot take C structs by value, while the Acme ABI has neither.
The generic shim (≈150 lines of C, compiled with the package prefix) exports
the real ABI symbols, creates the isolate once, attaches every calling thread
and forwards to `acme_jvm_*` entry points. native-image hides all symbols but
its own entry points, so the shim is a separate tiny library linked against the
image: apps load `libgematik_pki.so`, which finds `libgematik_pki_image.so`
next to it (`rpath $ORIGIN`).

## Build

```sh
mvn -B verify                                  # compile + JVM tests (any JDK 21+)
GRAALVM_HOME=/path/to/graalvm-jdk-21 ./build-native.sh      # → build/pki/libgematik_pki.so
GRAALVM_HOME=… ./build-native.sh --trace       # re-record reachability metadata from the JVM tests
../run-tests.sh                                 # JVM tests, native build, C#/Java/Dart apps against the .so
```

Reachability metadata: gemLibPki uses reflection (JAXB for the TSL,
BouncyCastle as JCA provider, Lombok builders are plain code). The configs in
`src/main/resources/META-INF/native-image` were recorded by running the JVM
tests with `-agentlib:native-image-agent`; re-run `--trace` after changing the
wrapper or upgrading gemLibPki, and extend the tests if a new code path needs
reflection.

## Scope / limits of the PoC

* gemLibPki **0.9.3** (the newest version on Maven Central at the time).
* OCSP (`with-ocsp = true`) needs network access to the responder in the TSL;
  the tests use `with-ocsp = false` with a self-made test PKI + TSL.
* The TSL itself is not signature-checked here (TUC_PKI_001 would be a second
  method on the resource).
* Java cores support sync/async methods; `stream<T>` and host calls (WIT
  imports) are not implemented for Java cores yet.
* Build per target platform: native-image does not cross-compile (Linux .so,
  macOS .dylib, Windows .dll, Android/iOS need their own GraalVM targets).

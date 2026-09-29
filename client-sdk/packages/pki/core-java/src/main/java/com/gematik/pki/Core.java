// The Java core of gematik:pki: binds the generated contract to gemLibPki.
// Everything ABI-related lives in the generated glue and runtime/java-core.
package com.gematik.pki;

public final class Core implements Implementation {
    @Override
    public String coreInfo() {
        return "gematik-pki-java-core/0.1.0 (gemLibPki 0.9.3, GraalVM native-image)";
    }

    @Override
    public CertificateVerifierResource newCertificateVerifier(String productType, byte[] tsl) {
        return new GemLibPkiVerifier(productType, tsl);
    }
}

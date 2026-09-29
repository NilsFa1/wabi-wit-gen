// JVM tests of the Java core: the gemLibPki wrapper directly, and the
// generated dispatcher at the byte level (what the native library runs).
package com.gematik.pki;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;

import acme.core.runtime.AbiException;
import acme.core.runtime.Status;
import com.acme.client.runtime.MsgPackReader;
import com.acme.client.runtime.MsgPackWriter;
import java.nio.file.Path;
import java.util.List;
import java.util.concurrent.ExecutionException;
import org.junit.jupiter.api.BeforeAll;
import org.junit.jupiter.api.Test;

class CoreTest {
    static TestPki pki;

    @BeforeAll
    static void setUp() throws Exception {
        pki = new TestPki();
        var out = System.getProperty("pki.fixtures");
        if (out != null && !out.isEmpty()) pki.write(Path.of(out));
    }

    static CertificateVerifierResource verifier() {
        return new Core().newCertificateVerifier("PoC-Test", pki.tsl);
    }

    static PkiError error(byte[] cert, List<CertificateProfile> profiles) throws Exception {
        var e = assertThrows(ExecutionException.class,
                () -> verifier().verify(cert, profiles, false).toCompletableFuture().get());
        return assertInstanceOf(PkiException.class, e.getCause()).error();
    }

    static String code(PkiError e) {
        return assertInstanceOf(PkiError.VerificationFailed.class, e).value().code();
    }

    @Test
    void validCertificate() throws Exception {
        var a = verifier().verify(pki.valid, List.of(CertificateProfile.C_HCI_AUT_ECC), false).toCompletableFuture().get();
        assertEquals(CertificateProfile.C_HCI_AUT_ECC, a.profile());
        assertEquals(List.of("Betriebsstätte Arzt"), a.professionItems());
        assertEquals(List.of(TestPki.OID_PRAXIS_ARZT), a.professionOids());
        assertEquals("1-2-ARZT-Test-01", a.registrationNumber());
    }

    @Test
    void firstMatchingProfileWins() throws Exception {
        var a = verifier().verify(pki.valid, List.of(CertificateProfile.C_HP_AUT_ECC, CertificateProfile.C_HCI_AUT_ECC), false)
                .toCompletableFuture().get();
        assertEquals(CertificateProfile.C_HCI_AUT_ECC, a.profile());
    }

    @Test
    void expired() throws Exception {
        assertEquals("SE_1021_CERTIFICATE_NOT_VALID_TIME", code(error(pki.expired, List.of(CertificateProfile.C_HCI_AUT_ECC))));
    }

    @Test
    void wrongKeyUsage() throws Exception {
        assertEquals("SE_1016_WRONG_KEYUSAGE", code(error(pki.wrongKeyUsage, List.of(CertificateProfile.C_HCI_AUT_ECC))));
    }

    @Test
    void unknownCa() throws Exception {
        assertEquals("TE_1027_CA_CERT_MISSING", code(error(pki.unknownCa, List.of(CertificateProfile.C_HCI_AUT_ECC))));
    }

    @Test
    void wrongProfile() throws Exception {
        // gemLibPki checks key usage, then extended key usage, then the certificate type
        assertEquals("SE_1017_WRONG_EXTENDEDKEYUSAGE", code(error(pki.valid, List.of(CertificateProfile.C_FD_SIG))));
    }

    @Test
    void invalidInputs() throws Exception {
        assertInstanceOf(PkiError.InvalidInput.class, error(new byte[] {1, 2, 3}, List.of(CertificateProfile.C_HCI_AUT_ECC)));
        assertInstanceOf(PkiError.InvalidInput.class, error(pki.valid, List.of()));
        var broken = new Core().newCertificateVerifier("PoC-Test", "not xml".getBytes());
        var e = assertThrows(ExecutionException.class,
                () -> broken.verify(pki.valid, List.of(CertificateProfile.C_HCI_AUT_ECC), false).toCompletableFuture().get());
        assertInstanceOf(PkiError.InvalidInput.class, assertInstanceOf(PkiException.class, e.getCause()).error());
    }

    /** The generated dispatcher, byte for byte as the native entry points call it. */
    @Test
    void dispatcher() throws Exception {
        var glue = new CoreGlue();
        assertEquals("gematik:pki@0.1.0", glue.contractId());

        var ctor = new MsgPackWriter();
        ctor.writeArrayHeader(2);
        ctor.writeString("PoC-Test");
        ctor.writeBinary(pki.tsl);
        long handle = new MsgPackReader(glue.dispatchSync(0, MethodIds.CERTIFICATE_VERIFIER_CONSTRUCTOR, ctor.toByteArray())).readUInt64();

        var req = new MsgPackWriter();
        req.writeArrayHeader(3);
        req.writeBinary(pki.valid);
        req.writeArrayHeader(1);
        WireCodec.writeCertificateProfile(req, CertificateProfile.C_HCI_AUT_ECC);
        req.writeBool(false);
        var answer = glue.dispatchAsync(handle, MethodIds.CERTIFICATE_VERIFIER_VERIFY, req.toByteArray()).get().toCompletableFuture().get();
        var r = new MsgPackReader(answer);
        assertEquals(0, r.readResultTag());
        assertEquals("1-2-ARZT-Test-01", WireCodec.readAdmission(r).registrationNumber());

        var bad = assertThrows(AbiException.class, () -> glue.dispatchSync(0, 0x7777, new byte[] {(byte) 0x90}));
        assertEquals(Status.UNKNOWN_METHOD, bad.status());
        var wrongHandle = assertThrows(AbiException.class,
                () -> glue.dispatchAsync(handle + 1, MethodIds.CERTIFICATE_VERIFIER_VERIFY, req.toByteArray()));
        assertEquals(Status.INVALID_HANDLE, wrongHandle.status());
    }
}

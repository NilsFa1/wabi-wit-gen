// The Java/Android app side of gematik:pki: certificate verification with
// gemLibPki running as a GraalVM native library (Panama or JNI binding).
import com.acme.client.runtime.NativeBackend;
import com.gematik.pki.CertificateProfile;
import com.gematik.pki.CertificateVerifier;
import com.gematik.pki.PkiError;
import com.gematik.pki.PkiException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;

public final class PkiTests {
    static int passed, failed;

    interface Body { void run() throws Exception; }

    static void test(String name, Body body) {
        try {
            body.run();
            passed++;
            System.out.println("PASS " + name);
        } catch (Throwable t) {
            failed++;
            System.out.println("FAIL " + name + ": " + t);
        }
    }

    static void check(boolean c, String what) {
        if (!c) throw new AssertionError(what);
    }

    static byte[] load(String name) throws Exception {
        var dir = System.getenv().getOrDefault("PKI_FIXTURES", "packages/pki/tests/fixtures");
        return Files.readAllBytes(Path.of(dir, name));
    }

    static PkiError errorOf(CompletableFuture<?> f) throws InterruptedException {
        try {
            f.get();
        } catch (ExecutionException e) {
            if (e.getCause() instanceof PkiException p) return p.error();
            throw new AssertionError("unexpected " + e.getCause(), e.getCause());
        }
        throw new AssertionError("expected PkiException");
    }

    static String code(PkiError e) {
        return e instanceof PkiError.VerificationFailed f ? f.value().code() : "not a verification failure: " + e;
    }

    public static void main(String[] args) throws Exception {
        System.out.println("core under test: " + NativeBackend.shared().description());
        var aut = List.of(CertificateProfile.C_HCI_AUT_ECC);
        try (var verifier = new CertificateVerifier("PoC-Test", load("tsl.xml"))) {
            test("valid SMC-B certificate", () -> {
                var a = verifier.verify(load("valid.der"), aut, false).get();
                check(a.profile() == CertificateProfile.C_HCI_AUT_ECC, "profile");
                check(a.registrationNumber().equals("1-2-ARZT-Test-01"), a.registrationNumber());
                check(a.professionItems().equals(List.of("Betriebsstätte Arzt")), a.professionItems().toString());
            });
            test("first matching profile wins", () -> {
                var a = verifier.verify(load("valid.der"), List.of(CertificateProfile.C_HP_AUT_ECC, CertificateProfile.C_HCI_AUT_ECC), false).get();
                check(a.profile() == CertificateProfile.C_HCI_AUT_ECC, a.profile().toString());
            });
            test("expired certificate", () ->
                    check(code(errorOf(verifier.verify(load("expired.der"), aut, false))).equals("SE_1021_CERTIFICATE_NOT_VALID_TIME"), "code"));
            test("wrong key usage", () ->
                    check(code(errorOf(verifier.verify(load("wrong-key-usage.der"), aut, false))).equals("SE_1016_WRONG_KEYUSAGE"), "code"));
            test("issuer not in the TSL", () ->
                    check(code(errorOf(verifier.verify(load("unknown-ca.der"), aut, false))).equals("TE_1027_CA_CERT_MISSING"), "code"));
            test("invalid input", () ->
                    check(errorOf(verifier.verify(new byte[] {1, 2, 3}, aut, false)) instanceof PkiError.InvalidInput, "invalid input"));
            test("concurrent verifications", () -> {
                var all = new ArrayList<CompletableFuture<com.gematik.pki.Admission>>();
                for (int i = 0; i < 16; i++) all.add(verifier.verify(load("valid.der"), aut, false));
                for (var f : all) check(f.get().registrationNumber().equals("1-2-ARZT-Test-01"), "valid");
            });
        }
        System.out.println(passed + " passed, " + failed + " failed");
        System.exit(failed == 0 ? 0 : 1);
    }
}

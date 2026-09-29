// Variant 1: gemLibPki used directly in a Java application on the JVM (JIT),
// through the same wrapper code the native core runs — no SDK, no ABI.
import com.gematik.pki.CertificateProfile;
import com.gematik.pki.CertificateVerifierResource;
import com.gematik.pki.Core;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

public final class JvmDirect {
    public static void main(String[] args) throws Exception {
        var dir = Path.of(System.getenv().getOrDefault("PKI_FIXTURES", "../tests/fixtures"));
        byte[] tsl = Files.readAllBytes(dir.resolve("tsl.xml")), cert = Files.readAllBytes(dir.resolve("valid.der"));
        var aut = List.of(CertificateProfile.C_HCI_AUT_ECC);
        System.out.println("== JVM direct (" + System.getProperty("java.vm.name") + " " + System.getProperty("java.version") + ")");
        CertificateVerifierResource[] v = new CertificateVerifierResource[1];
        Bench.once("first verify (incl. TSL parse)", () -> {
            v[0] = new Core().newCertificateVerifier("Bench", tsl);
            v[0].verify(cert, aut, false).toCompletableFuture().get();
        });
        Bench.latency("verify (valid certificate)", () -> v[0].verify(cert, aut, false).toCompletableFuture().get());
        Bench.latency("rejected call (no profiles)", () -> {
            try { v[0].verify(cert, List.of(), false).toCompletableFuture().get(); } catch (Exception expected) {}
        });
        Bench.throughput("verify throughput", () -> v[0].verify(cert, aut, false).toCompletableFuture().get());
    }
}

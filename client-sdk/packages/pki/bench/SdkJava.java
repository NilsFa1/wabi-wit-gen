// Variants 2/3: a Java application using the generated SDK against the native
// library (gemLibPki compiled with GraalVM native-image), via Panama or JNI.
import com.gematik.pki.runtime.NativeBackend;
import com.gematik.pki.CertificateProfile;
import com.gematik.pki.CertificateVerifier;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;

public final class SdkJava {
    public static void main(String[] args) throws Exception {
        var dir = Path.of(System.getenv().getOrDefault("PKI_FIXTURES", "../tests/fixtures"));
        byte[] tsl = Files.readAllBytes(dir.resolve("tsl.xml")), cert = Files.readAllBytes(dir.resolve("valid.der"));
        var aut = List.of(CertificateProfile.C_HCI_AUT_ECC);
        CertificateVerifier[] v = new CertificateVerifier[1];
        Bench.once("load native library", () -> NativeBackend.shared());
        System.out.println("== Java SDK -> native library (" + NativeBackend.shared().binding() + ")");
        Bench.once("first verify (incl. TSL parse)", () -> {
            v[0] = new CertificateVerifier("Bench", tsl);
            v[0].verify(cert, aut, false).get();
        });
        Bench.latency("verify (valid certificate)", () -> v[0].verify(cert, aut, false).get());
        Bench.latency("rejected call (no profiles)", () -> {
            try { v[0].verify(cert, List.of(), false).get(); } catch (Exception expected) {}
        });
        Bench.throughput("verify throughput", () -> v[0].verify(cert, aut, false).get());
        v[0].close();
    }
}

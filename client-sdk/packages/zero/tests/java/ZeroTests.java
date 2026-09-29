// The app side of host calls in Java/Android: the app owns the key (here a JDK
// EC key standing in for the Android Keystore, which returns the same formats:
// SPKI DER public keys, ASN.1 DER signatures). The Go core (libzero) builds
// the DPoP proof and asks the app to sign. Runs with -Dacme.binding=panama|jni.
import com.gematik.zero.DpopSigner;
import com.gematik.zero.HostKeys;
import com.gematik.zero.ZeroError;
import com.gematik.zero.ZeroException;
import com.gematik.zero.ZeroHost;
import com.acme.client.runtime.NativeBackend;

import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.MessageDigest;
import java.security.Signature;
import java.security.interfaces.ECPublicKey;
import java.security.spec.ECGenParameterSpec;
import java.util.Base64;
import java.util.HashSet;
import java.util.List;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.stream.IntStream;

public final class ZeroTests {
    static final class AppKeys implements HostKeys {
        final KeyPair key;
        final AtomicInteger signatures = new AtomicInteger();

        AppKeys() throws Exception {
            var gen = KeyPairGenerator.getInstance("EC");
            gen.initialize(new ECGenParameterSpec("secp256r1"));
            key = gen.generateKeyPair();
        }

        @Override
        public CompletionStage<byte[]> publicKey(String keyId) {
            if (!keyId.equals("dpop-key")) throw new ZeroException(new ZeroError.KeyNotFound(keyId));
            return CompletableFuture.completedFuture(key.getPublic().getEncoded()); // SPKI DER
        }

        @Override
        public CompletionStage<byte[]> sign(String keyId, byte[] digest) {
            if (!keyId.equals("dpop-key")) {
                return CompletableFuture.failedFuture(new ZeroException(new ZeroError.KeyNotFound(keyId)));
            }
            return CompletableFuture.supplyAsync(() -> { // answered later, on another thread
                try {
                    var s = Signature.getInstance("NONEwithECDSA"); // digest computed by the core
                    s.initSign(key.getPrivate());
                    s.update(digest);
                    signatures.incrementAndGet();
                    return s.sign(); // ASN.1 DER
                } catch (Exception e) {
                    throw new ZeroException(new ZeroError.Crypto(e.toString()));
                }
            });
        }

        byte[] coord(BigInteger v) {
            byte[] b = v.toByteArray(), out = new byte[32];
            int n = Math.min(b.length, 32);
            System.arraycopy(b, b.length - n, out, 32 - n, n);
            return out;
        }

        byte[] x() { return coord(((ECPublicKey) key.getPublic()).getW().getAffineX()); }

        byte[] y() { return coord(((ECPublicKey) key.getPublic()).getW().getAffineY()); }

        boolean verify(String jwt) throws Exception {
            var parts = jwt.split("\\.");
            var s = Signature.getInstance("SHA256withECDSAinP1363Format");
            s.initVerify(key.getPublic());
            s.update((parts[0] + "." + parts[1]).getBytes(StandardCharsets.US_ASCII));
            return s.verify(Base64.getUrlDecoder().decode(parts[2]));
        }
    }

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
            t.printStackTrace(System.out);
        }
    }

    static void check(boolean c, String what) {
        if (!c) throw new AssertionError(what);
    }

    static String b64url(byte[] b) {
        return Base64.getUrlEncoder().withoutPadding().encodeToString(b);
    }

    static byte[] sha256(byte[] b) throws Exception {
        return MessageDigest.getInstance("SHA-256").digest(b);
    }

    static ZeroError errorOf(CompletableFuture<?> f) throws InterruptedException {
        try {
            f.get();
            throw new AssertionError("expected ZeroException");
        } catch (ExecutionException e) {
            if (e.getCause() instanceof ZeroException z) return z.error();
            throw new AssertionError("unexpected " + e.getCause(), e.getCause());
        }
    }

    public static void main(String[] args) throws Exception {
        var app = new AppKeys();
        System.out.println("core under test: " + NativeBackend.shared().description());

        test("without registration the core reports a domain error", () -> {
            try (var signer = new DpopSigner("dpop-key")) {
                check(errorOf(signer.thumbprint()) instanceof ZeroError.Crypto, "crypto error");
            }
        });

        ZeroHost.register(app);

        test("thumbprint (libzero) equals RFC 7638 computed here", () -> {
            try (var signer = new DpopSigner("dpop-key")) {
                var json = "{\"crv\":\"P-256\",\"kty\":\"EC\",\"x\":\"" + b64url(app.x()) + "\",\"y\":\"" + b64url(app.y()) + "\"}";
                check(signer.thumbprint().get().equals(b64url(sha256(json.getBytes(StandardCharsets.UTF_8)))), "thumbprint");
            }
        });

        test("DPoP proof is signed by the app key", () -> {
            try (var signer = new DpopSigner("dpop-key")) {
                var jwt = signer.createProof("GET", "https://zero.example/api", java.util.Optional.of("at-1"), java.util.Optional.empty()).get();
                check(app.verify(jwt), "signature");
                var parts = jwt.split("\\.");
                var header = new String(Base64.getUrlDecoder().decode(parts[0]), StandardCharsets.UTF_8);
                var claims = new String(Base64.getUrlDecoder().decode(parts[1]), StandardCharsets.UTF_8);
                check(header.contains("\"typ\":\"dpop+jwt\"") && header.contains(b64url(app.x())), header);
                check(claims.contains("\"htm\":\"GET\"") && claims.contains("\"htu\":\"https://zero.example/api\""), claims);
                check(claims.contains("\"ath\":\"" + b64url(sha256("at-1".getBytes(StandardCharsets.UTF_8))) + "\""), claims);
                check(!claims.contains("nonce"), claims);
            }
        });

        test("concurrent proofs", () -> {
            try (var signer = new DpopSigner("dpop-key")) {
                int before = app.signatures.get();
                List<CompletableFuture<String>> all = IntStream.range(0, 16)
                        .mapToObj(i -> signer.createProof("POST", "https://zero.example/" + i, java.util.Optional.empty(), java.util.Optional.of("n")))
                        .toList();
                var proofs = new HashSet<String>();
                for (var f : all) {
                    var jwt = f.get();
                    check(app.verify(jwt), "signature");
                    proofs.add(jwt);
                }
                check(proofs.size() == 16, "distinct proofs");
                check(app.signatures.get() - before == 16, "one signature per proof");
            }
        });

        test("app exception becomes the caller's ZeroException", () -> {
            try (var signer = new DpopSigner("unknown-key")) {
                var e = errorOf(signer.createProof("GET", "https://zero.example/", java.util.Optional.empty(), java.util.Optional.empty()));
                check(e instanceof ZeroError.KeyNotFound k && k.value().equals("unknown-key"), String.valueOf(e));
            }
        });

        test("invalid input is rejected by the core", () -> {
            try (var signer = new DpopSigner("dpop-key")) {
                var e = errorOf(signer.createProof("", "ftp://x", java.util.Optional.empty(), java.util.Optional.empty()));
                check(e instanceof ZeroError.InvalidInput, String.valueOf(e));
            }
        });

        ZeroHost.unregister();
        System.out.println(passed + " passed, " + failed + " failed");
        System.exit(failed == 0 ? 0 : 1);
    }
}

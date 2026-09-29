// The C# app side of gematik:pki: certificate verification with gemLibPki
// (Java) running as a GraalVM native library — no JVM in this process.
// Fixtures come from core-java/src/test/java/com/gematik/pki/TestPki.java.
using Gematik.Pki;

static class Program
{
    static int failures, passed;
    static string Fixtures => Environment.GetEnvironmentVariable("PKI_FIXTURES") ?? "packages/pki/tests/fixtures";
    static byte[] Load(string name) => File.ReadAllBytes(Path.Combine(Fixtures, name));

    static async Task Test(string name, Func<Task> body)
    {
        try { await body(); passed++; Console.WriteLine($"PASS {name}"); }
        catch (Exception e) { failures++; Console.WriteLine($"FAIL {name}: {e}"); }
    }

    static void Check(bool c, string what) { if (!c) throw new Exception(what); }

    static async Task<PkiError> ErrorOf(Func<Task> call)
    {
        try { await call(); }
        catch (PkiException e) { return e.Error; }
        throw new Exception("expected PkiException");
    }

    static string Code(PkiError e) => e is PkiError.VerificationFailed f ? f.Value.Code : $"not a verification failure: {e}";

    static async Task<int> Main()
    {
        using var verifier = new CertificateVerifier("PoC-Test", Load("tsl.xml"));
        var aut = new[] { CertificateProfile.CHciAutEcc };

        await Test("valid SMC-B certificate", async () =>
        {
            var a = await verifier.VerifyAsync(Load("valid.der"), aut, false);
            Check(a.Profile == CertificateProfile.CHciAutEcc, "profile");
            Check(a.RegistrationNumber == "1-2-ARZT-Test-01", a.RegistrationNumber);
            Check(a.ProfessionItems.SequenceEqual(new[] { "Betriebsstätte Arzt" }), string.Join(",", a.ProfessionItems));
            Check(a.ProfessionOids.SequenceEqual(new[] { "1.2.276.0.76.4.50" }), string.Join(",", a.ProfessionOids));
        });

        await Test("first matching profile wins", async () =>
        {
            var a = await verifier.VerifyAsync(Load("valid.der"), new[] { CertificateProfile.CHpAutEcc, CertificateProfile.CHciAutEcc }, false);
            Check(a.Profile == CertificateProfile.CHciAutEcc, a.Profile.ToString());
        });

        await Test("expired certificate", async () =>
            Check(Code(await ErrorOf(() => verifier.VerifyAsync(Load("expired.der"), aut, false))) == "SE_1021_CERTIFICATE_NOT_VALID_TIME", "code"));

        await Test("wrong key usage", async () =>
            Check(Code(await ErrorOf(() => verifier.VerifyAsync(Load("wrong-key-usage.der"), aut, false))) == "SE_1016_WRONG_KEYUSAGE", "code"));

        await Test("issuer not in the TSL", async () =>
            Check(Code(await ErrorOf(() => verifier.VerifyAsync(Load("unknown-ca.der"), aut, false))) == "TE_1027_CA_CERT_MISSING", "code"));

        await Test("invalid input", async () =>
            Check(await ErrorOf(() => verifier.VerifyAsync(new byte[] { 1, 2, 3 }, aut, false)) is PkiError.InvalidInput, "invalid input"));

        await Test("concurrent verifications", async () =>
        {
            var all = await Task.WhenAll(Enumerable.Range(0, 16).Select(_ => verifier.VerifyAsync(Load("valid.der"), aut, false)));
            Check(all.All(a => a.RegistrationNumber == "1-2-ARZT-Test-01"), "all valid");
        });

        Console.WriteLine($"{passed} passed, {failures} failed");
        return failures == 0 ? 0 : 1;
    }
}

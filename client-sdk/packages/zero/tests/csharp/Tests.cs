// The app side of host calls in C#: the app owns the key (here a .NET ECDsa
// key standing in for a TPM / Windows Hello / HSM), the Go core (libzero)
// builds the DPoP proof and asks the app to sign.
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Gematik.Zero;

sealed class AppKeys : IHostKeys
{
    public readonly ECDsa Key = ECDsa.Create(ECCurve.NamedCurves.nistP256);
    public int Signatures;

    public Task<byte[]> PublicKeyAsync(string keyId) =>
        keyId == "dpop-key" ? Task.FromResult(Key.ExportSubjectPublicKeyInfo())
                            : throw new ZeroException(new ZeroError.KeyNotFound(keyId));

    public async Task<byte[]> SignAsync(string keyId, byte[] digest)
    {
        if (keyId != "dpop-key") throw new ZeroException(new ZeroError.KeyNotFound(keyId));
        // a real keystore may prompt the user: answering later (and out of order) is fine
        await Task.Delay(Random.Shared.Next(0, 6));
        Interlocked.Increment(ref Signatures);
        return Key.SignHash(digest, DSASignatureFormat.Rfc3279DerSequence);
    }
}

static class Program
{
    static int failures, passed;

    static async Task Test(string name, Func<Task> body)
    {
        try { await body(); passed++; Console.WriteLine($"PASS {name}"); }
        catch (Exception e) { failures++; Console.WriteLine($"FAIL {name}: {e}"); }
    }

    static void Check(bool c, string what) { if (!c) throw new Exception(what); }

    static byte[] B64Url(string s)
    {
        s = s.Replace('-', '+').Replace('_', '/');
        return Convert.FromBase64String(s + new string('=', (4 - s.Length % 4) % 4));
    }

    static string B64Url(byte[] b) => Convert.ToBase64String(b).TrimEnd('=').Replace('+', '-').Replace('/', '_');

    static async Task<int> Main()
    {
        var app = new AppKeys();

        await Test("without registration the core reports a domain error", async () =>
        {
            using var signer = new DpopSigner("dpop-key");
            try { await signer.ThumbprintAsync(); throw new Exception("expected ZeroException"); }
            catch (ZeroException e) { Check(e.Error is ZeroError.Crypto, e.Error.ToString()); }
        });

        ZeroHost.Register(app);

        await Test("thumbprint (libzero) equals RFC 7638 computed here", async () =>
        {
            using var signer = new DpopSigner("dpop-key");
            var p = app.Key.ExportParameters(false);
            var json = $"{{\"crv\":\"P-256\",\"kty\":\"EC\",\"x\":\"{B64Url(p.Q.X!)}\",\"y\":\"{B64Url(p.Q.Y!)}\"}}";
            Check(await signer.ThumbprintAsync() == B64Url(SHA256.HashData(Encoding.UTF8.GetBytes(json))), "thumbprint");
        });

        await Test("DPoP proof is signed by the app key", async () =>
        {
            using var signer = new DpopSigner("dpop-key");
            var jwt = await signer.CreateProofAsync("GET", "https://zero.example/api", "at-1", null);
            var parts = jwt.Split('.');
            Check(parts.Length == 3, "three parts");
            Check(app.Key.VerifyData(Encoding.ASCII.GetBytes($"{parts[0]}.{parts[1]}"), B64Url(parts[2]),
                                     HashAlgorithmName.SHA256, DSASignatureFormat.IeeeP1363FixedFieldConcatenation), "signature");
            using var header = JsonDocument.Parse(B64Url(parts[0]));
            using var claims = JsonDocument.Parse(B64Url(parts[1]));
            Check(header.RootElement.GetProperty("typ").GetString() == "dpop+jwt", "typ");
            Check(header.RootElement.GetProperty("jwk").GetProperty("crv").GetString() == "P-256", "jwk");
            Check(claims.RootElement.GetProperty("htm").GetString() == "GET", "htm");
            Check(claims.RootElement.GetProperty("htu").GetString() == "https://zero.example/api", "htu");
            Check(claims.RootElement.GetProperty("ath").GetString() == B64Url(SHA256.HashData("at-1"u8.ToArray())), "ath");
            Check(!claims.RootElement.TryGetProperty("nonce", out _), "no nonce");
        });

        await Test("concurrent proofs", async () =>
        {
            using var signer = new DpopSigner("dpop-key");
            var before = app.Signatures;
            var proofs = await Task.WhenAll(Enumerable.Range(0, 64).Select(i => signer.CreateProofAsync("POST", $"https://zero.example/{i}", null, "n")));
            Check(proofs.Distinct().Count() == 64, "distinct proofs (jti)");
            Check(app.Signatures - before == 64, "one signature per proof");
            for (var i = 0; i < proofs.Length; i++) // every answer belongs to its own request
            {
                var parts = proofs[i].Split('.');
                Check(Encoding.UTF8.GetString(B64Url(parts[1])).Contains($"\"htu\":\"https://zero.example/{i}\""), $"proof {i} names its own URL");
                Check(app.Key.VerifyData(Encoding.ASCII.GetBytes($"{parts[0]}.{parts[1]}"), B64Url(parts[2]),
                                         HashAlgorithmName.SHA256, DSASignatureFormat.IeeeP1363FixedFieldConcatenation), $"proof {i} signed over its own content");
            }
        });

        await Test("app exception becomes the caller's ZeroException", async () =>
        {
            using var signer = new DpopSigner("unknown-key");
            try { await signer.CreateProofAsync("GET", "https://zero.example/", null, null); throw new Exception("expected ZeroException"); }
            catch (ZeroException e) { Check(e.Error is ZeroError.KeyNotFound { Value: "unknown-key" }, e.Error.ToString()); }
        });

        await Test("invalid input is rejected by the core", async () =>
        {
            using var signer = new DpopSigner("dpop-key");
            try { await signer.CreateProofAsync("", "ftp://x", null, null); throw new Exception("expected ZeroException"); }
            catch (ZeroException e) { Check(e.Error is ZeroError.InvalidInput, e.Error.ToString()); }
        });

        ZeroHost.Unregister();
        Console.WriteLine($"{passed} passed, {failures} failed");
        return failures == 0 ? 0 : 1;
    }
}

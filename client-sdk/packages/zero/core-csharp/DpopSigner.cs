// Business logic of the gematik:zero C# core: RFC 9449 DPoP proofs. The private
// key stays in the app; every signature is a host call (import host-keys).
using System.Security.Cryptography;
using System.Text;
using System.Text.Encodings.Web;
using System.Text.Json;
using Gematik.Zero;

namespace Acme.Core;

sealed class DpopSigner(string keyId) : ICoreObject
{
    /// <summary>SubjectPublicKeyInfo header for id-ecPublicKey / prime256v1 (+ 65-byte point).</summary>
    private static readonly byte[] SpkiPrefix =
        Convert.FromHexString("3059301306072a8648ce3d020106082a8648ce3d030107034200");

    public void Close() { }

    private static ZeroError Crypto(string m) => new ZeroError.Crypto(m);

    private static string B64Url(ReadOnlySpan<byte> b) =>
        Convert.ToBase64String(b).TrimEnd('=').Replace('+', '-').Replace('/', '_');

    private async Task<(string? X, string? Y, ZeroError? Error)> PublicJwkAsync(CancellationToken ct)
    {
        byte[]? der;
        ZeroError? error;
        try
        {
            (der, error) = await HostKeys.PublicKeyAsync(keyId, ct).ConfigureAwait(false);
        }
        catch (HostCallException e)
        {
            return (null, null, Crypto($"host-keys: {e.Message}"));
        }
        if (error is not null) return (null, null, error);
        if (der is not { Length: 91 } || !der.AsSpan(0, 26).SequenceEqual(SpkiPrefix) || der[26] != 0x04)
            return (null, null, Crypto("public key: expected EC P-256 SubjectPublicKeyInfo"));
        return (B64Url(der.AsSpan(27, 32)), B64Url(der.AsSpan(59, 32)), null);
    }

    public async Task<(string?, ZeroError?)> ThumbprintAsync(CancellationToken ct)
    {
        var (x, y, error) = await PublicJwkAsync(ct).ConfigureAwait(false);
        if (error is not null) return (null, error);
        // RFC 7638: required members in lexicographic order, no whitespace.
        var canonical = $"{{\"crv\":\"P-256\",\"kty\":\"EC\",\"x\":\"{x}\",\"y\":\"{y}\"}}";
        return (B64Url(SHA256.HashData(Encoding.UTF8.GetBytes(canonical))), null);
    }

    public async Task<(string?, ZeroError?)> CreateProofAsync(string method, string url, string? accessToken, string? nonce, CancellationToken ct)
    {
        if (method.Length == 0 || !url.StartsWith("http", StringComparison.Ordinal))
            return (null, new ZeroError.InvalidInput("http-method and an absolute http(s) http-url are required"));
        var (x, y, error) = await PublicJwkAsync(ct).ConfigureAwait(false);
        if (error is not null) return (null, error);

        var header = Json(w =>
        {
            w.WriteString("typ", "dpop+jwt");
            w.WriteString("alg", "ES256");
            w.WriteStartObject("jwk");
            w.WriteString("kty", "EC");
            w.WriteString("crv", "P-256");
            w.WriteString("x", x);
            w.WriteString("y", y);
            w.WriteEndObject();
        });
        var claims = Json(w =>
        {
            w.WriteString("jti", B64Url(RandomNumberGenerator.GetBytes(16)));
            w.WriteString("htm", method);
            w.WriteString("htu", url);
            w.WriteNumber("iat", DateTimeOffset.UtcNow.ToUnixTimeSeconds());
            if (accessToken is not null) w.WriteString("ath", B64Url(SHA256.HashData(Encoding.UTF8.GetBytes(accessToken))));
            if (nonce is not null) w.WriteString("nonce", nonce);
        });
        var input = $"{B64Url(header)}.{B64Url(claims)}";

        byte[]? der;
        try
        {
            (der, error) = await HostKeys.SignAsync(keyId, SHA256.HashData(Encoding.ASCII.GetBytes(input)), ct).ConfigureAwait(false);
        }
        catch (HostCallException e)
        {
            return (null, Crypto($"host-keys: {e.Message}"));
        }
        if (error is not null) return (null, error);
        var jose = DerToJose(der!);
        return jose is null ? (null, Crypto("signature: expected an ASN.1 DER ECDSA signature")) : ($"{input}.{B64Url(jose)}", null);
    }

    private static byte[] Json(Action<Utf8JsonWriter> body)
    {
        using var ms = new MemoryStream();
        using (var w = new Utf8JsonWriter(ms, new JsonWriterOptions { Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping }))
        {
            w.WriteStartObject();
            body(w);
            w.WriteEndObject();
        }
        return ms.ToArray();
    }

    /// <summary>ASN.1 DER SEQUENCE { INTEGER r, INTEGER s } → JOSE r || s (32 + 32 bytes).</summary>
    private static byte[]? DerToJose(byte[] der)
    {
        if (der.Length < 2 || der[0] != 0x30 || der[1] != der.Length - 2) return null;
        var out64 = new byte[64];
        var pos = 2;
        for (var half = 0; half < 2; half++)
        {
            if (pos + 2 > der.Length || der[pos] != 0x02) return null;
            int n = der[pos + 1];
            if (pos + 2 + n > der.Length) return null;
            var value = der.AsSpan(pos + 2, n);
            while (value.Length > 0 && value[0] == 0) value = value[1..]; // sign padding
            if (value.Length > 32) return null;
            value.CopyTo(out64.AsSpan(half * 32 + 32 - value.Length));
            pos += 2 + n;
        }
        return pos == der.Length ? out64 : null;
    }
}

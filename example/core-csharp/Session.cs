// One session with the backend, over plain HTTP/1.1. The protocol is the one of
// the Rust core (core-rust/src/protocol): every body is sealed with AES-256-GCM
// under a key derived from the shared secret, every request is signed by the
// app-held key (host call), and one long-lived response delivers the backend's
// events as server-sent events. JSON is read and written without reflection
// (NativeAOT).
using System.Net;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Demo.Client;

namespace Wabi.Core;

sealed class Session
{
    public const string PathSessions = "/v1/sessions", PathEvents = "/v1/events", PathWhoami = "/v1/whoami",
        PathCustomers = "/v1/customers", PathCommands = "/v1/commands";
    private const string HeaderSession = "x-demo-session", HeaderKey = "x-demo-key", HeaderSignature = "x-demo-signature";
    private const int NonceSize = 12, TagSize = 16;
    private static readonly byte[] AadRequest = "req"u8.ToArray(), AadResponse = "res"u8.ToArray(), AadEvent = "evt"u8.ToArray();

    // All sessions share one connection pool.
    private static readonly HttpClient Http = new(new SocketsHttpHandler { UseProxy = false }) { Timeout = Timeout.InfiniteTimeSpan };

    private readonly Transport _link;
    private readonly string _base;
    private readonly byte[] _key;
    private readonly string? _keyId; // app-held key that signs every request
    private string _id = "";
    private readonly CancellationTokenSource _ended = new();

    /// <summary>Cancelled once the event channel has ended: by <see cref="End"/> or by the backend.</summary>
    public CancellationToken Ended => _ended.Token;
    public bool Alive => !_ended.IsCancellationRequested;
    public void End() => _ended.Cancel();

    private Session(ClientOptions o, Transport link)
    {
        _link = link;
        _base = link.Endpoint.TrimEnd('/');
        // HMAC-SHA256(api-key, "demo-client/v1 body key")
        _key = HMACSHA256.HashData(Encoding.UTF8.GetBytes(o.ApiKey ?? ""), "demo-client/v1 body key"u8);
        _keyId = o.KeyId;
    }

    private static ClientException Fail(ClientError e) => new(e);

    public static async Task<Session> OpenAsync(ClientOptions o, Transport link, Action<ClientEvent> publish, CancellationToken ct)
    {
        if (!link.Endpoint.StartsWith("http://", StringComparison.Ordinal) && !link.Endpoint.StartsWith("https://", StringComparison.Ordinal))
            throw Fail(new ClientError.NetworkError($"unsupported endpoint (the C# core speaks http:// only): {link.Endpoint}"));
        var s = new Session(o, link);
        try
        {
            var headers = new List<(string, string)>();
            if (s._keyId is not null)
            {
                // The app's keystore answers; the private key never comes here.
                var spki = await HostCall("public-key", () => HostKeys.PublicKeyAsync(s._keyId, ct)).ConfigureAwait(false);
                headers.Add((HeaderKey, Convert.ToBase64String(spki)));
            }
            var opened = await s.CallAsync(HttpMethod.Post, PathSessions, w => w.WriteString("client", "demo-csharp-core"), o.RequestTimeoutMs, ct, headers)
                .ConfigureAwait(false);
            s._id = opened.GetProperty("session").GetString()!;

            // The event channel lives as long as the session; only its start is awaited.
            using var request = await s.RequestAsync(HttpMethod.Get, PathEvents, null, null, ct).ConfigureAwait(false);
            using var start = CancellationTokenSource.CreateLinkedTokenSource(ct, s.Ended);
            HttpResponseMessage response;
            try
            {
                response = await Http.SendAsync(request, HttpCompletionOption.ResponseHeadersRead, start.Token).ConfigureAwait(false);
            }
            catch (HttpRequestException e)
            {
                throw Fail(new ClientError.NetworkError(e.Message));
            }
            if (response.StatusCode != HttpStatusCode.OK)
            {
                response.Dispose();
                throw Fail(new ClientError.ProtocolError($"event channel: HTTP {(int)response.StatusCode}"));
            }
            _ = Task.Run(() => s.PumpAsync(response, publish));
            return s;
        }
        catch
        {
            s.End();
            throw;
        }
    }

    /// <summary>A failed host call becomes a domain error for the app.</summary>
    private static async Task<byte[]> HostCall(string what, Func<Task<byte[]>> call)
    {
        try
        {
            return await call().ConfigureAwait(false); // the app's own errors: ClientException
        }
        catch (HostCallException e)
        {
            throw Fail(new ClientError.KeyError($"host-keys.{what}: {e.Message}"));
        }
    }

    /// <summary>Percent-encodes everything but RFC 3986 unreserved characters.</summary>
    public static string QueryValue(string s)
    {
        var sb = new StringBuilder();
        foreach (var b in Encoding.UTF8.GetBytes(s))
        {
            if (b is >= (byte)'A' and <= (byte)'Z' or >= (byte)'a' and <= (byte)'z' or >= (byte)'0' and <= (byte)'9'
                or (byte)'-' or (byte)'.' or (byte)'_' or (byte)'~') sb.Append((char)b);
            else sb.Append('%').Append(b.ToString("X2"));
        }
        return sb.ToString();
    }

    /// <summary>nonce || ciphertext || tag</summary>
    private byte[] Seal(byte[] aad, ReadOnlySpan<byte> plaintext)
    {
        var sealedBody = new byte[NonceSize + plaintext.Length + TagSize];
        var nonce = sealedBody.AsSpan(0, NonceSize);
        RandomNumberGenerator.Fill(nonce);
        using var aes = new AesGcm(_key, TagSize);
        aes.Encrypt(nonce, plaintext, sealedBody.AsSpan(NonceSize, plaintext.Length), sealedBody.AsSpan(NonceSize + plaintext.Length), aad);
        return sealedBody;
    }

    private byte[]? Open(byte[] aad, ReadOnlySpan<byte> sealedBody)
    {
        if (sealedBody.Length < NonceSize + TagSize) return null;
        var plaintext = new byte[sealedBody.Length - NonceSize - TagSize];
        try
        {
            using var aes = new AesGcm(_key, TagSize);
            aes.Decrypt(sealedBody[..NonceSize], sealedBody[NonceSize..^TagSize], sealedBody[^TagSize..], plaintext, aad);
            return plaintext;
        }
        catch (CryptographicException)
        {
            return null;
        }
    }

    /// <summary>Seals the body and signs the request.</summary>
    private async Task<HttpRequestMessage> RequestAsync(HttpMethod method, string path, Action<Utf8JsonWriter>? body,
        IEnumerable<(string Name, string Value)>? headers, CancellationToken ct)
    {
        var sealedBody = Array.Empty<byte>();
        if (body is not null)
        {
            using var json = new MemoryStream();
            using (var w = new Utf8JsonWriter(json))
            {
                w.WriteStartObject();
                body(w);
                w.WriteEndObject();
            }
            sealedBody = Seal(AadRequest, json.GetBuffer().AsSpan(0, (int)json.Length));
        }
        _link.Count();
        var request = new HttpRequestMessage(method, _base + path) { Content = new ByteArrayContent(sealedBody) };
        foreach (var (name, value) in headers ?? []) request.Headers.TryAddWithoutValidation(name, value);
        if (_id.Length > 0) request.Headers.TryAddWithoutValidation(HeaderSession, _id);
        if (_keyId is not null)
        {
            // What the app-held key signs: SHA-256 over method, path, session and the sealed body.
            var digest = SHA256.HashData([.. Encoding.UTF8.GetBytes($"{method.Method}\n{path}\n{_id}\n"), .. sealedBody]);
            var signature = await HostCall("sign", () => HostKeys.SignAsync(_keyId, digest, ct)).ConfigureAwait(false);
            request.Headers.TryAddWithoutValidation(HeaderSignature, Convert.ToBase64String(signature));
        }
        return request;
    }

    /// <summary>One request/response exchange; the sealed JSON answer.</summary>
    public async Task<JsonElement> CallAsync(HttpMethod method, string path, Action<Utf8JsonWriter>? body, uint timeoutMs,
        CancellationToken ct, IEnumerable<(string, string)>? headers = null)
    {
        // The request ends with the host's token (cancellation), the session, or the timeout.
        using var timeout = new CancellationTokenSource();
        using var any = CancellationTokenSource.CreateLinkedTokenSource(ct, Ended, timeout.Token);
        if (timeoutMs > 0) timeout.CancelAfter((int)timeoutMs);
        HttpStatusCode status;
        byte[] sealedBody;
        try
        {
            using var request = await RequestAsync(method, path, body, headers, any.Token).ConfigureAwait(false);
            using var response = await Http.SendAsync(request, any.Token).ConfigureAwait(false);
            status = response.StatusCode;
            sealedBody = await response.Content.ReadAsByteArrayAsync(any.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (timeout.IsCancellationRequested && Alive && !ct.IsCancellationRequested)
        {
            throw Fail(new ClientError.NetworkError($"request timed out after {timeoutMs} ms"));
        }
        catch (OperationCanceledException)
        {
            throw Fail(new ClientError.Disconnected()); // (if the host cancelled, the result is discarded anyway)
        }
        catch (HttpRequestException e)
        {
            throw Fail(new ClientError.NetworkError(e.InnerException is { } inner ? $"{e.Message}: {inner.Message}" : e.Message));
        }
        var plain = Open(AadResponse, sealedBody);
        if (status == HttpStatusCode.OK && plain is null) throw Fail(new ClientError.ProtocolError("response cannot be decrypted"));
        if (status == HttpStatusCode.Unauthorized) throw Fail(new ClientError.Unauthorized()); // a backend with another key cannot even tell us why
        if (plain is null) throw Fail(new ClientError.ProtocolError($"HTTP {(int)status}"));
        JsonElement json;
        try
        {
            using var doc = JsonDocument.Parse(plain);
            json = doc.RootElement.Clone();
        }
        catch (JsonException e)
        {
            throw Fail(new ClientError.ProtocolError($"unexpected response shape: {e.Message}"));
        }
        if (status == HttpStatusCode.OK) return json;
        var code = json.TryGetProperty("code", out var c) ? c.GetString() ?? "" : "";
        var message = json.TryGetProperty("message", out var m) ? m.GetString() : null;
        throw Fail(code switch
        {
            "not-found" => new ClientError.NotFound(),
            "unauthorized" => new ClientError.Unauthorized(),
            _ => new ClientError.ProtocolError(message ?? code),
        });
    }

    /// <summary>Reads server-sent events (<c>data: &lt;base64 sealed JSON&gt;</c>) until the response ends or the session is ended.</summary>
    private async Task PumpAsync(HttpResponseMessage response, Action<ClientEvent> publish)
    {
        try
        {
            using var _ = response;
            using var lines = new StreamReader(await response.Content.ReadAsStreamAsync(Ended).ConfigureAwait(false));
            while (await lines.ReadLineAsync(Ended).ConfigureAwait(false) is { } line)
            {
                if (!line.StartsWith("data: ", StringComparison.Ordinal)) continue; // comments, keep-alives, blank lines
                try
                {
                    if (Open(AadEvent, Convert.FromBase64String(line[6..])) is not { } plain) continue;
                    using var doc = JsonDocument.Parse(plain);
                    var e = doc.RootElement;
                    switch (e.GetProperty("kind").GetString())
                    {
                        case "customer-updated":
                            var c = e.GetProperty("customer");
                            publish(new ClientEvent.CustomerUpdated(new Customer(c.GetProperty("id").GetString()!, c.GetProperty("name").GetString()!,
                                c.TryGetProperty("email", out var email) ? email.GetString() : null)));
                            break;
                        case "message-received":
                            publish(new ClientEvent.MessageReceived(e.GetProperty("text").GetString()!));
                            break;
                    }
                }
                catch (Exception ex) when (ex is FormatException or JsonException or KeyNotFoundException or InvalidOperationException)
                {
                    // not for us or from a newer backend: skip
                }
            }
        }
        catch (Exception ex) when (ex is OperationCanceledException or IOException or HttpRequestException)
        {
            // the session ended
        }
        finally
        {
            End();
        }
    }
}

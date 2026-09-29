// Method-id dispatch and host-call stubs for the gematik:zero C# core.
// Payloads use the *generated* WireCodec (the same code the C# SDK uses); only
// the "which method does what" table is written by hand — the Go and Rust
// cores get the equivalent from acme-wit-gen.
using Acme.Client.Runtime;
using Gematik.Zero;

namespace Acme.Core;

static class Dispatch
{
    // Method ids: packages/zero/wit/abi-ids.lock
    private const uint Ctor = 0x0200, Thumbprint = 0x0201, CreateProof = 0x0202;

    private static MsgPackReader Params(byte[] request, int n)
    {
        var r = new MsgPackReader(request);
        var got = r.ReadArrayHeader();
        if (got != n) throw new AcmeProtocolException($"expected {n} parameters, found {got}");
        return r;
    }

    private static void Done(MsgPackReader r)
    {
        if (r.Remaining != 0) throw new AcmeProtocolException($"{r.Remaining} trailing bytes after payload");
    }

    /// <summary>result&lt;string, zero-error&gt; → [0, s] | [1, E]</summary>
    private static byte[] Result((string? Ok, ZeroError? Error) v) => Wire.Encode(w =>
    {
        w.WriteArrayHeader(2);
        if (v.Error is not null)
        {
            w.WriteUInt(1);
            WireCodec.WriteZeroError(w, v.Error);
        }
        else
        {
            w.WriteUInt(0);
            w.WriteString(v.Ok!);
        }
    });

    public static (int, byte[]?) Sync(ulong obj, uint method, byte[] request)
    {
        switch (method)
        {
            case Ctor:
            {
                if (obj != 0) return (Status.InvalidHandle, null);
                var r = Params(request, 1);
                var keyId = r.ReadString();
                Done(r);
                var handle = Handles.Add(Handles.Object, new DpopSigner(keyId));
                return (Status.Ok, Wire.Encode(w => w.WriteUInt(handle)));
            }
            case Thumbprint or CreateProof:
                return (Handles.Get<DpopSigner>(obj) is null ? Status.InvalidHandle : Status.Unsupported, null);
            default:
                return (Status.UnknownMethod, null);
        }
    }

    public static (int, Func<CancellationToken, Task<byte[]>>?) Async(ulong obj, uint method, byte[] request)
    {
        if (method is Ctor) return (Status.Unsupported, null);
        if (method is not (Thumbprint or CreateProof)) return (Status.UnknownMethod, null);
        if (Handles.Get<DpopSigner>(obj) is not { } s) return (Status.InvalidHandle, null);
        if (method == Thumbprint)
        {
            Done(Params(request, 0));
            return (Status.Ok, async ct => Result(await s.ThumbprintAsync(ct).ConfigureAwait(false)));
        }
        var r = Params(request, 4);
        var httpMethod = r.ReadString();
        var httpUrl = r.ReadString();
        var accessToken = r.TryReadNil() ? null : r.ReadString();
        var nonce = r.TryReadNil() ? null : r.ReadString();
        Done(r);
        return (Status.Ok, async ct => Result(await s.CreateProofAsync(httpMethod, httpUrl, accessToken, nonce, ct).ConfigureAwait(false)));
    }
}

/// <summary>WIT import `host-keys`: implemented by the APP, reached through host calls.</summary>
static class HostKeys
{
    private const uint PublicKey = 0x0101, Sign = 0x0102;

    public static Task<(byte[]? Ok, ZeroError? Error)> PublicKeyAsync(string keyId, CancellationToken ct) =>
        CallAsync(PublicKey, w => { w.WriteArrayHeader(1); w.WriteString(keyId); }, ct);

    public static Task<(byte[]? Ok, ZeroError? Error)> SignAsync(string keyId, byte[] digest, CancellationToken ct) =>
        CallAsync(Sign, w => { w.WriteArrayHeader(2); w.WriteString(keyId); w.WriteBinary(digest); }, ct);

    /// <summary>result&lt;list&lt;u8&gt;, zero-error&gt;; HostCallException on transport failures.</summary>
    private static async Task<(byte[]?, ZeroError?)> CallAsync(uint method, Action<MsgPackWriter> args, CancellationToken ct)
    {
        var answer = await HostCalls.CallAsync(method, Wire.Encode(args), ct).ConfigureAwait(false);
        try
        {
            var r = new MsgPackReader(answer);
            if (r.ReadArrayHeader() != 2) throw new AcmeProtocolException("result must be [tag, value]");
            (byte[]?, ZeroError?) v = r.ReadUInt64() == 0 ? (r.ReadBinary(), null) : (null, WireCodec.ReadZeroError(r));
            Done(r);
            return v;
        }
        catch (AcmeProtocolException e)
        {
            throw new HostCallException(Status.InvalidArgument, e.Message);
        }
    }

    private static void Done(MsgPackReader r)
    {
        if (r.Remaining != 0) throw new AcmeProtocolException("trailing bytes in host answer");
    }
}

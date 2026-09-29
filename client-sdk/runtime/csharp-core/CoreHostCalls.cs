// Hand-written, contract-agnostic: ABI 1.1 host calls (ABI.md §4a) for C#
// cores. Only compiled into cores whose package has WIT imports: it adds the
// exports <prefix>_host_register / <prefix>_host_reply (set HostCallsEnabled).
using System.Collections.Concurrent;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace Acme.Core;

/// <summary>A host call that produced no return value (no handler, cancelled, error status, bad answer).</summary>
sealed class HostCallException(int status, string message) : Exception(message)
{
    public int Status { get; } = status;
}

static class HostCalls
{
    private const ulong KindHostCall = 4;

    private sealed class Pending(nint request)
    {
        public nint Request = request; // native copy, valid until the reply (ABI contract)
        public readonly TaskCompletionSource<(int Status, byte[] Payload)> Reply =
            new(TaskCreationOptions.RunContinuationsAsynchronously);
    }

    private static readonly object Gate = new();
    private static nint _handler, _userData;
    private static readonly ConcurrentDictionary<ulong, Pending> PendingCalls = new();

    public static int Register(nint handler, nint userData)
    {
        lock (Gate) (_handler, _userData) = (handler, userData);
        return Status.Ok;
    }

    /// <summary>
    /// Runs import function <paramref name="methodId"/> in the app and waits for the
    /// answer. Only call it from async work (never inside a sync dispatch).
    /// Cancellation abandons the call; the request stays valid until the reply anyway.
    /// </summary>
    public static async Task<byte[]> CallAsync(uint methodId, byte[] request, CancellationToken ct)
    {
        var pending = Start(methodId, request);
        (int status, byte[] payload) answer;
        using (ct.Register(() => pending.Reply.TrySetCanceled()))
        {
            try
            {
                answer = await pending.Reply.Task.ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                throw new HostCallException(Status.Cancelled, "cancelled");
            }
        }
        if (answer.status != Status.Ok)
            throw new HostCallException(answer.status, System.Text.Encoding.UTF8.GetString(answer.payload));
        return answer.payload;
    }

    private static unsafe Pending Start(uint methodId, byte[] request)
    {
        nint handler, userData;
        lock (Gate) (handler, userData) = (_handler, _userData);
        if (handler == 0) throw new HostCallException(Status.Unsupported, "no host handler registered (host_register)");
        var copy = (byte*)NativeMemory.Alloc((nuint)Math.Max(request.Length, 1));
        request.CopyTo(new Span<byte>(copy, request.Length));
        var pending = new Pending((nint)copy);
        var call = Handles.NextId(KindHostCall);
        PendingCalls[call] = pending;
        ((delegate* unmanaged[Cdecl]<void*, uint, byte*, nuint, ulong, void>)handler)(
            (void*)userData, methodId, copy, (nuint)request.Length, call);
        return pending;
    }

    public static unsafe int Reply(ulong call, int status, AcmeBytes response)
    {
        if (Exports.FromBytes(response) is not { } payload) return Status.InvalidArgument;
        if (!PendingCalls.TryRemove(call, out var pending)) return Status.InvalidHandle;
        NativeMemory.Free((void*)pending.Request); // the app is done with the request now
        pending.Reply.TrySetResult((status, payload)); // no-op if the caller gave up
        return Status.Ok;
    }
}

static unsafe partial class Exports
{
    [UnmanagedCallersOnly(EntryPoint = Prefix + "host_register", CallConvs = [typeof(CallConvCdecl)])]
    public static int HostRegister(nint handler, nint userData) => HostCalls.Register(handler, userData);

    [UnmanagedCallersOnly(EntryPoint = Prefix + "host_reply", CallConvs = [typeof(CallConvCdecl)])]
    public static int HostReply(ulong call, int status, AcmeBytes response)
    {
        try
        {
            return HostCalls.Reply(call, status, response);
        }
        catch
        {
            return Status.Internal; // never unwind into native code
        }
    }
}

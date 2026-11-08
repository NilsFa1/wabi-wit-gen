// Hand-written, contract-agnostic: host calls (ABI 1.1 and 1.2, ABI.md §4a) for
// C# cores. Only compiled into cores whose package has WIT imports: it adds the
// exports <prefix>_host_register, <prefix>_host_register2 and <prefix>_host_reply
// (set HostCallsEnabled).
using System.Collections.Concurrent;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace Wabi.Core;

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

    // Readers hold the lock while they run the handler, so that registering (the
    // writer) returns only once no invocation of the old handler is running.
    private static readonly ReaderWriterLockSlim Registered = new();
    private static nint _handler, _cancel, _userData;
    private static bool _v2;
    private static readonly ConcurrentDictionary<ulong, Pending> PendingCalls = new();

    /// <summary>ABI 1.1 (<paramref name="v2"/> false) or 1.2: the handler learns the object, onCancel abandoned calls.</summary>
    public static int Register(nint handler, nint onCancel, nint userData, bool v2)
    {
        Registered.EnterWriteLock();
        try
        {
            (_handler, _cancel, _userData, _v2) = (handler, onCancel, userData, v2);
        }
        finally
        {
            Registered.ExitWriteLock();
        }
        return Status.Ok;
    }

    /// <summary>
    /// Runs import function <paramref name="methodId"/> in the app and waits for the
    /// answer. Only call it from async work (never inside a sync dispatch).
    /// Cancellation abandons the call and tells the app; the request stays valid
    /// until the reply anyway.
    /// </summary>
    public static async Task<byte[]> CallAsync(uint methodId, byte[] request, CancellationToken ct)
    {
        var (call, pending) = Start(methodId, request);
        (int status, byte[] payload) answer;
        using (ct.Register(() => pending.Reply.TrySetCanceled()))
        {
            try
            {
                answer = await pending.Reply.Task.ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                // Never from inside an ABI call of the host: on a pool thread.
                _ = Task.Run(() => NotifyAbandoned(call));
                throw new HostCallException(Status.Cancelled, "cancelled");
            }
        }
        if (answer.status != Status.Ok)
            throw new HostCallException(answer.status, System.Text.Encoding.UTF8.GetString(answer.payload));
        return answer.payload;
    }

    private static unsafe (ulong, Pending) Start(uint methodId, byte[] request)
    {
        var obj = Runner.CurrentObject.Value;
        Registered.EnterReadLock();
        try
        {
            if (_handler == 0) throw new HostCallException(Status.Unsupported, "no host handler registered (host_register)");
            var copy = (byte*)NativeMemory.Alloc((nuint)Math.Max(request.Length, 1));
            request.CopyTo(new Span<byte>(copy, request.Length));
            var pending = new Pending((nint)copy);
            var call = Handles.NextId(KindHostCall);
            PendingCalls[call] = pending;
            if (_v2)
                ((delegate* unmanaged[Cdecl]<void*, ulong, uint, byte*, nuint, ulong, void>)_handler)(
                    (void*)_userData, obj, methodId, copy, (nuint)request.Length, call);
            else
                ((delegate* unmanaged[Cdecl]<void*, uint, byte*, nuint, ulong, void>)_handler)(
                    (void*)_userData, methodId, copy, (nuint)request.Length, call);
            return (call, pending);
        }
        finally
        {
            Registered.ExitReadLock();
        }
    }

    /// <summary>Tells the app that nobody waits for the call any more, unless it has answered already.</summary>
    private static unsafe void NotifyAbandoned(ulong call)
    {
        if (!PendingCalls.ContainsKey(call)) return;
        Registered.EnterReadLock();
        try
        {
            if (_handler != 0 && _cancel != 0)
                ((delegate* unmanaged[Cdecl]<void*, ulong, void>)_cancel)((void*)_userData, call);
        }
        finally
        {
            Registered.ExitReadLock();
        }
    }

    public static unsafe int Reply(ulong call, int status, WabiBytes response)
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
    public static int HostRegister(nint handler, nint userData) => HostCalls.Register(handler, 0, userData, v2: false);

    [UnmanagedCallersOnly(EntryPoint = Prefix + "host_register2", CallConvs = [typeof(CallConvCdecl)])]
    public static int HostRegister2(nint handler, nint onCancel, nint userData) => HostCalls.Register(handler, onCancel, userData, v2: true);

    [UnmanagedCallersOnly(EntryPoint = Prefix + "host_reply", CallConvs = [typeof(CallConvCdecl)])]
    public static int HostReply(ulong call, int status, WabiBytes response)
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

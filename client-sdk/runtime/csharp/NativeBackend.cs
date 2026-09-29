// Hand-written, generic: executes contract calls through the Acme native ABI.
// Contains no per-method code — every WIT function goes through CallSync /
// CallAsync / ReadStream with its method id.
#nullable enable
using System;
using System.Collections.Generic;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;

namespace Acme.Client.Runtime;

public sealed class NativeBackend : IClientBackend
{
    /// <summary>
    /// Capabilities this SDK relies on. Cancellation and ThreadedCallback are informative:
    /// acme_future_cancel is always callable, and an eager core (futures born complete)
    /// has nothing to interrupt.
    /// </summary>
    public const AcmeCapabilities Required =
        AcmeCapabilities.SyncCalls | AcmeCapabilities.AsyncCalls | AcmeCapabilities.Streams |
        AcmeCapabilities.PayloadMsgpack1;

    /// <summary>Environment variable with an explicit path to the core library (tests, side-by-side cores).</summary>
    public const string LibraryPathVariable = "ACME_NATIVE_LIB";

    private static readonly Lazy<NativeBackend> s_shared = new(() => new NativeBackend(), LazyThreadSafetyMode.ExecutionAndPublication);

    /// <summary>The process-wide native backend. Validated on first use.</summary>
    public static NativeBackend Shared => s_shared.Value;

    public CoreInfo Info { get; }
    public string Description => $"native: {Info}";

    static NativeBackend()
    {
        NativeLibrary.SetDllImportResolver(typeof(NativeBackend).Assembly, Resolve);
    }

    private static IntPtr Resolve(string name, Assembly assembly, DllImportSearchPath? path)
    {
        if (name != NativeAbi.Library) return IntPtr.Zero;
        var explicitPath = Environment.GetEnvironmentVariable(LibraryPathVariable);
        return string.IsNullOrEmpty(explicitPath) ? IntPtr.Zero : NativeLibrary.Load(explicitPath);
    }

    private NativeBackend()
    {
        Info = new CoreInfo(
            NativeAbi.acme_abi_version(),
            (AcmeCapabilities)NativeAbi.acme_abi_capabilities(),
            Marshal.PtrToStringUTF8(NativeAbi.acme_contract_id()) ?? "",
            Marshal.PtrToStringUTF8(NativeAbi.acme_core_info()) ?? "");
        Info.EnsureCompatible();
    }

    // ------------------------------------------------------------ buffers

    /// <summary>Copies a core-owned buffer into managed memory and frees it (always).</summary>
    private static unsafe byte[] Consume(AcmeBuffer buf)
    {
        try
        {
            return buf.Len == 0 ? Array.Empty<byte>() : new ReadOnlySpan<byte>(buf.Ptr, checked((int)buf.Len)).ToArray();
        }
        finally
        {
            NativeAbi.acme_buffer_free(buf);
        }
    }

    private static AcmeNativeException Error(int status, byte[]? detail = null) =>
        new(status, Marshal.PtrToStringUTF8(NativeAbi.acme_status_name(status)) ?? status.ToString(),
            detail is { Length: > 0 } ? System.Text.Encoding.UTF8.GetString(detail) : null);

    // -------------------------------------------------------------- calls

    public unsafe byte[] CallSync(ulong obj, uint methodId, byte[] request)
    {
        AcmeBuffer response;
        int status;
        fixed (byte* p = request)
            status = NativeAbi.acme_call_sync(obj, methodId, new AcmeBytes(p, (nuint)request.Length), &response);
        var bytes = Consume(response);
        return status == AcmeStatus.Ok ? bytes : throw Error(status, bytes);
    }

    public Task<byte[]> CallAsync(ulong obj, uint methodId, byte[] request, CancellationToken cancellationToken)
    {
        // .NET convention: an already-cancelled token never starts the operation
        // (otherwise the outcome would depend on the core's scheduling).
        if (cancellationToken.IsCancellationRequested) return Task.FromCanceled<byte[]>(cancellationToken);
        var (status, future) = StartCall(obj, methodId, request);
        if (status != AcmeStatus.Ok) return Task.FromException<byte[]>(Error(status));
        return CompleteAsync(future, cancellationToken);
    }

    private static unsafe (int, ulong) StartCall(ulong obj, uint methodId, byte[] request)
    {
        ulong future;
        int status;
        fixed (byte* p = request)
            status = NativeAbi.acme_call_async(obj, methodId, new AcmeBytes(p, (nuint)request.Length), &future);
        return (status, future);
    }

    private static async Task<byte[]> CompleteAsync(ulong future, CancellationToken ct)
    {
        try
        {
            await WhenReady(future, ct).ConfigureAwait(false);
            var (status, bytes) = Take(future);
            return status switch
            {
                AcmeStatus.Ok => bytes,
                AcmeStatus.ErrCancelled => throw new OperationCanceledException("operation was cancelled", ct.IsCancellationRequested ? ct : CancellationToken.None),
                _ => throw Error(status, bytes),
            };
        }
        finally
        {
            NativeAbi.acme_future_drop(future);
        }
    }

    private static unsafe (int Status, byte[] Bytes) Take(ulong future)
    {
        AcmeBuffer buf;
        int status = NativeAbi.acme_future_take(future, &buf);
        return (status, Consume(buf));
    }

    // ------------------------------------------------------------ futures

    /// <summary>
    /// Completes when the native future is ready. Cancellation of <paramref name="ct"/>
    /// is forwarded to the core with acme_future_cancel; the task still waits for
    /// the core's completion callback, so the future's outcome is always observed
    /// (cancel racing with completion: whichever the core saw first wins).
    /// </summary>
    private static async Task WhenReady(ulong future, CancellationToken ct)
    {
        var tcs = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        var gch = GCHandle.Alloc(tcs);
        int status = Subscribe(future, GCHandle.ToIntPtr(gch));
        if (status != AcmeStatus.Ok)
        {
            gch.Free(); // the callback will never run
            if (status == AcmeStatus.Ready) return;
            throw Error(status);
        }
        // UnsafeRegister: no ExecutionContext capture; the callback just forwards to the core.
        using (ct.UnsafeRegister(static s => NativeAbi.acme_future_cancel((ulong)s!), future))
            await tcs.Task.ConfigureAwait(false);
    }

    private static unsafe int Subscribe(ulong future, IntPtr userData) =>
        NativeAbi.acme_future_subscribe(future, (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, ulong, void>)&OnReady, userData);

    /// <summary>
    /// Invoked by the core on one of ITS threads, exactly once per successful subscribe.
    /// Must not block: it only frees the GC handle and signals the task; continuations
    /// run on the thread pool (RunContinuationsAsynchronously), never on the core thread.
    /// No SynchronizationContext is involved — UI apps marshal to their main thread
    /// themselves (await resumes on the captured context if they do not ConfigureAwait(false)).
    /// </summary>
    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static void OnReady(IntPtr userData, ulong future)
    {
        try
        {
            var handle = GCHandle.FromIntPtr(userData);
            var tcs = (TaskCompletionSource)handle.Target!;
            handle.Free();
            tcs.TrySetResult();
        }
        catch
        {
            // Never let an exception unwind into native code.
        }
    }

    // ------------------------------------------------------------ streams

    public async IAsyncEnumerable<byte[]> ReadStream(ulong stream, [EnumeratorCancellation] CancellationToken cancellationToken)
    {
        try
        {
            while (true)
            {
                var (status, bytes) = await NextAsync(stream, cancellationToken).ConfigureAwait(false);
                if (status == AcmeStatus.StreamEnd) yield break;
                if (status == AcmeStatus.ErrCancelled)
                {
                    cancellationToken.ThrowIfCancellationRequested();
                    yield break; // cancelled by the core (e.g. object disposed)
                }
                if (status != AcmeStatus.Ok) throw Error(status, bytes);
                yield return bytes;
            }
        }
        finally
        {
            NativeAbi.acme_stream_cancel(stream);
            NativeAbi.acme_stream_drop(stream);
        }
    }

    private static async Task<(int, byte[])> NextAsync(ulong stream, CancellationToken ct)
    {
        var future = StreamNext(stream);
        try
        {
            await WhenReady(future, ct).ConfigureAwait(false);
            return Take(future);
        }
        finally
        {
            NativeAbi.acme_future_drop(future);
        }
    }

    private static unsafe ulong StreamNext(ulong stream)
    {
        ulong future;
        int status = NativeAbi.acme_stream_next(stream, &future);
        return status == AcmeStatus.Ok ? future : throw Error(status);
    }

    // ------------------------------------------------------------ objects

    public void DropObject(ulong obj) => NativeAbi.acme_object_drop(obj);
}

/// <summary>Identity and version of a loaded core, plus the compatibility rule.</summary>
public sealed record CoreInfo(uint AbiVersion, AcmeCapabilities Capabilities, string ContractId, string Description)
{
    public uint AbiMajor => AbiVersion >> 16;
    public uint AbiMinor => AbiVersion & 0xffff;

    public override string ToString() => $"{Description} (abi {AbiMajor}.{AbiMinor}, {ContractId})";

    /// <summary>
    /// ABI: same major, core minor &gt;= SDK minor. Contract: same WIT package, same major,
    /// core minor &gt;= SDK minor (a newer core may add functions; ids never change).
    /// </summary>
    public void EnsureCompatible()
    {
        if (AbiMajor != NativeAbi.AbiMajor || AbiMinor < NativeAbi.AbiMinor)
            throw new AcmeIncompatibleLibraryException(
                $"native core '{Description}' implements ABI {AbiMajor}.{AbiMinor}, this SDK needs {NativeAbi.AbiMajor}.{NativeAbi.AbiMinor}+");
        var missing = NativeBackend.Required & ~Capabilities;
        if (missing != 0)
            throw new AcmeIncompatibleLibraryException($"native core '{Description}' lacks capabilities: {missing}");
        var at = ContractId.IndexOf('@');
        var pkg = at < 0 ? ContractId : ContractId[..at];
        var ver = at < 0 ? Array.Empty<string>() : ContractId[(at + 1)..].Split('.');
        if (pkg != Contract.Package || ver.Length < 2 ||
            !int.TryParse(ver[0], out var major) || !int.TryParse(ver[1], out var minor) ||
            major != Contract.Major || minor < Contract.Minor)
            throw new AcmeIncompatibleLibraryException(
                $"native core implements contract '{ContractId}', this SDK was generated for '{Contract.Id}'");
    }

    /// <summary>Reads the metadata of an arbitrary core library without binding the SDK to it.</summary>
    public static unsafe CoreInfo Probe(string libraryPath)
    {
        var lib = NativeLibrary.Load(libraryPath);
        try
        {
            var version = ((delegate* unmanaged[Cdecl]<uint>)NativeLibrary.GetExport(lib, "acme_abi_version"))();
            var caps = ((delegate* unmanaged[Cdecl]<ulong>)NativeLibrary.GetExport(lib, "acme_abi_capabilities"))();
            var contract = ((delegate* unmanaged[Cdecl]<IntPtr>)NativeLibrary.GetExport(lib, "acme_contract_id"))();
            var info = ((delegate* unmanaged[Cdecl]<IntPtr>)NativeLibrary.GetExport(lib, "acme_core_info"))();
            return new CoreInfo(version, (AcmeCapabilities)caps, Marshal.PtrToStringUTF8(contract) ?? "", Marshal.PtrToStringUTF8(info) ?? "");
        }
        finally
        {
            NativeLibrary.Free(lib);
        }
    }
}

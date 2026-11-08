// Hand-written, generic: host calls (ABI 1.2, ABI.md §4a). The core calls app
// code for the functions of WIT *imports*; the generated <Package>Host class
// decodes the request and calls the app's interface implementation.
//
// An implementation is registered for the whole package or for one instance
// (object); the core names the instance with every call. If the core gives up
// on a call, the token passed to the implementation is cancelled.
#nullable enable
using System;
using System.Collections.Concurrent;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

namespace Wabi.Client.Runtime;

/// <summary>Runs one import function: method id and encoded parameters in, encoded return value out.</summary>
public delegate Task<byte[]> HostDispatcher(uint methodId, byte[] request, CancellationToken cancellationToken);

public static class HostCalls
{
    private static readonly object s_gate = new();
    private static HostDispatcher? s_default;
    private static readonly ConcurrentDictionary<ulong, HostDispatcher> s_perObject = new();
    private static bool s_registered;
    /// <summary>Calls the app is working on: cancelled when the core gives up on them.</summary>
    private static readonly ConcurrentDictionary<ulong, CancellationTokenSource> s_running = new();

    /// <summary>
    /// Installs the dispatcher for all imports of this package (one per process;
    /// a later call replaces it). Loads and validates the core first.
    /// </summary>
    public static void Install(HostDispatcher dispatch)
    {
        ArgumentNullException.ThrowIfNull(dispatch);
        lock (s_gate)
        {
            s_default = dispatch;
            Sync();
        }
    }

    /// <summary>
    /// Installs the dispatcher for the calls the core makes on behalf of one
    /// object; it takes precedence over the package-wide one and goes away
    /// with the object.
    /// </summary>
    public static void Install(ulong obj, HostDispatcher dispatch)
    {
        ArgumentNullException.ThrowIfNull(dispatch);
        lock (s_gate)
        {
            s_perObject[obj] = dispatch;
            Sync();
        }
    }

    /// <summary>Removes the package-wide dispatcher; calls already running finish with it.</summary>
    public static void Uninstall()
    {
        lock (s_gate)
        {
            s_default = null;
            Sync();
        }
    }

    /// <summary>Removes the dispatcher of one object (no-op if it has none).</summary>
    public static void Uninstall(ulong obj)
    {
        if (s_perObject.IsEmpty) return; // the common case when objects are dropped
        lock (s_gate)
        {
            if (s_perObject.TryRemove(obj, out _)) Sync();
        }
    }

    /// <summary>Registers with the core while there is any dispatcher, unregisters when the last one is gone. Caller holds s_gate.</summary>
    private static unsafe void Sync()
    {
        bool wanted = s_default is not null || !s_perObject.IsEmpty;
        if (wanted == s_registered) return;
        if (!wanted)
        {
            // Returns once no handler invocation is running; later host calls fail inside the core.
            NativeAbi.wabi_host_register2(IntPtr.Zero, IntPtr.Zero, IntPtr.Zero);
            s_registered = false;
            return;
        }
        var info = NativeBackend.Shared.Info;
        if ((info.Capabilities & WabiCapabilities.HostCalls2) == 0)
        {
            s_default = null;
            s_perObject.Clear();
            throw new WabiIncompatibleLibraryException($"native core '{info.Description}' does not support host calls (ABI 1.2)");
        }
        var handler = (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, ulong, uint, byte*, nuint, ulong, void>)&OnHostCall;
        var cancel = (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, ulong, void>)&OnHostCancel;
        int status = NativeAbi.wabi_host_register2(handler, cancel, IntPtr.Zero);
        if (status != WabiStatus.Ok) throw new WabiNativeException(status, "host_register2 failed", null);
        s_registered = true;
    }

    /// <summary>
    /// Invoked by the core on one of ITS threads. Must return quickly: copies the
    /// request and runs the app code on the thread pool; the answer goes back
    /// later with wabi_host_reply (exactly once, whatever happens).
    /// </summary>
    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnHostCall(IntPtr userData, ulong obj, uint methodId, byte* request, nuint requestLen, ulong call)
    {
        try
        {
            var req = requestLen == 0 ? Array.Empty<byte>() : new ReadOnlySpan<byte>(request, checked((int)requestLen)).ToArray();
            // The implementation is chosen now: a call keeps it even if it is replaced meanwhile.
            HostDispatcher? dispatch = s_perObject.TryGetValue(obj, out var own) ? own : Volatile.Read(ref s_default);
            var cts = new CancellationTokenSource();
            s_running[call] = cts; // before the core can cancel the call
            _ = Task.Run(() => RunAsync(dispatch, methodId, req, call, cts));
        }
        catch (Exception e)
        {
            s_running.TryRemove(call, out _);
            Reply(call, WabiStatus.ErrInternal, Encoding.UTF8.GetBytes(e.Message)); // never unwind into native code
        }
    }

    /// <summary>Invoked by the core on one of ITS threads: it no longer waits for the call.</summary>
    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static void OnHostCancel(IntPtr userData, ulong call)
    {
        // Cancellation callbacks are app code: never run them on the core's thread.
        if (s_running.TryGetValue(call, out var cts))
            _ = Task.Run(() => { try { cts.Cancel(); } catch (ObjectDisposedException) { } });
    }

    private static async Task RunAsync(HostDispatcher? dispatch, uint methodId, byte[] request, ulong call, CancellationTokenSource cts)
    {
        int status = WabiStatus.Ok;
        byte[] response;
        try
        {
            if (dispatch is null) throw new WabiNativeException(WabiStatus.ErrUnsupported, "no host handler", null);
            response = await dispatch(methodId, request, cts.Token).ConfigureAwait(false);
        }
        catch (WabiNativeException e)
        {
            status = e.Status;
            response = Encoding.UTF8.GetBytes(e.Message);
        }
        catch (OperationCanceledException) when (cts.IsCancellationRequested)
        {
            status = WabiStatus.ErrCancelled; // the core discards the answer anyway
            response = Array.Empty<byte>();
        }
        catch (Exception e) // an app bug: reported to the core, never lost
        {
            status = WabiStatus.ErrInternal;
            response = Encoding.UTF8.GetBytes($"{e.GetType().Name}: {e.Message}");
        }
        s_running.TryRemove(call, out _);
        Reply(call, status, response);
        cts.Dispose();
    }

    private static unsafe void Reply(ulong call, int status, byte[] response)
    {
        fixed (byte* p = response)
            NativeAbi.wabi_host_reply(call, status, new WabiBytes(p, (nuint)response.Length));
    }
}

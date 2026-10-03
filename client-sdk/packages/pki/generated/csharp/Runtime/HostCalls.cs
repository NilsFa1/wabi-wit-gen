// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: ABI 1.1 host calls (ABI.md §4a). The core calls app
// code for the functions of WIT *imports*; the generated <Package>Host class
// decodes the request and calls the app's interface implementation.
#nullable enable
using System;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading.Tasks;

namespace Gematik.Pki.Runtime;

public static class HostCalls
{
    private static volatile Func<uint, byte[], Task<byte[]>>? s_dispatch;

    /// <summary>
    /// Installs the dispatcher for all imports of this package (one per process;
    /// a later call replaces it). Loads and validates the core first.
    /// </summary>
    public static unsafe void Install(Func<uint, byte[], Task<byte[]>> dispatch)
    {
        ArgumentNullException.ThrowIfNull(dispatch);
        var info = NativeBackend.Shared.Info;
        if ((info.Capabilities & WabiCapabilities.HostCalls) == 0)
            throw new WabiIncompatibleLibraryException($"native core '{info.Description}' does not support host calls (ABI 1.1)");
        s_dispatch = dispatch;
        var handler = (IntPtr)(delegate* unmanaged[Cdecl]<IntPtr, uint, byte*, nuint, ulong, void>)&OnHostCall;
        int status = NativeAbi.wabi_host_register(handler, IntPtr.Zero);
        if (status != WabiStatus.Ok) throw new WabiNativeException(status, "host_register failed", null);
    }

    /// <summary>Removes the handler; later host calls fail inside the core.</summary>
    public static void Uninstall()
    {
        NativeAbi.wabi_host_register(IntPtr.Zero, IntPtr.Zero);
        s_dispatch = null;
    }

    /// <summary>
    /// Invoked by the core on one of ITS threads. Must return quickly: copies the
    /// request and runs the app code on the thread pool; the answer goes back
    /// later with wabi_host_reply (exactly once, whatever happens).
    /// </summary>
    [UnmanagedCallersOnly(CallConvs = new[] { typeof(CallConvCdecl) })]
    private static unsafe void OnHostCall(IntPtr userData, uint methodId, byte* request, nuint requestLen, ulong call)
    {
        try
        {
            var req = requestLen == 0 ? Array.Empty<byte>() : new ReadOnlySpan<byte>(request, checked((int)requestLen)).ToArray();
            _ = Task.Run(() => RunAsync(methodId, req, call));
        }
        catch (Exception e)
        {
            Reply(call, WabiStatus.ErrInternal, Encoding.UTF8.GetBytes(e.Message)); // never unwind into native code
        }
    }

    private static async Task RunAsync(uint methodId, byte[] request, ulong call)
    {
        int status = WabiStatus.Ok;
        byte[] response;
        try
        {
            var dispatch = s_dispatch ?? throw new WabiNativeException(WabiStatus.ErrUnsupported, "no host handler", null);
            response = await dispatch(methodId, request).ConfigureAwait(false);
        }
        catch (WabiNativeException e)
        {
            status = e.Status;
            response = Encoding.UTF8.GetBytes(e.Message);
        }
        catch (Exception e) // an app bug: reported to the core, never lost
        {
            status = WabiStatus.ErrInternal;
            response = Encoding.UTF8.GetBytes($"{e.GetType().Name}: {e.Message}");
        }
        Reply(call, status, response);
    }

    private static unsafe void Reply(ulong call, int status, byte[] response)
    {
        fixed (byte* p = response)
            NativeAbi.wabi_host_reply(call, status, new WabiBytes(p, (nuint)response.Length));
    }
}

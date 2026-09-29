// C# ABI adapter: implements abi/acme_client.h with [UnmanagedCallersOnly]
// exports. NativeAOT turns this into a plain C shared library. This file
// replaces core-rust/src/ffi — nothing on the SDK side changes.
using System.Collections.Concurrent;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Threading.Channels;
using Acme.Client;
using Acme.Client.Runtime;

namespace Acme.Core;

[StructLayout(LayoutKind.Sequential)]
unsafe struct AcmeBytes
{
    public byte* Ptr;
    public nuint Len;
}

[StructLayout(LayoutKind.Sequential)]
unsafe struct AcmeBuffer
{
    public byte* Ptr;
    public nuint Len;
}

static class Status
{
    public const int Ok = 0, Pending = 1, Ready = 2, StreamEnd = 3;
    public const int InvalidHandle = -1, InvalidArgument = -2, UnknownMethod = -3, Cancelled = -4,
        Busy = -5, AlreadyTaken = -6, Unsupported = -7, Internal = -8;
}

/// <summary>Handle table: kind in the top byte, never-reused serial below.</summary>
static class Handles
{
    public const ulong Object = 1, Future = 2, Stream = 3;
    private static long _serial;
    private static readonly ConcurrentDictionary<ulong, object> Table = new();

    public static ulong Add(ulong kind, object value)
    {
        var h = (kind << 56) | (ulong)Interlocked.Increment(ref _serial);
        Table[h] = value;
        return h;
    }

    public static T? Get<T>(ulong h) where T : class => Table.TryGetValue(h, out var v) ? v as T : null;

    public static T? Remove<T>(ulong h) where T : class =>
        Table.TryGetValue(h, out var v) && v is T t && Table.TryRemove(new KeyValuePair<ulong, object>(h, v)) ? t : null;
}

/// <summary>One future per async call / stream pull. First transition out of pending wins.</summary>
sealed unsafe class NativeFuture
{
    private readonly object _gate = new();
    private readonly CancellationTokenSource _cts = new();
    private bool _done, _taken, _subscribed;
    private int _status;
    private byte[]? _payload;
    private nint _callback; // delegate* unmanaged[Cdecl]<void*, ulong, void>
    private nint _userData;
    public readonly ulong Handle;

    public NativeFuture() => Handle = Handles.Add(Handles.Future, this);

    public CancellationToken Token => _cts.Token;

    /// <summary>Called on a thread-pool ("core") thread; invokes the callback inline.</summary>
    public bool Complete(int status, byte[]? payload)
    {
        nint cb, ud;
        lock (_gate)
        {
            if (_done) return false;
            (_done, _status, _payload) = (true, status, payload);
            (cb, ud, _callback) = (_callback, _userData, 0);
        }
        if (cb != 0) Invoke(cb, ud, Handle);
        return true;
    }

    private static void Invoke(nint cb, nint ud, ulong handle) =>
        ((delegate* unmanaged[Cdecl]<void*, ulong, void>)cb)((void*)ud, handle);

    /// <summary>Host-initiated: never calls back on the host's thread.</summary>
    public void CancelByHost()
    {
        nint cb, ud;
        lock (_gate)
        {
            if (_done) return;
            (_done, _status) = (true, Status.Cancelled);
            (cb, ud, _callback) = (_callback, _userData, 0);
        }
        _cts.Cancel();
        if (cb != 0)
        {
            var h = Handle;
            ThreadPool.UnsafeQueueUserWorkItem(_ => Invoke(cb, ud, h), null);
        }
    }

    public int Subscribe(delegate* unmanaged[Cdecl]<void*, ulong, void> cb, void* ud)
    {
        lock (_gate)
        {
            if (_subscribed) return Status.Busy;
            _subscribed = true;
            if (_done) return Status.Ready;
            (_callback, _userData) = ((nint)cb, (nint)ud);
            return Status.Ok;
        }
    }

    public int Poll()
    {
        lock (_gate) return _done ? Status.Ready : Status.Pending;
    }

    public (int, byte[]?) Take()
    {
        lock (_gate)
        {
            if (!_done) return (Status.Pending, null);
            if (_taken) return (Status.AlreadyTaken, null);
            _taken = true;
            var p = _payload;
            _payload = null;
            return (_status, p);
        }
    }
}

/// <summary>Pull-based stream over a channel of events; items encoded lazily.</summary>
sealed class NativeStream(ChannelReader<ClientEvent> events, Action release)
{
    private readonly CancellationTokenSource _stop = new();
    private readonly ConcurrentQueue<byte[]> _leftover = new(); // items dequeued by a pull that lost to cancel
    private int _busy;
    private int _released;

    public (int, ulong) Next()
    {
        if (Interlocked.Exchange(ref _busy, 1) == 1) return (Status.Busy, 0);
        var f = new NativeFuture();
        if (_stop.IsCancellationRequested)
        {
            Volatile.Write(ref _busy, 0);
            f.Complete(Status.StreamEnd, null);
            return (Status.Ok, f.Handle);
        }
        if (_leftover.TryDequeue(out var early))
        {
            Volatile.Write(ref _busy, 0);
            f.Complete(Status.Ok, early);
            return (Status.Ok, f.Handle);
        }
        _ = Task.Run(async () =>
        {
            int status;
            byte[]? item = null;
            try
            {
                using var any = CancellationTokenSource.CreateLinkedTokenSource(_stop.Token, f.Token);
                while (true)
                {
                    if (events.TryRead(out var e))
                    {
                        item = Wire.Encode(w => WireCodec.WriteClientEvent(w, e));
                        status = Status.Ok;
                        break;
                    }
                    if (!await events.WaitToReadAsync(any.Token).ConfigureAwait(false))
                    {
                        status = _stop.IsCancellationRequested ? Status.Cancelled : Status.StreamEnd;
                        break;
                    }
                }
            }
            catch (OperationCanceledException)
            {
                if (!_stop.IsCancellationRequested)
                {
                    Volatile.Write(ref _busy, 0); // only this pull was cancelled; nothing consumed
                    return;
                }
                status = Status.Cancelled;
            }
            Volatile.Write(ref _busy, 0);
            if (!f.Complete(status, item) && status == Status.Ok) _leftover.Enqueue(item!);
        });
        return (Status.Ok, f.Handle);
    }

    public void Cancel()
    {
        if (Interlocked.Exchange(ref _released, 1) == 1) return;
        _stop.Cancel();
        release();
    }
}

/// <summary>Runs async work on the thread pool (kept out of the unsafe export class).</summary>
static class Runner
{
    public static ulong Start(Func<CancellationToken, Task<byte[]>> work)
    {
        var f = new NativeFuture();
        _ = Task.Run(async () =>
        {
            try
            {
                f.Complete(Status.Ok, await work(f.Token).ConfigureAwait(false));
            }
            catch (Exception e)
            {
                f.Complete(Status.Internal, System.Text.Encoding.UTF8.GetBytes(e.Message));
            }
        });
        return f.Handle;
    }
}

static class Wire
{
    public static byte[] Encode(Action<MsgPackWriter> body)
    {
        var w = new MsgPackWriter();
        body(w);
        return w.ToArray();
    }
}

static unsafe class Exports
{
    private static readonly byte* ContractId = Utf8("acme:client@1.0.0");
    private static readonly byte* CoreInfo = Utf8("acme-csharp-core/0.1.0 (NativeAOT, thread pool)");
    private static readonly Dictionary<int, nint> StatusNames = new()
    {
        [Status.Ok] = (nint)Utf8("ACME_OK"), [Status.Pending] = (nint)Utf8("ACME_PENDING"),
        [Status.Ready] = (nint)Utf8("ACME_READY"), [Status.StreamEnd] = (nint)Utf8("ACME_STREAM_END"),
        [Status.InvalidHandle] = (nint)Utf8("ACME_ERR_INVALID_HANDLE"),
        [Status.InvalidArgument] = (nint)Utf8("ACME_ERR_INVALID_ARGUMENT"),
        [Status.UnknownMethod] = (nint)Utf8("ACME_ERR_UNKNOWN_METHOD"), [Status.Cancelled] = (nint)Utf8("ACME_ERR_CANCELLED"),
        [Status.Busy] = (nint)Utf8("ACME_ERR_BUSY"), [Status.AlreadyTaken] = (nint)Utf8("ACME_ERR_ALREADY_TAKEN"),
        [Status.Unsupported] = (nint)Utf8("ACME_ERR_UNSUPPORTED"), [Status.Internal] = (nint)Utf8("ACME_ERR_INTERNAL"),
    };
    private static readonly byte* UnknownStatus = Utf8("ACME_UNKNOWN_STATUS");

    /// <summary>Static, NUL-terminated, never freed (process lifetime).</summary>
    private static byte* Utf8(string s) => (byte*)Marshal.StringToCoTaskMemUTF8(s);

    private static AcmeBuffer ToBuffer(byte[]? data)
    {
        if (data is null || data.Length == 0) return default;
        var p = (byte*)NativeMemory.Alloc((nuint)data.Length);
        data.CopyTo(new Span<byte>(p, data.Length));
        return new AcmeBuffer { Ptr = p, Len = (nuint)data.Length };
    }

    private static byte[]? FromBytes(AcmeBytes b) =>
        b.Len == 0 ? Array.Empty<byte>() : b.Ptr == null ? null : new ReadOnlySpan<byte>(b.Ptr, checked((int)b.Len)).ToArray();

    // ------------------------------------------------------------ metadata

    [UnmanagedCallersOnly(EntryPoint = "acme_abi_version", CallConvs = [typeof(CallConvCdecl)])]
    public static uint AbiVersion() => 1u << 16;

    // sync + async + streams + cancellation + threaded callbacks + msgpack v1
    [UnmanagedCallersOnly(EntryPoint = "acme_abi_capabilities", CallConvs = [typeof(CallConvCdecl)])]
    public static ulong Capabilities() => 0b111111;

    [UnmanagedCallersOnly(EntryPoint = "acme_contract_id", CallConvs = [typeof(CallConvCdecl)])]
    public static byte* Contract() => ContractId;

    [UnmanagedCallersOnly(EntryPoint = "acme_core_info", CallConvs = [typeof(CallConvCdecl)])]
    public static byte* Info() => CoreInfo;

    [UnmanagedCallersOnly(EntryPoint = "acme_status_name", CallConvs = [typeof(CallConvCdecl)])]
    public static byte* StatusName(int status) => StatusNames.TryGetValue(status, out var p) ? (byte*)p : UnknownStatus;

    [UnmanagedCallersOnly(EntryPoint = "acme_buffer_free", CallConvs = [typeof(CallConvCdecl)])]
    public static void BufferFree(AcmeBuffer b)
    {
        if (b.Ptr != null) NativeMemory.Free(b.Ptr);
    }

    // --------------------------------------------------------------- calls

    [UnmanagedCallersOnly(EntryPoint = "acme_call_sync", CallConvs = [typeof(CallConvCdecl)])]
    public static int CallSync(ulong obj, uint method, AcmeBytes request, AcmeBuffer* output)
    {
        if (output == null) return Status.InvalidArgument;
        *output = default;
        try
        {
            if (FromBytes(request) is not { } req) return Status.InvalidArgument;
            var (status, payload) = Dispatch.Sync(obj, method, req);
            *output = ToBuffer(payload);
            return status;
        }
        catch (Exception e)
        {
            *output = ToBuffer(System.Text.Encoding.UTF8.GetBytes(e.Message));
            return e is AcmeProtocolException ? Status.InvalidArgument : Status.Internal;
        }
    }

    [UnmanagedCallersOnly(EntryPoint = "acme_call_async", CallConvs = [typeof(CallConvCdecl)])]
    public static int CallAsync(ulong obj, uint method, AcmeBytes request, ulong* future)
    {
        if (future == null) return Status.InvalidArgument;
        *future = 0;
        try
        {
            if (FromBytes(request) is not { } req) return Status.InvalidArgument;
            var (status, work) = Dispatch.Async(obj, method, req);
            if (status != Status.Ok) return status;
            *future = Runner.Start(work!);
            return Status.Ok;
        }
        catch (AcmeProtocolException)
        {
            return Status.InvalidArgument;
        }
        catch (Exception)
        {
            return Status.Internal;
        }
    }

    [UnmanagedCallersOnly(EntryPoint = "acme_object_drop", CallConvs = [typeof(CallConvCdecl)])]
    public static int ObjectDrop(ulong obj)
    {
        if (Handles.Remove<Client>(obj) is not { } c) return Status.InvalidHandle;
        c.Close();
        return Status.Ok;
    }

    // ------------------------------------------------------------- futures

    [UnmanagedCallersOnly(EntryPoint = "acme_future_subscribe", CallConvs = [typeof(CallConvCdecl)])]
    public static int Subscribe(ulong h, delegate* unmanaged[Cdecl]<void*, ulong, void> cb, void* ud)
    {
        if (cb == null) return Status.InvalidArgument;
        return Handles.Get<NativeFuture>(h)?.Subscribe(cb, ud) ?? Status.InvalidHandle;
    }

    [UnmanagedCallersOnly(EntryPoint = "acme_future_poll", CallConvs = [typeof(CallConvCdecl)])]
    public static int Poll(ulong h) => Handles.Get<NativeFuture>(h)?.Poll() ?? Status.InvalidHandle;

    [UnmanagedCallersOnly(EntryPoint = "acme_future_take", CallConvs = [typeof(CallConvCdecl)])]
    public static int Take(ulong h, AcmeBuffer* output)
    {
        if (output == null) return Status.InvalidArgument;
        *output = default;
        if (Handles.Get<NativeFuture>(h) is not { } f) return Status.InvalidHandle;
        var (status, payload) = f.Take();
        *output = ToBuffer(payload);
        return status;
    }

    [UnmanagedCallersOnly(EntryPoint = "acme_future_cancel", CallConvs = [typeof(CallConvCdecl)])]
    public static int Cancel(ulong h)
    {
        if (Handles.Get<NativeFuture>(h) is not { } f) return Status.InvalidHandle;
        f.CancelByHost();
        return Status.Ok;
    }

    [UnmanagedCallersOnly(EntryPoint = "acme_future_drop", CallConvs = [typeof(CallConvCdecl)])]
    public static int FutureDrop(ulong h)
    {
        if (Handles.Remove<NativeFuture>(h) is not { } f) return Status.InvalidHandle;
        f.CancelByHost();
        return Status.Ok;
    }

    // ------------------------------------------------------------- streams

    [UnmanagedCallersOnly(EntryPoint = "acme_stream_next", CallConvs = [typeof(CallConvCdecl)])]
    public static int StreamNext(ulong h, ulong* future)
    {
        if (future == null) return Status.InvalidArgument;
        *future = 0;
        if (Handles.Get<NativeStream>(h) is not { } s) return Status.InvalidHandle;
        var (status, f) = s.Next();
        *future = f;
        return status;
    }

    [UnmanagedCallersOnly(EntryPoint = "acme_stream_cancel", CallConvs = [typeof(CallConvCdecl)])]
    public static int StreamCancel(ulong h)
    {
        if (Handles.Get<NativeStream>(h) is not { } s) return Status.InvalidHandle;
        s.Cancel();
        return Status.Ok;
    }

    [UnmanagedCallersOnly(EntryPoint = "acme_stream_drop", CallConvs = [typeof(CallConvCdecl)])]
    public static int StreamDrop(ulong h)
    {
        if (Handles.Remove<NativeStream>(h) is not { } s) return Status.InvalidHandle;
        s.Cancel();
        return Status.Ok;
    }
}

// Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.
// Hand-written, generic: the seam between generated SDK code and whatever
// executes the contract (native core via the Wabi ABI, a WebAssembly
// component via the Canonical ABI, or a test double).
#nullable enable
using System;
using System.Collections.Generic;
using System.Threading;
using System.Threading.Tasks;

namespace Gematik.Pki.Runtime;

/// <summary>
/// Executes contract calls addressed by stable method id with payload-encoded
/// arguments. Generated client classes depend only on this interface.
/// </summary>
public interface IClientBackend
{
    string Description { get; }

    /// <summary>Synchronous WIT function (never blocks on I/O). Returns the encoded result.</summary>
    byte[] CallSync(ulong obj, uint methodId, byte[] request);

    /// <summary>Async WIT function. Cancelling the token cancels the operation in the core.</summary>
    Task<byte[]> CallAsync(ulong obj, uint methodId, byte[] request, CancellationToken cancellationToken);

    /// <summary>Consumes a stream handle returned by a call. Ends the stream when the enumeration ends.</summary>
    IAsyncEnumerable<byte[]> ReadStream(ulong stream, CancellationToken cancellationToken);

    /// <summary>Releases an object handle. Must be idempotent-safe (called once by <see cref="ObjectHandle"/>).</summary>
    void DropObject(ulong obj);
}

/// <summary>Owns one object handle; guarantees it is dropped exactly once (Dispose or finalizer).</summary>
public sealed class ObjectHandle : IDisposable
{
    private readonly IClientBackend _backend;
    private ulong _value;

    public ObjectHandle(IClientBackend backend, ulong value)
    {
        if (value == 0) throw new WabiProtocolException("core returned the null handle");
        _backend = backend;
        _value = value;
    }

    public ulong Value
    {
        get
        {
            var v = Volatile.Read(ref _value);
            return v != 0 ? v : throw new ObjectDisposedException("native object already disposed");
        }
    }

    public void Dispose()
    {
        Release();
        GC.SuppressFinalize(this);
    }

    ~ObjectHandle() => Release();

    private void Release()
    {
        var v = Interlocked.Exchange(ref _value, 0);
        if (v != 0) _backend.DropObject(v); // ABI is thread-safe: fine from the finalizer thread
    }
}

/// <summary>
/// Placeholder for executing the same contract as a WebAssembly component
/// (client.component.wasm). See docs/COMPONENT_MODEL.md: the generated client
/// classes stay identical; only this backend maps (method id, payload) to the
/// component's exported functions via the Canonical ABI.
/// </summary>
public sealed class ComponentModelBackend : IClientBackend
{
    public string Description => "component-model (not implemented in this PoC)";
    public byte[] CallSync(ulong obj, uint methodId, byte[] request) => throw NotImplemented();
    public Task<byte[]> CallAsync(ulong obj, uint methodId, byte[] request, CancellationToken ct) => throw NotImplemented();
    public IAsyncEnumerable<byte[]> ReadStream(ulong stream, CancellationToken ct) => throw NotImplemented();
    public void DropObject(ulong obj) => throw NotImplemented();
    private static NotSupportedException NotImplemented() =>
        new("The Component Model backend is an extension point; see docs/COMPONENT_MODEL.md.");
}

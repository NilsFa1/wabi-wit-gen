using System.Diagnostics;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using Demo.Client;
using Demo.Client.Runtime;

namespace Demo.Client.Tests;

[AttributeUsage(AttributeTargets.Method)]
sealed class TestAttribute : Attribute;

/// <summary>
/// The app side of host calls: the app owns the key (here a .NET ECDsa key
/// standing in for a TPM / Windows Hello / HSM); the core asks it to sign.
/// </summary>
sealed class AppKeys : IHostKeys
{
    public readonly ECDsa Key = ECDsa.Create(ECCurve.NamedCurves.nistP256);
    readonly ECDsa _other = ECDsa.Create(ECCurve.NamedCurves.nistP256);
    public int Signatures;
    /// <summary>Completed when the core gave up on a call for "slow-key".</summary>
    public readonly TaskCompletionSource Cancelled = new(TaskCreationOptions.RunContinuationsAsynchronously);

    public string Fingerprint => Convert.ToHexString(SHA256.HashData(Key.ExportSubjectPublicKeyInfo())).ToLowerInvariant();

    // "forged-key" announces the app's key but signs with another one.
    public async Task<byte[]> PublicKeyAsync(string keyId, CancellationToken cancellationToken)
    {
        if (keyId == "slow-key") // e.g. waiting for the user: ends when the core no longer waits
        {
            using var _ = cancellationToken.Register(() => Cancelled.TrySetResult());
            await Task.Delay(Timeout.Infinite, cancellationToken);
        }
        return keyId is "app-key" or "forged-key" ? Key.ExportSubjectPublicKeyInfo()
                                                  : throw new ClientException(new ClientError.KeyError(keyId));
    }

    public async Task<byte[]> SignAsync(string keyId, byte[] digest, CancellationToken cancellationToken)
    {
        if (keyId is not ("app-key" or "forged-key")) throw new ClientException(new ClientError.KeyError(keyId));
        // a real keystore may prompt the user: answering later (and out of order) is fine
        await Task.Delay(Random.Shared.Next(0, 6));
        Interlocked.Increment(ref Signatures);
        return (keyId == "app-key" ? Key : _other).SignHash(digest, DSASignatureFormat.Rfc3279DerSequence);
    }
}

static class Assert
{
    public static void True(bool cond, string what) { if (!cond) throw new Exception("assertion failed: " + what); }
    public static void Equal<T>(T expected, T actual, string what = "")
    {
        if (!EqualityComparer<T>.Default.Equals(expected, actual))
            throw new Exception($"assertion failed {what}: expected <{expected}>, got <{actual}>");
    }
    public static async Task<TEx> ThrowsAsync<TEx>(Func<Task> f) where TEx : Exception
    {
        try { await f(); }
        catch (TEx e) { return e; }
        catch (Exception e) { throw new Exception($"expected {typeof(TEx).Name}, got {e.GetType().Name}: {e.Message}"); }
        throw new Exception($"expected {typeof(TEx).Name}, nothing thrown");
    }
    public static TEx Throws<TEx>(Action f) where TEx : Exception
    {
        try { f(); }
        catch (TEx e) { return e; }
        catch (Exception e) { throw new Exception($"expected {typeof(TEx).Name}, got {e.GetType().Name}: {e.Message}"); }
        throw new Exception($"expected {typeof(TEx).Name}, nothing thrown");
    }
}

public static class Program
{
    // The backend the core talks to: the in-process mock unless WABI_TEST_ENDPOINT names a mock server.
    static readonly string Endpoint = Environment.GetEnvironmentVariable("WABI_TEST_ENDPOINT") ?? "mem://csharp-tests";

    static ClientOptions Options(string key = "demo-key", string? endpoint = null, string? keyId = null) =>
        new(endpoint ?? Endpoint, key, RequestTimeoutMs: 10_000, keyId);

    static async Task<DemoClient> Connected()
    {
        var c = new DemoClient(Options());
        await c.ConnectAsync();
        return c;
    }

    // ------------------------------------------------------------------ tests

    [Test]
    static Task SyncCall()
    {
        using var c = new DemoClient(Options());
        Assert.Equal(ConnectionState.Disconnected, c.State());
        return Task.CompletedTask;
    }

    [Test]
    static async Task AsyncCall()
    {
        await using var c = await Connected();
        Assert.Equal(ConnectionState.Connected, c.State());
        var customer = await c.GetCustomerAsync("4711");
        Assert.Equal(new Customer("4711", "Erika Mustermann", "erika@example.com"), customer);
        await c.DisconnectAsync();
        Assert.Equal(ConnectionState.Disconnected, c.State());
    }

    [Test]
    static async Task AsyncError()
    {
        await using var c = await Connected();
        var e = await Assert.ThrowsAsync<ClientException>(() => c.GetCustomerAsync("does-not-exist"));
        Assert.True(e.Error is ClientError.NotFound, "not-found");

        using var bad = new DemoClient(Options(key: "wrong-key"));
        e = await Assert.ThrowsAsync<ClientException>(() => bad.ConnectAsync());
        Assert.True(e.Error is ClientError.Unauthorized, "unauthorized");

        using var offline = new DemoClient(Options(endpoint: "http://127.0.0.1:1"));
        e = await Assert.ThrowsAsync<ClientException>(() => offline.ConnectAsync());
        Assert.True(e.Error is ClientError.NetworkError { Value: var msg } && msg.Length > 0, "network-error");

        using var notConnected = new DemoClient(Options());
        e = await Assert.ThrowsAsync<ClientException>(() => notConnected.GetCustomerAsync("4711"));
        Assert.True(e.Error is ClientError.Disconnected, "disconnected");

        // A session without a key was not signed by anyone.
        e = await Assert.ThrowsAsync<ClientException>(() => c.KeyFingerprintAsync());
        Assert.True(e.Error is ClientError.Unauthorized, "no key fingerprint without a key");
    }

    [Test]
    static async Task Cancellation()
    {
        await using var c = await Connected();
        using var cts = new CancellationTokenSource(TimeSpan.FromMilliseconds(100));
        var sw = Stopwatch.StartNew();
        await Assert.ThrowsAsync<OperationCanceledException>(() =>
            c.SendCommandAsync(new Command("sleep", Encoding.UTF8.GetBytes("5000")), cts.Token));
        Assert.True(sw.ElapsedMilliseconds < 2000, $"cancel took {sw.ElapsedMilliseconds} ms");
        // Already-cancelled token: fails fast, the call is never started (independent of core timing).
        await Assert.ThrowsAsync<OperationCanceledException>(() => c.GetCustomerAsync("4711", new CancellationToken(true)));
        // The client is still usable.
        Assert.Equal("4711", (await c.GetCustomerAsync("4711")).Id);
    }

    [Test]
    static async Task Stream()
    {
        await using var c = await Connected();
        using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(10));
        var received = new List<ClientEvent>();
        var reader = Task.Run(async () =>
        {
            await foreach (var evt in c.Events(cts.Token))
            {
                received.Add(evt);
                if (received.Count == 2) break;
            }
        });
        await Task.Delay(100); // let the subscription start
        await c.SendCommandAsync(new Command("broadcast", Encoding.UTF8.GetBytes("hello 👋")));
        var created = await c.CreateCustomerAsync("Max Mustermann", null);
        await reader;
        Assert.True(received[0] is ClientEvent.MessageReceived { Value: "hello 👋" }, "message-received");
        Assert.True(received[1] is ClientEvent.CustomerUpdated { Value: var cu } && cu == created, "customer-updated");
    }

    [Test]
    static async Task StreamCancellation()
    {
        await using var c = await Connected();
        using var cts = new CancellationTokenSource();
        var started = new TaskCompletionSource();
        var reader = Task.Run(async () =>
        {
            started.SetResult();
            await foreach (var _ in c.Events(cts.Token)) { }
        });
        await started.Task;
        await Task.Delay(100);
        cts.Cancel();
        await Assert.ThrowsAsync<OperationCanceledException>(() => reader);
        // A new subscription still works after the old one was cancelled and dropped.
        using var cts2 = new CancellationTokenSource(TimeSpan.FromSeconds(10));
        var next = Task.Run(async () => { await foreach (var e in c.Events(cts2.Token)) return e; return null; });
        await Task.Delay(100);
        await c.SendCommandAsync(new Command("broadcast", Encoding.UTF8.GetBytes("again")));
        Assert.True(await next is ClientEvent.MessageReceived { Value: "again" }, "event after resubscribe");
    }

    [Test]
    static async Task ObjectLifetime()
    {
        var c = await Connected();
        c.Dispose();
        Assert.Throws<ObjectDisposedException>(() => c.State());
        await Assert.ThrowsAsync<ObjectDisposedException>(() => c.GetCustomerAsync("4711"));
        // Finalizer path: an undisposed client is released by the GC without crashing.
        for (int i = 0; i < 50; i++) _ = new DemoClient(Options());
        GC.Collect();
        GC.WaitForPendingFinalizers();
    }

    [Test]
    static Task DoubleDrop()
    {
        var c = new DemoClient(Options());
        c.Dispose();
        c.Dispose(); // SDK level: idempotent
        // ABI level: the core detects the second drop instead of corrupting memory.
        var backend = NativeBackend.Shared;
        var bytes = new MsgPackWriter();
        bytes.WriteArrayHeader(1);
        WireCodec.WriteClientOptions(bytes, Options());
        var handle = new MsgPackReader(backend.CallSync(0, MethodIds.ClientConstructor, bytes.ToArray())).ReadUInt64();
        Assert.Equal(WabiStatus.Ok, NativeAbi.wabi_object_drop(handle));
        Assert.Equal(WabiStatus.ErrInvalidHandle, NativeAbi.wabi_object_drop(handle));
        Assert.Equal(WabiStatus.ErrInvalidHandle, NativeAbi.wabi_future_drop(handle));
        var e = Assert.Throws<WabiNativeException>(() => backend.CallSync(handle, MethodIds.ClientState, [0x90]));
        Assert.Equal(WabiStatus.ErrInvalidHandle, e.Status);
        return Task.CompletedTask;
    }

    [Test]
    static async Task ConcurrentCalls()
    {
        await using var c = await Connected();
        var tasks = Enumerable.Range(0, 500).Select(_ => Task.Run(() => c.GetCustomerAsync("4711")));
        var results = await Task.WhenAll(tasks);
        Assert.True(results.All(r => r.Name == "Erika Mustermann"), "all results correct");
    }

    [Test]
    static async Task CallbackDuringDispose()
    {
        var c = await Connected();
        var slow = c.SendCommandAsync(new Command("sleep", Encoding.UTF8.GetBytes("3000")));
        var events = Task.Run(async () => { await foreach (var _ in c.Events()) { } });
        await Task.Delay(100);
        c.Dispose(); // completion callbacks race with the dispose
        try { await slow; throw new Exception("slow call should not succeed"); }
        catch (ClientException e) { Assert.True(e.Error is ClientError.Disconnected, "disconnected"); }
        catch (OperationCanceledException) { /* also acceptable per ABI.md §4 */ }
        await events.WaitAsync(TimeSpan.FromSeconds(5)); // stream ends after dispose
    }

    [Test]
    static Task CoreVersionMismatch()
    {
        var current = NativeBackend.Shared.Info;
        current.EnsureCompatible();
        var dir = Environment.GetEnvironmentVariable("WABI_FAKE_CORES_DIR");
        if (dir is null) { Console.WriteLine("    (WABI_FAKE_CORES_DIR not set: using synthetic metadata only)"); }
        else
        {
            foreach (var (file, reason) in new[] { ("libfake_abi_v2.so", "ABI"), ("libfake_contract_v2.so", "contract"), ("libfake_no_streams.so", "capabilities") })
            {
                var info = CoreInfo.Probe(Path.Combine(dir, file));
                var e = Assert.Throws<WabiIncompatibleLibraryException>(info.EnsureCompatible);
                Console.WriteLine($"    rejected {file} ({reason}): {e.Message}");
            }
        }
        Assert.Throws<WabiIncompatibleLibraryException>(() => (current with { AbiVersion = 2u << 16 }).EnsureCompatible());
        Assert.Throws<WabiIncompatibleLibraryException>(() => (current with { ContractId = "wabi:other@1.0.0" }).EnsureCompatible());
        // Newer minor versions are accepted.
        (current with { AbiVersion = (1u << 16) | 7, ContractId = "demo:client@1.9.0" }).EnsureCompatible();
        return Task.CompletedTask;
    }

    [Test]
    static async Task LargePayload()
    {
        await using var c = await Connected();
        var payload = new byte[4 * 1024 * 1024];
        new Random(42).NextBytes(payload);
        var result = await c.SendCommandAsync(new Command("echo", payload));
        Assert.True(result.Payload.AsSpan().SequenceEqual(payload), "4 MiB echo roundtrip");
    }

    [Test]
    static async Task UnicodeStrings()
    {
        await using var c = await Connected();
        const string name = "Jürgen Groß 张伟 Ölçü 🚀👩‍💻";
        var created = await c.CreateCustomerAsync(name, "jürgen@exämple.de");
        Assert.Equal(name, created.Name);
        Assert.Equal(created, await c.GetCustomerAsync(created.Id));
        // Lone surrogates are not valid Unicode scalar values: rejected before crossing the ABI.
        await Assert.ThrowsAsync<ArgumentException>(() => c.GetCustomerAsync("\ud800"));
    }

    // ------------------------------- resources as parameters and results

    [Test]
    static async Task ResourceHandles()
    {
        using var transport = new Transport(Endpoint);
        await using var a = transport.OpenClient(Options(endpoint: "unused://")); // a resource as result
        await using var b = new DemoClient(Options());
        b.UseTransport(transport); // lent for the call
        await Task.WhenAll(a.ConnectAsync(), b.ConnectAsync());
        var before = transport.RequestCount();
        await a.GetCustomerAsync("4711");
        await b.GetCustomerAsync("4711");
        Assert.Equal(before + 2, transport.RequestCount(), "both clients share the transport");

        // An owned parameter moves into the core: the SDK object is gone afterwards.
        var other = new Transport(Endpoint);
        await using var c = other.OpenClient(Options());
        await c.ConnectAsync();
        var (mine, theirs) = (transport.RequestCount(), other.RequestCount());
        transport.Absorb(other);
        Assert.Equal(mine + theirs, transport.RequestCount(), "absorbed");
        Assert.Throws<ObjectDisposedException>(() => other.RequestCount());
        Assert.Throws<ObjectDisposedException>(() => transport.Absorb(other));
        other.Dispose(); // harmless: nothing left to drop
    }

    // ------------------------------------------------- host calls (ABI.md §4a)

    [Test]
    static async Task HostKeysNotRegistered()
    {
        using var c = new DemoClient(Options(keyId: "app-key"));
        var e = await Assert.ThrowsAsync<ClientException>(() => c.ConnectAsync());
        Assert.True(e.Error is ClientError.KeyError, "without registration the core reports a domain error");
    }

    [Test]
    static async Task SignedSession()
    {
        var app = new AppKeys();
        ClientHost.Register(app);
        try
        {
            await using var c = new DemoClient(Options(keyId: "app-key"));
            await c.ConnectAsync();
            // The backend verified the signatures and names the key they were made with.
            Assert.Equal(app.Fingerprint, await c.KeyFingerprintAsync());
            var before = app.Signatures;
            Assert.Equal("Erika Mustermann", (await c.GetCustomerAsync("4711")).Name);
            Assert.Equal(before + 1, app.Signatures, "one signature per request");
        }
        finally { ClientHost.Unregister(); }
    }

    [Test]
    static async Task ConcurrentSignedCalls()
    {
        var app = new AppKeys();
        ClientHost.Register(app);
        try
        {
            await using var c = new DemoClient(Options(keyId: "app-key"));
            await c.ConnectAsync();
            var before = app.Signatures;
            var results = await Task.WhenAll(Enumerable.Range(0, 64).Select(i =>
                c.SendCommandAsync(new Command("echo", Encoding.UTF8.GetBytes($"payload-{i}")))));
            Assert.Equal(before + 64, app.Signatures, "one signature per request");
            for (var i = 0; i < results.Length; i++) // every signature ended up on its own request
                Assert.Equal($"payload-{i}", Encoding.UTF8.GetString(results[i].Payload));
        }
        finally { ClientHost.Unregister(); }
    }

    [Test]
    static async Task HostErrors()
    {
        ClientHost.Register(new AppKeys());
        try
        {
            // The app's exception becomes the caller's ClientException.
            using var unknown = new DemoClient(Options(keyId: "unknown-key"));
            var e = await Assert.ThrowsAsync<ClientException>(() => unknown.ConnectAsync());
            Assert.True(e.Error is ClientError.KeyError { Value: "unknown-key" }, $"key-error from the app: {e.Error}");
            // A signature that does not belong to the announced key is rejected by the backend.
            using var forged = new DemoClient(Options(keyId: "forged-key"));
            e = await Assert.ThrowsAsync<ClientException>(() => forged.ConnectAsync());
            Assert.True(e.Error is ClientError.Unauthorized, $"forged signature: {e.Error}");
        }
        finally { ClientHost.Unregister(); }
    }

    [Test]
    static async Task HostKeysPerInstance()
    {
        // No process-wide registration: every client has its own keystore.
        var (first, second) = (new AppKeys(), new AppKeys());
        await using var a = new DemoClient(Options(keyId: "app-key"));
        await using var b = new DemoClient(Options(keyId: "app-key"));
        ClientHost.Register(a, first);
        ClientHost.Register(b, second);
        await Task.WhenAll(a.ConnectAsync(), b.ConnectAsync());
        Assert.Equal(first.Fingerprint, await a.KeyFingerprintAsync());
        Assert.Equal(second.Fingerprint, await b.KeyFingerprintAsync());
        Assert.True(first.Fingerprint != second.Fingerprint, "two keys");

        // An instance without its own registration falls back to the process-wide one (here: none).
        using var c = new DemoClient(Options(keyId: "app-key"));
        var e = await Assert.ThrowsAsync<ClientException>(() => c.ConnectAsync());
        Assert.True(e.Error is ClientError.KeyError, "no keystore for the third client");

        // Unregistering one instance does not touch the other.
        ClientHost.Unregister(a);
        e = await Assert.ThrowsAsync<ClientException>(() => a.GetCustomerAsync("4711"));
        Assert.True(e.Error is ClientError.KeyError, "a lost its keystore");
        Assert.Equal("4711", (await b.GetCustomerAsync("4711")).Id);
    }

    [Test]
    static async Task HostCallCancellation()
    {
        var app = new AppKeys();
        ClientHost.Register(app);
        try
        {
            using var c = new DemoClient(Options(keyId: "slow-key"));
            using var cts = new CancellationTokenSource(TimeSpan.FromMilliseconds(100));
            await Assert.ThrowsAsync<OperationCanceledException>(() => c.ConnectAsync(cts.Token));
            // The core told the app that nobody waits for the public key any more.
            await app.Cancelled.Task.WaitAsync(TimeSpan.FromSeconds(5));
        }
        finally { ClientHost.Unregister(); }
    }

    // ----------------------------------------------------------------- runner

    public static async Task<int> Main(string[] args)
    {
        var filter = args.FirstOrDefault(a => !a.StartsWith("--"));
        Console.WriteLine($"core under test: {NativeBackend.Shared.Description}");
        var tests = typeof(Program).GetMethods(BindingFlags.NonPublic | BindingFlags.Static)
            .Select(m => (Method: m, Attr: m.GetCustomAttribute<TestAttribute>()))
            .Where(t => t.Attr is not null)
            .OrderBy(t => t.Method.MetadataToken);
        int failed = 0, passed = 0;
        foreach (var (m, _) in tests)
        {
            if (filter is not null && !m.Name.Contains(filter, StringComparison.OrdinalIgnoreCase)) continue;
            var sw = Stopwatch.StartNew();
            try
            {
                await ((Task)m.Invoke(null, null)!).WaitAsync(TimeSpan.FromSeconds(60));
                passed++;
                Console.WriteLine($"PASS {m.Name} ({sw.ElapsedMilliseconds} ms)");
            }
            catch (Exception e)
            {
                failed++;
                Console.WriteLine($"FAIL {m.Name}: {(e is TargetInvocationException t ? t.InnerException : e)}");
            }
        }
        Console.WriteLine($"{passed} passed, {failed} failed");
        return failed == 0 ? 0 : 1;
    }
}

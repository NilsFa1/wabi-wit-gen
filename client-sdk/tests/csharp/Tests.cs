using System.Diagnostics;
using System.Reflection;
using System.Text;
using Wabi.Client;
using Wabi.Client.Runtime;

namespace Wabi.Client.Tests;

[AttributeUsage(AttributeTargets.Method)]
sealed class TestAttribute(bool needsFullCore = true) : Attribute
{
    public bool NeedsFullCore { get; } = needsFullCore;
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
    static ClientOptions Options(string key = "demo-key", string endpoint = "mem://csharp-tests") =>
        new(endpoint, key, RequestTimeoutMs: 10_000);

    static async Task<WabiClient> Connected()
    {
        var c = new WabiClient(Options());
        await c.ConnectAsync();
        return c;
    }

    // ------------------------------------------------------------------ tests

    [Test(needsFullCore: false)]
    static Task SyncCall()
    {
        using var c = new WabiClient(Options());
        Assert.Equal(ConnectionState.Disconnected, c.State());
        return Task.CompletedTask;
    }

    [Test(needsFullCore: false)]
    static async Task AsyncCall()
    {
        await using var c = await Connected();
        Assert.Equal(ConnectionState.Connected, c.State());
        var customer = await c.GetCustomerAsync("4711");
        Assert.Equal(new Customer("4711", "Erika Mustermann", "erika@example.com"), customer);
        await c.DisconnectAsync();
        Assert.Equal(ConnectionState.Disconnected, c.State());
    }

    [Test(needsFullCore: false)]
    static async Task AsyncError()
    {
        await using var c = await Connected();
        var e = await Assert.ThrowsAsync<ClientException>(() => c.GetCustomerAsync("does-not-exist"));
        Assert.True(e.Error is ClientError.NotFound, "not-found");

        using var bad = new WabiClient(Options(key: "wrong-key"));
        e = await Assert.ThrowsAsync<ClientException>(() => bad.ConnectAsync());
        Assert.True(e.Error is ClientError.Unauthorized, "unauthorized");

        using var offline = new WabiClient(Options(endpoint: "mem://unreachable"));
        e = await Assert.ThrowsAsync<ClientException>(() => offline.ConnectAsync());
        Assert.True(e.Error is ClientError.NetworkError { Value: var msg } && msg.Length > 0, "network-error");

        using var notConnected = new WabiClient(Options());
        e = await Assert.ThrowsAsync<ClientException>(() => notConnected.GetCustomerAsync("4711"));
        Assert.True(e.Error is ClientError.Disconnected, "disconnected");
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

    [Test(needsFullCore: false)]
    static async Task ObjectLifetime()
    {
        var c = await Connected();
        c.Dispose();
        Assert.Throws<ObjectDisposedException>(() => c.State());
        await Assert.ThrowsAsync<ObjectDisposedException>(() => c.GetCustomerAsync("4711"));
        // Finalizer path: an undisposed client is released by the GC without crashing.
        for (int i = 0; i < 50; i++) _ = new WabiClient(Options());
        GC.Collect();
        GC.WaitForPendingFinalizers();
    }

    [Test(needsFullCore: false)]
    static Task DoubleDrop()
    {
        var c = new WabiClient(Options());
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

    [Test(needsFullCore: false)]
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

    [Test(needsFullCore: false)]
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
        (current with { AbiVersion = (1u << 16) | 7, ContractId = "wabi:client@1.9.0" }).EnsureCompatible();
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

    // ----------------------------------------------------------------- runner

    public static async Task<int> Main(string[] args)
    {
        bool basic = args.Contains("--basic");
        var filter = args.FirstOrDefault(a => !a.StartsWith("--"));
        Console.WriteLine($"core under test: {NativeBackend.Shared.Description}");
        var tests = typeof(Program).GetMethods(BindingFlags.NonPublic | BindingFlags.Static)
            .Select(m => (Method: m, Attr: m.GetCustomAttribute<TestAttribute>()))
            .Where(t => t.Attr is not null)
            .OrderBy(t => t.Method.MetadataToken);
        int failed = 0, passed = 0;
        foreach (var (m, attr) in tests)
        {
            if (filter is not null && !m.Name.Contains(filter, StringComparison.OrdinalIgnoreCase)) continue;
            if (basic && attr!.NeedsFullCore) { Console.WriteLine($"SKIP {m.Name} (basic core)"); continue; }
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

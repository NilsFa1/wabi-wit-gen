import com.acme.client.*;
import com.acme.client.runtime.*;

import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Dependency-free test runner for the Java SDK.
 *   java --enable-native-access=ALL-UNNAMED -cp acme-client.jar:. AcmeClientTests [--basic]
 * The core under test is chosen with ACME_NATIVE_LIB.
 */
public final class AcmeClientTests {
    interface Test { void run() throws Exception; }

    static ClientOptions options() {
        return options("demo-key", "mem://java-tests");
    }

    static ClientOptions options(String key, String endpoint) {
        return new ClientOptions(endpoint, Optional.of(key), 10_000);
    }

    static AcmeClient connected() {
        var c = new AcmeClient(options());
        c.connect().join();
        return c;
    }

    static void check(boolean cond, String what) {
        if (!cond) throw new AssertionError(what);
    }

    static <T extends Throwable> T failsWith(Class<T> type, CompletableFuture<?> f) {
        try {
            f.get(30, TimeUnit.SECONDS);
        } catch (ExecutionException e) {
            if (type.isInstance(e.getCause())) return type.cast(e.getCause());
            throw new AssertionError("expected " + type.getSimpleName() + ", got " + e.getCause());
        } catch (CancellationException e) {
            if (type.isInstance(e)) return type.cast(e);
            throw new AssertionError("expected " + type.getSimpleName() + ", got " + e);
        } catch (Exception e) {
            throw new AssertionError("unexpected " + e);
        }
        throw new AssertionError("expected " + type.getSimpleName() + ", completed normally");
    }

    /** Collects up to {@code max} items of a publisher. */
    static final class Collector<T> implements Flow.Subscriber<T> {
        final List<T> items = Collections.synchronizedList(new ArrayList<>());
        final CompletableFuture<List<T>> done = new CompletableFuture<>();
        final CompletableFuture<Void> subscribed = new CompletableFuture<>();
        final int max;
        volatile Flow.Subscription subscription;

        Collector(int max) { this.max = max; }

        public void onSubscribe(Flow.Subscription s) { subscription = s; s.request(1); subscribed.complete(null); }
        public void onNext(T item) {
            items.add(item);
            if (items.size() >= max) { subscription.cancel(); done.complete(items); }
            else subscription.request(1);
        }
        public void onError(Throwable t) { done.completeExceptionally(t); }
        public void onComplete() { done.complete(items); }
    }

    // ------------------------------------------------------------------ tests

    static void syncCall() {
        try (var c = new AcmeClient(options())) {
            check(c.state() == ConnectionState.DISCONNECTED, "initial state");
        }
    }

    static void asyncCall() {
        try (var c = new AcmeClient(options())) {
            // thenCompose chain, as an application would write it
            Customer customer = c.connect().thenCompose(v -> c.getCustomer("4711")).join();
            check(customer.equals(new Customer("4711", "Erika Mustermann", Optional.of("erika@example.com"))), "customer " + customer);
            check(c.state() == ConnectionState.CONNECTED, "connected");
            c.disconnect().join();
            check(c.state() == ConnectionState.DISCONNECTED, "disconnected");
        }
    }

    static void asyncError() {
        try (var c = connected()) {
            var e = failsWith(ClientException.class, c.getCustomer("does-not-exist"));
            check(e.error() instanceof ClientError.NotFound, "not-found");
            // join() wraps in CompletionException, as usual for CompletableFuture
            try {
                c.getCustomer("nope").join();
                throw new AssertionError("no exception");
            } catch (CompletionException ce) {
                check(ce.getCause() instanceof ClientException, "CompletionException(ClientException)");
            }
        }
        try (var bad = new AcmeClient(options("wrong-key", "mem://java-tests"))) {
            check(failsWith(ClientException.class, bad.connect()).error() instanceof ClientError.Unauthorized, "unauthorized");
        }
        try (var offline = new AcmeClient(options("demo-key", "mem://unreachable"))) {
            var err = failsWith(ClientException.class, offline.connect()).error();
            check(err instanceof ClientError.NetworkError ne && !ne.value().isEmpty(), "network-error");
        }
    }

    static void cancellation() throws Exception {
        try (var c = connected()) {
            long t0 = System.nanoTime();
            var slow = c.sendCommand(new Command("sleep", "5000".getBytes(StandardCharsets.UTF_8)));
            Thread.sleep(50);
            check(slow.cancel(true), "cancel() returns true");
            check(slow.isCancelled(), "future cancelled");
            failsWith(CancellationException.class, slow);
            // The core really stopped: a follow-up request is answered immediately.
            check(c.getCustomer("4711").get(2, TimeUnit.SECONDS).id().equals("4711"), "client usable");
            check((System.nanoTime() - t0) / 1_000_000 < 2000, "cancel was prompt");
        }
    }

    static void stream() throws Exception {
        try (var c = connected()) {
            var collector = new Collector<ClientEvent>(2);
            c.events().subscribe(collector);
            collector.subscribed.get(5, TimeUnit.SECONDS);
            Thread.sleep(100);
            c.sendCommand(new Command("broadcast", "hello 👋".getBytes(StandardCharsets.UTF_8))).join();
            var created = c.createCustomer("Max", Optional.empty()).join();
            var items = collector.done.get(10, TimeUnit.SECONDS);
            check(items.get(0) instanceof ClientEvent.MessageReceived m && m.value().equals("hello 👋"), "message-received " + items);
            check(items.get(1) instanceof ClientEvent.CustomerUpdated u && u.value().equals(created), "customer-updated");
        }
    }

    static void streamCancellation() throws Exception {
        try (var c = connected()) {
            var collector = new Collector<ClientEvent>(Integer.MAX_VALUE);
            c.events().subscribe(collector);
            collector.subscribed.get(5, TimeUnit.SECONDS);
            Thread.sleep(50);
            collector.subscription.cancel();
            collector.subscription.cancel(); // idempotent
            Thread.sleep(50);
            // Resubscribing works; the cancelled subscriber receives nothing more.
            var second = new Collector<ClientEvent>(1);
            c.events().subscribe(second);
            second.subscribed.get(5, TimeUnit.SECONDS);
            Thread.sleep(100);
            c.sendCommand(new Command("broadcast", "again".getBytes(StandardCharsets.UTF_8))).join();
            check(second.done.get(10, TimeUnit.SECONDS).get(0) instanceof ClientEvent.MessageReceived, "event after resubscribe");
            check(collector.items.isEmpty(), "cancelled subscriber got no items");
        }
    }

    static void objectLifetime() {
        var c = connected();
        c.close();
        try {
            c.state();
            throw new AssertionError("expected IllegalStateException");
        } catch (IllegalStateException expected) {
            // closed
        }
        // Cleaner path: unreachable clients are released without crashing.
        for (int i = 0; i < 50; i++) new AcmeClient(options());
        System.gc();
    }

    static void doubleDrop() {
        var c = new AcmeClient(options());
        c.close();
        c.close(); // idempotent at SDK level
        var backend = NativeBackend.shared();
        var w = new MsgPackWriter();
        w.writeArrayHeader(1);
        w.writeMapHeader(2); // required fields only (api-key is optional)
        w.writeUInt(1);
        w.writeString("mem://x");
        w.writeUInt(3);
        w.writeUInt(1000);
        long handle = new MsgPackReader(backend.callSync(0, 0x0100, w.toByteArray())).readUInt64();
        backend.dropObject(handle);
        try {
            backend.callSync(handle, 0x0101, new byte[] {(byte) 0x90});
            throw new AssertionError("expected AcmeNativeException");
        } catch (AcmeNativeException e) {
            check(e.status() == AbiConstants.ACME_ERR_INVALID_HANDLE, "invalid handle after drop");
        }
        backend.dropObject(handle); // second drop: detected by the core, harmless
    }

    static void concurrentCalls() {
        try (var c = connected()) {
            var futures = new ArrayList<CompletableFuture<Customer>>();
            for (int i = 0; i < 500; i++) futures.add(c.getCustomer("4711"));
            CompletableFuture.allOf(futures.toArray(CompletableFuture[]::new)).join();
            check(futures.stream().allMatch(f -> f.join().name().equals("Erika Mustermann")), "all correct");
        }
    }

    static void callbackDuringDispose() throws Exception {
        var c = connected();
        var slow = c.sendCommand(new Command("sleep", "3000".getBytes(StandardCharsets.UTF_8)));
        var events = new Collector<ClientEvent>(Integer.MAX_VALUE);
        c.events().subscribe(events);
        events.subscribed.get(5, TimeUnit.SECONDS);
        Thread.sleep(100);
        c.close();
        try {
            slow.get(5, TimeUnit.SECONDS);
            throw new AssertionError("should fail");
        } catch (ExecutionException e) {
            check(e.getCause() instanceof ClientException ce && ce.error() instanceof ClientError.Disconnected
                    || e.getCause() instanceof CancellationException, "disconnected: " + e.getCause());
        } catch (CancellationException e) {
            // also acceptable
        }
        events.done.get(5, TimeUnit.SECONDS); // publisher completes after close
    }

    static void coreVersionMismatch() {
        var info = NativeBackend.shared().info();
        info.ensureCompatible();
        var dir = System.getenv("ACME_FAKE_CORES_DIR");
        if (dir != null) {
            for (var f : List.of("libfake_abi_v2.so", "libfake_contract_v2.so", "libfake_no_streams.so")) {
                try {
                    CoreInfo.probe(Path.of(dir, f)).ensureCompatible();
                    throw new AssertionError(f + " accepted");
                } catch (AcmeIncompatibleLibraryException e) {
                    System.out.println("    rejected " + f + ": " + e.getMessage());
                }
            }
        }
        try {
            new CoreInfo(2 << 16, info.capabilities(), info.contractId(), "x").ensureCompatible();
            throw new AssertionError("abi 2.0 accepted");
        } catch (AcmeIncompatibleLibraryException expected) {
            // ok
        }
        new CoreInfo((1 << 16) | 3, info.capabilities(), "acme:client@1.4.0", "newer minor").ensureCompatible();
    }

    static void largePayload() {
        try (var c = connected()) {
            var payload = new byte[4 * 1024 * 1024];
            new Random(42).nextBytes(payload);
            var result = c.sendCommand(new Command("echo", payload)).join();
            check(Arrays.equals(payload, result.payload()), "4 MiB roundtrip");
        }
    }

    static void unicodeStrings() {
        try (var c = connected()) {
            var name = "Jürgen Groß 张伟 Ölçü 🚀👩‍💻";
            var created = c.createCustomer(name, Optional.of("jürgen@exämple.de")).join();
            check(created.name().equals(name), "name roundtrip");
            check(c.getCustomer(created.id()).join().equals(created), "read back");
            try {
                c.getCustomer("\ud800");
                throw new AssertionError("lone surrogate accepted");
            } catch (IllegalArgumentException expected) {
                // rejected before crossing the ABI
            }
        }
    }

    // ----------------------------------------------------------------- runner

    public static void main(String[] args) {
        boolean basic = Arrays.asList(args).contains("--basic");
        System.out.println("core under test: " + NativeBackend.shared().description());
        record T(String name, boolean full, Test test) {}
        var tests = List.of(
                new T("syncCall", false, AcmeClientTests::syncCall),
                new T("asyncCall", false, AcmeClientTests::asyncCall),
                new T("asyncError", false, AcmeClientTests::asyncError),
                new T("cancellation", true, AcmeClientTests::cancellation),
                new T("stream", true, AcmeClientTests::stream),
                new T("streamCancellation", true, AcmeClientTests::streamCancellation),
                new T("objectLifetime", false, AcmeClientTests::objectLifetime),
                new T("doubleDrop", false, AcmeClientTests::doubleDrop),
                new T("concurrentCalls", false, AcmeClientTests::concurrentCalls),
                new T("callbackDuringDispose", true, AcmeClientTests::callbackDuringDispose),
                new T("coreVersionMismatch", false, AcmeClientTests::coreVersionMismatch),
                new T("largePayload", true, AcmeClientTests::largePayload),
                new T("unicodeStrings", true, AcmeClientTests::unicodeStrings));
        int failed = 0, passed = 0;
        for (var t : tests) {
            if (basic && t.full()) {
                System.out.println("SKIP " + t.name() + " (basic core)");
                continue;
            }
            long t0 = System.nanoTime();
            var error = new AtomicReference<Throwable>();
            var th = new Thread(() -> {
                try { t.test().run(); } catch (Throwable e) { error.set(e); }
            });
            th.start();
            try { th.join(60_000); } catch (InterruptedException e) { error.set(e); }
            if (th.isAlive()) error.compareAndSet(null, new TimeoutException("test timed out"));
            if (error.get() == null) {
                passed++;
                System.out.printf("PASS %s (%d ms)%n", t.name(), (System.nanoTime() - t0) / 1_000_000);
            } else {
                failed++;
                System.out.println("FAIL " + t.name() + ": " + error.get());
                error.get().printStackTrace(System.out);
            }
        }
        System.out.println(passed + " passed, " + failed + " failed");
        System.exit(failed == 0 ? 0 : 1);
    }
}

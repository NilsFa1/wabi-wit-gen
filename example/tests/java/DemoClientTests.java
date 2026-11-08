import com.demo.client.*;
import com.demo.client.runtime.*;

import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.MessageDigest;
import java.security.Signature;
import java.security.spec.ECGenParameterSpec;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicReference;

/**
 * Dependency-free test runner for the Java SDK.
 *   java --enable-native-access=ALL-UNNAMED -cp demo-client.jar:. DemoClientTests
 * The core under test is chosen with WABI_NATIVE_LIB, the backend it talks to
 * with WABI_TEST_ENDPOINT (default: the in-process mock). Runs with
 * -Dwabi.binding=panama|jni.
 */
public final class DemoClientTests {
    interface Test { void run() throws Exception; }

    static final String ENDPOINT = Optional.ofNullable(System.getenv("WABI_TEST_ENDPOINT")).orElse("mem://java-tests");

    static ClientOptions options() {
        return options("demo-key", ENDPOINT);
    }

    static ClientOptions options(String key, String endpoint) {
        return new ClientOptions(endpoint, Optional.of(key), 10_000, Optional.empty());
    }

    /** A client whose requests are signed with the app-held key {@code keyId}. */
    static DemoClient signedClient(String keyId) {
        return new DemoClient(new ClientOptions(ENDPOINT, Optional.of("demo-key"), 10_000, Optional.of(keyId)));
    }

    /**
     * The app side of host calls: the app owns the key (here a JDK EC key standing
     * in for the Android Keystore, which returns the same formats: SPKI DER public
     * keys, ASN.1 DER signatures); the core asks it to sign.
     */
    static final class AppKeys implements HostKeys {
        final KeyPair key, other;
        final AtomicInteger signatures = new AtomicInteger();
        /** What the app returned for "slow-key": cancelled when the core gives up on the call. */
        final CompletableFuture<byte[]> slow = new CompletableFuture<>();

        AppKeys() throws Exception {
            var gen = KeyPairGenerator.getInstance("EC");
            gen.initialize(new ECGenParameterSpec("secp256r1"));
            key = gen.generateKeyPair();
            other = gen.generateKeyPair();
        }

        String fingerprint() throws Exception {
            return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(key.getPublic().getEncoded()));
        }

        // "forged-key" announces the app's key but signs with another one.
        static boolean known(String keyId) { return keyId.equals("app-key") || keyId.equals("forged-key"); }

        @Override
        public CompletionStage<byte[]> publicKey(String keyId) {
            if (keyId.equals("slow-key")) return slow; // e.g. waiting for the user
            if (!known(keyId)) throw new ClientException(new ClientError.KeyError(keyId));
            return CompletableFuture.completedFuture(key.getPublic().getEncoded()); // SPKI DER
        }

        @Override
        public CompletionStage<byte[]> sign(String keyId, byte[] digest) {
            if (!known(keyId)) return CompletableFuture.failedFuture(new ClientException(new ClientError.KeyError(keyId)));
            return CompletableFuture.supplyAsync(() -> { // answered later (out of order), on another thread
                try {
                    Thread.sleep(ThreadLocalRandom.current().nextInt(6));
                    var s = Signature.getInstance("NONEwithECDSA"); // digest computed by the core
                    s.initSign((keyId.equals("app-key") ? key : other).getPrivate());
                    s.update(digest);
                    signatures.incrementAndGet();
                    return s.sign(); // ASN.1 DER
                } catch (Exception e) {
                    throw new ClientException(new ClientError.KeyError(e.toString()));
                }
            });
        }
    }

    static DemoClient connected() {
        var c = new DemoClient(options());
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
        try (var c = new DemoClient(options())) {
            check(c.state() == ConnectionState.DISCONNECTED, "initial state");
        }
    }

    static void asyncCall() {
        try (var c = new DemoClient(options())) {
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
        try (var bad = new DemoClient(options("wrong-key", ENDPOINT))) {
            check(failsWith(ClientException.class, bad.connect()).error() instanceof ClientError.Unauthorized, "unauthorized");
        }
        try (var offline = new DemoClient(options("demo-key", "http://127.0.0.1:1"))) {
            var err = failsWith(ClientException.class, offline.connect()).error();
            check(err instanceof ClientError.NetworkError ne && !ne.value().isEmpty(), "network-error");
        }
        try (var c = connected()) { // a session without a key was not signed by anyone
            check(failsWith(ClientException.class, c.keyFingerprint()).error() instanceof ClientError.Unauthorized, "no key fingerprint without a key");
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
        for (int i = 0; i < 50; i++) new DemoClient(options());
        System.gc();
    }

    static void doubleDrop() {
        var c = new DemoClient(options());
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
        long handle = new MsgPackReader(backend.callSync(0, 0x0200 /* constructor */, w.toByteArray())).readUInt64();
        backend.dropObject(handle);
        try {
            backend.callSync(handle, 0x0201 /* state */, new byte[] {(byte) 0x90});
            throw new AssertionError("expected WabiNativeException");
        } catch (WabiNativeException e) {
            check(e.status() == AbiConstants.WABI_ERR_INVALID_HANDLE, "invalid handle after drop");
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
        var dir = System.getenv("WABI_FAKE_CORES_DIR");
        if (dir != null) {
            for (var f : List.of("libfake_abi_v2.so", "libfake_contract_v2.so", "libfake_no_streams.so")) {
                try {
                    CoreInfo.probe(Path.of(dir, f)).ensureCompatible();
                    throw new AssertionError(f + " accepted");
                } catch (WabiIncompatibleLibraryException e) {
                    System.out.println("    rejected " + f + ": " + e.getMessage());
                }
            }
        }
        try {
            new CoreInfo(2 << 16, info.capabilities(), info.contractId(), "x").ensureCompatible();
            throw new AssertionError("abi 2.0 accepted");
        } catch (WabiIncompatibleLibraryException expected) {
            // ok
        }
        new CoreInfo((1 << 16) | 3, info.capabilities(), "demo:client@1.4.0", "newer minor").ensureCompatible();
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

    // ------------------------------- resources as parameters and results

    static void resourceHandles() {
        try (var transport = new Transport(ENDPOINT);
             var a = transport.openClient(options("demo-key", "unused://")); // a resource as result
             var b = new DemoClient(options())) {
            b.useTransport(transport); // lent for the call
            CompletableFuture.allOf(a.connect(), b.connect()).join();
            long before = transport.requestCount();
            a.getCustomer("4711").join();
            b.getCustomer("4711").join();
            check(transport.requestCount() == before + 2, "both clients share the transport");

            // An owned parameter moves into the core: the SDK object is gone afterwards.
            var other = new Transport(ENDPOINT);
            try (var c = other.openClient(options())) {
                c.connect().join();
                long mine = transport.requestCount(), theirs = other.requestCount();
                transport.absorb(other);
                check(transport.requestCount() == mine + theirs, "absorbed");
            }
            try {
                other.requestCount();
                throw new AssertionError("expected IllegalStateException");
            } catch (IllegalStateException expected) {
                // moved
            }
            other.close(); // harmless: nothing left to drop
        }
    }

    // ------------------------------------------------- host calls (ABI.md §4a)

    static void hostKeysNotRegistered() {
        try (var c = signedClient("app-key")) {
            var e = failsWith(ClientException.class, c.connect());
            check(e.error() instanceof ClientError.KeyError, "without registration the core reports a domain error: " + e.error());
        }
    }

    static void signedSession() throws Exception {
        var app = new AppKeys();
        ClientHost.register(app);
        try (var c = signedClient("app-key")) {
            c.connect().join();
            // The backend verified the signatures and names the key they were made with.
            check(c.keyFingerprint().join().equals(app.fingerprint()), "key fingerprint");
            int before = app.signatures.get();
            check(c.getCustomer("4711").join().name().equals("Erika Mustermann"), "signed request");
            check(app.signatures.get() == before + 1, "one signature per request");
        } finally {
            ClientHost.unregister();
        }
    }

    static void concurrentSignedCalls() throws Exception {
        var app = new AppKeys();
        ClientHost.register(app);
        try (var c = signedClient("app-key")) {
            c.connect().join();
            int before = app.signatures.get();
            var futures = new ArrayList<CompletableFuture<CommandResult>>();
            for (int i = 0; i < 64; i++) futures.add(c.sendCommand(new Command("echo", ("payload-" + i).getBytes(StandardCharsets.UTF_8))));
            for (int i = 0; i < 64; i++) { // every signature ended up on its own request
                check(new String(futures.get(i).join().payload(), StandardCharsets.UTF_8).equals("payload-" + i), "call " + i + " got its own answer");
            }
            check(app.signatures.get() == before + 64, "one signature per request");
        } finally {
            ClientHost.unregister();
        }
    }

    static void hostErrors() throws Exception {
        ClientHost.register(new AppKeys());
        try (var unknown = signedClient("unknown-key"); var forged = signedClient("forged-key")) {
            // The app's exception becomes the caller's ClientException.
            var e = failsWith(ClientException.class, unknown.connect());
            check(e.error() instanceof ClientError.KeyError k && k.value().equals("unknown-key"), "key-error from the app: " + e.error());
            // A signature that does not belong to the announced key is rejected by the backend.
            e = failsWith(ClientException.class, forged.connect());
            check(e.error() instanceof ClientError.Unauthorized, "forged signature: " + e.error());
        } finally {
            ClientHost.unregister();
        }
    }

    static void hostKeysPerInstance() throws Exception {
        // No process-wide registration: every client has its own keystore.
        var first = new AppKeys();
        var second = new AppKeys();
        try (var a = signedClient("app-key"); var b = signedClient("app-key"); var c = signedClient("app-key")) {
            ClientHost.register(a, first);
            ClientHost.register(b, second);
            CompletableFuture.allOf(a.connect(), b.connect()).join();
            check(a.keyFingerprint().join().equals(first.fingerprint()), "a signs with the first key");
            check(b.keyFingerprint().join().equals(second.fingerprint()), "b signs with the second key");
            // An instance without its own registration falls back to the process-wide one (here: none).
            check(failsWith(ClientException.class, c.connect()).error() instanceof ClientError.KeyError, "no keystore for the third client");
            // Unregistering one instance does not touch the other.
            ClientHost.unregister(a);
            check(failsWith(ClientException.class, a.getCustomer("4711")).error() instanceof ClientError.KeyError, "a lost its keystore");
            check(b.getCustomer("4711").join().id().equals("4711"), "b still signs");
        }
    }

    static void hostCallCancellation() throws Exception {
        var app = new AppKeys();
        ClientHost.register(app);
        try (var c = signedClient("slow-key")) {
            var connecting = c.connect();
            Thread.sleep(100);
            connecting.cancel(true);
            failsWith(CancellationException.class, connecting);
            // The core told the app that nobody waits for the public key any more.
            try {
                app.slow.get(5, TimeUnit.SECONDS);
                throw new AssertionError("the app's stage should have been cancelled");
            } catch (CancellationException expected) {
                // cancelled by the runtime
            }
        } finally {
            ClientHost.unregister();
        }
    }

    // ----------------------------------------------------------------- runner

    public static void main(String[] args) {
        System.out.println("core under test: " + NativeBackend.shared().description());
        record T(String name, Test test) {}
        var tests = List.of(
                new T("syncCall", DemoClientTests::syncCall),
                new T("asyncCall", DemoClientTests::asyncCall),
                new T("asyncError", DemoClientTests::asyncError),
                new T("cancellation", DemoClientTests::cancellation),
                new T("stream", DemoClientTests::stream),
                new T("streamCancellation", DemoClientTests::streamCancellation),
                new T("objectLifetime", DemoClientTests::objectLifetime),
                new T("doubleDrop", DemoClientTests::doubleDrop),
                new T("concurrentCalls", DemoClientTests::concurrentCalls),
                new T("callbackDuringDispose", DemoClientTests::callbackDuringDispose),
                new T("coreVersionMismatch", DemoClientTests::coreVersionMismatch),
                new T("largePayload", DemoClientTests::largePayload),
                new T("unicodeStrings", DemoClientTests::unicodeStrings),
                new T("resourceHandles", DemoClientTests::resourceHandles),
                new T("hostKeysNotRegistered", DemoClientTests::hostKeysNotRegistered),
                new T("signedSession", DemoClientTests::signedSession),
                new T("concurrentSignedCalls", DemoClientTests::concurrentSignedCalls),
                new T("hostErrors", DemoClientTests::hostErrors),
                new T("hostKeysPerInstance", DemoClientTests::hostKeysPerInstance),
                new T("hostCallCancellation", DemoClientTests::hostCallCancellation));
        int failed = 0, passed = 0;
        for (var t : tests) {
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

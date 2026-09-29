// Plain application code against the generated Java SDK. Which core runs
// underneath (Rust, Go, Zig) is decided by the native library — not here.
import com.acme.client.*;

import java.nio.charset.StandardCharsets;
import java.util.Optional;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.Flow;

public final class Example {
    public static void main(String[] args) throws Exception {
        var options = new ClientOptions(args.length > 0 ? args[0] : "mem://example", Optional.of("demo-key"), 5_000);
        try (var client = new AcmeClient(options)) {
            var gotMessage = new CompletableFuture<Void>();
            client.events().subscribe(new Flow.Subscriber<>() {
                Flow.Subscription s;
                public void onSubscribe(Flow.Subscription s) { this.s = s; s.request(Long.MAX_VALUE); }
                public void onNext(ClientEvent evt) {
                    System.out.println("  event: " + evt);
                    if (evt instanceof ClientEvent.MessageReceived) { s.cancel(); gotMessage.complete(null); }
                }
                public void onError(Throwable t) { gotMessage.completeExceptionally(t); }
                public void onComplete() {}
            });

            client.connect()
                  .thenCompose(v -> client.getCustomer("4711"))
                  .thenAccept(c -> System.out.println("customer: " + c.name() + " <" + c.email().orElse("-") + ">"))
                  .join();

            CompletableFuture<Customer> missing = client.getCustomer("0000");
            missing.exceptionally(e -> {
                // Dependent stages see CompletionException(cause); the source stage sees the cause itself.
                var cause = e instanceof java.util.concurrent.CompletionException ? e.getCause() : e;
                if (cause instanceof ClientException ce && ce.error() instanceof ClientError.NotFound) {
                    System.out.println("customer 0000: not found");
                }
                return null;
            }).join();

            System.out.println("created: " + client.createCustomer("Max Mustermann", Optional.empty()).join());
            client.sendCommand(new Command("broadcast", "hello from Java".getBytes(StandardCharsets.UTF_8))).join();
            gotMessage.join();
            client.disconnect().join();
            System.out.println("state: " + client.state());
        }
    }
}

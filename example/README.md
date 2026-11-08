# Beispiel: HTTP-Client mit Verschlüsselung und Mock-Service

Ein kleiner, vollständiger Anwendungsfall für `wabi-wit-gen`: ein Vertrag, drei
Cores, fünf SDKs, ein Mock-Service.

```
wit/client.wit        der Vertrag (+ abi-ids.lock, api-baseline.snapshot)
generate.sh           ruft den Generator auf; alles unter sdk/ ist sein Output
sdk/                  GENERIERT: pro Sprache ein fertiges Paket (csharp, java, swift, dart, ts),
                      dazu abi/, c/, der Glue und die Runtime für die Cores (core-*/)
                      und nach dem Bauen die Bibliotheken (native/)
core-rust/            Core in Rust (tokio, reqwest) + der Mock-Service
core-go/              Core in Go (net/http), dieselbe API und dasselbe Protokoll
core-csharp/          Core in C# (HttpClient), als NativeAOT-Bibliothek
apps/                 dieselbe kleine App in C#, Java, Swift, Dart, Flutter, TypeScript
tests/                Tests in C (ABI-Konformität, Host-Calls), C#, Java, Swift, Dart, TypeScript
                      und der Matrix-Runner
```

## Was der Client tut

```wit
resource client {
    constructor(options: client-options);       // endpoint, api-key, key-id, timeout
    state: func() -> connection-state;          // synchron
    connect: async func() -> result<_, client-error>;
    disconnect: async func() -> result<_, client-error>;
    key-fingerprint: async func() -> result<string, client-error>;
    get-customer: async func(id: string) -> result<customer, client-error>;
    create-customer: async func(name: string, email: option<string>) -> result<customer, client-error>;
    send-command: async func(command: command) -> result<command-result, client-error>;
    events: func() -> stream<client-event>;
    use-transport: func(transport: borrow<transport>);      // Ressource als Parameter
}
resource transport {                                         // gemeinsamer Verbindungspool
    constructor(endpoint: string);
    request-count: func() -> u64;
    open-client: func(options: client-options) -> client;   // Ressource als Ergebnis
    absorb: func(other: transport);                          // Besitz geht in den Core über
}
```

* **Verschlüsselung:** Jeder Body ist `Nonce || AES-256-GCM(JSON)`. Der Schlüssel
  ist `HMAC-SHA256(api-key, "demo-client/v1 body key")`; der `api-key` selbst
  geht nie über die Leitung. Wer den falschen Schlüssel hat, bekommt
  `unauthorized`.
* **Signatur über Host-Calls:** Ist `key-id` gesetzt, fragt der Core die App
  über das importierte Interface `host-keys` nach dem öffentlichen Schlüssel
  (einmal pro Session) und nach einer ECDSA-P-256-Signatur (für jeden Request,
  über Methode, Pfad, Session und Body). Der private Schlüssel bleibt in der
  App. Das Backend prüft jede Signatur; `key-fingerprint` liefert, welchen
  Schlüssel es gesehen hat. Der Schlüsselspeicher wird prozessweit registriert
  (`ClientHost.register(keys)`) oder für einen einzelnen Client
  (`register(client, keys)` bzw. `registerFor`); bricht die App einen Aufruf ab,
  erfährt auch der Schlüsselspeicher davon.
* **Ereignisse:** `connect` öffnet eine Session und eine langlebige Antwort
  (Server-Sent Events, jedes Ereignis verschlüsselt). `events()` liefert sie
  als Stream, zusammen mit den Zustandswechseln der Session.

Das HTTP-Protokoll steht in [`core-rust/src/protocol/mod.rs`](core-rust/src/protocol/mod.rs).

## Mock-Service

Der Mock ([`core-rust/src/mock`](core-rust/src/mock/mod.rs)) spricht das echte
Protokoll: Er entschlüsselt, verschlüsselt und prüft Signaturen. Es gibt ihn
zweimal:

* **in-process** hinter `mem://…`-Endpoints des Rust-Cores, damit Tests und Apps
  auf jeder Plattform ohne Netzwerk laufen (auch im iOS-Simulator),
* **über HTTP** als `demo-mock-server`:

```bash
cargo run --manifest-path core-rust/Cargo.toml --features mock-server --bin demo-mock-server -- 127.0.0.1:9001
# Endpoint http://127.0.0.1:9001, api-key demo-key
```

Jede Session hat ihre eigenen Kunden (`4711` existiert). Kommandos für Tests:
`echo`, `broadcast` (erzeugt ein Ereignis), `sleep <ms>`, `fail`, `hang-up`
(beendet die Session ohne Antwort).

## Ausprobieren

```bash
tests/compatibility/run.sh                   # baut alle Cores und den Mock, testet alle SDKs

# einzelne Apps (Rust-Core, Mock in-process)
cargo build --release --manifest-path core-rust/Cargo.toml
export WABI_NATIVE_LIB=$PWD/core-rust/target/release/libdemo_client.so   # macOS: .dylib
dotnet run --project apps/csharp
bash sdk/java/build.sh && javac --release 22 -cp sdk/java/build/demo-client.jar -d build/ex apps/java/Example.java \
  && java --enable-native-access=ALL-UNNAMED -cp sdk/java/build/demo-client.jar:build/ex Example
(cd apps/dart && dart pub get && dart run bin/main.dart)
(cd apps/ts && bun install) && bun apps/ts/main.ts

# dieselbe App gegen den Go-Core und den Mock über HTTP
(cd core-go && go build -buildmode=c-shared -o ../build/go/libdemo_client.so .)
WABI_NATIVE_LIB=$PWD/build/go/libdemo_client.so dotnet run --project apps/csharp -- http://127.0.0.1:9001
```

Jede App hält den Signaturschlüssel selbst (`AppKeys`: .NET `ECDsa`, JDK
`Signature`, Security-Framework, pointycastle, `@noble/curves`) und zeigt den
Fingerprint, den das Backend geprüft hat. Flutter:
[apps/flutter](apps/flutter/README.md). Swift: `swift run -Xlinker -L<dir>
DemoExample` in `apps/swift`.

Für alle Plattformen baut `bash sdk/core-rust/build-native.sh core-rust <ziele>`
den Rust-Core nach `sdk/native/`; das Dart- und das NuGet-Paket nehmen die
Bibliotheken von dort.

## Den Vertrag ändern

```bash
./generate.sh               # schreibt sdk/, den Go-Glue in core-go/ und abi-ids.lock
./generate.sh --check       # CI
```

Eine neue Methode in `client.wit` bekommt die nächste freie ID; die SDKs sind
danach fertig. Von Hand bleibt nur die Implementierung in den Cores: Der
Rust-Compiler meldet das fehlende Trait-Item, Go und C# die fehlende
Interface-Methode. Ändert sich die Signatur einer Methode, bekommt sie eine neue
ID und die alte wird in `abi-ids.lock` stillgelegt. Inkompatible Änderungen lehnt der Generator ab, solange die
Hauptversion gleich bleibt ([docs/API_COMPATIBILITY.md](../docs/API_COMPATIBILITY.md)).

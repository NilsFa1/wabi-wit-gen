# Acme Client SDK — WIT-first, sprachübergreifend, Core austauschbar

Proof of Concept für ein Client-SDK, dessen **öffentlicher Vertrag WIT** ist
und dessen **Implementierungssprache austauschbar** bleibt.

```
                        wit/client.wit                 ← öffentlicher Vertrag
                              │  acme-wit-gen
          ┌───────────────────┼───────────────────┐
          ▼                   ▼                   ▼
       C# SDK              Java SDK            Swift SDK   ← generiert + generische Runtime
   Task / IAsyncEnum.  CompletableFuture /   async throws /
                       Flow.Publisher        AsyncSequence
          └───────────────────┼───────────────────┘
                              ▼
                 abi/acme_client.h  (17 Funktionen)       ← stabiles Implementierungsprotokoll
          ┌───────────────────┼───────────────────┐
          ▼                   ▼                   ▼
      Rust Core            Go Core             Zig Core   ← austauschbar
   (tokio, WebSocket)   (goroutines)        (eager Stub)
```

Später zusätzlich: `client.wit → Canonical ABI → client.component.wasm`
(siehe [docs/COMPONENT_MODEL.md](docs/COMPONENT_MODEL.md)).

## Was nachgewiesen ist

`tests/compatibility/run.sh` führt **dieselben, unveränderten** Test-Binaries
gegen alle drei Cores aus:

| Core | C ABI Conformance | C# SDK (13 Tests) | Java SDK (13 Tests) |
|---|---|---|---|
| Rust (`core-rust`, tokio) | ✅ ~2950 Checks | ✅ 13/13 | ✅ 13/13 |
| Go (`alt-cores/go`, cgo) | ✅ ~2980 Checks | ✅ 13/13 | ✅ 13/13 |
| Zig (`alt-cores/zig`, eager Stub) | ✅ basic-Profil | ✅ 7/7 basic | ✅ 7/7 basic |

Abgedeckt: Sync Call, Async Call, Async Error, Cancellation, Cancel/Completion-
Race, Stream, Stream Cancellation, Object Lifetime, Double Drop, Concurrent
Calls (500 parallel), Callback während Dispose, Core-Version-Mismatch (drei
absichtlich inkompatible Fake-Cores), 4 MiB Payloads, Unicode (inkl. Emoji,
CJK, Lone-Surrogates). Außerdem Rust-Tests inkl. echtem WebSocket-Roundtrip
und Generator-Tests für ID-Stabilität.

Das „basic“-Profil lässt beim Zig-Stub die Tests aus, die ein Backend mit
Events, Befehlen und langlaufenden Operationen brauchen — der Stub
implementiert bewusst nur Konstruktor, `state`, `connect`/`disconnect` und
`get-customer`.

**Swift** wird generiert und ist vollständig ausgeschrieben (Package,
Runtime, Tests, Beispiel), konnte in dieser Linux-Umgebung aber **nicht
kompiliert** werden (keine Swift-Toolchain verfügbar). Der CI-Workflow
`.github/workflows/client-sdk.yml` baut und testet es auf macOS.

## Schnellstart

```bash
scripts/test-all.sh                     # Generator, Header, Rust, Matrix aller Cores
tests/compatibility/run.sh              # nur die Kompatibilitätsmatrix
CORES="rust go" tests/compatibility/run.sh

cd generator && cargo run -- --root ..  # nach Änderungen an wit/client.wit
cargo run -- --root .. --check          # CI: Lock-Datei/generierter Code aktuell?

scripts/build-native.sh all             # Artefakte je Plattform (siehe unten)

# Beispiele (Endpoint mem://… = In-Process-Backend, ws://… = echter WebSocket)
export ACME_NATIVE_LIB=$PWD/core-rust/target/release/libacme_client.so
dotnet run --project examples/csharp
sdk/java/build.sh && javac --release 22 -cp build/java/acme-client.jar -d build/ex examples/java/Example.java \
  && java --enable-native-access=ALL-UNNAMED -cp build/java/acme-client.jar:build/ex Example
cargo run --manifest-path core-rust/Cargo.toml --bin acme-mock-server   # ws://127.0.0.1:9001
```

Benötigt: Rust ≥ 1.80, .NET 8, JDK ≥ 22 (Panama FFM ist ab 22 final), Go ≥ 1.22,
Zig 0.16 (optional), Swift ≥ 5.9 (macOS/Linux).

## Die öffentliche API in den Zielsprachen

```csharp
var client = new AcmeClient(options);
await client.ConnectAsync();
Customer customer = await client.GetCustomerAsync("4711");
await foreach (var evt in client.Events(cancellationToken)) { … }
await client.DisconnectAsync();
// Fehler: catch (ClientException e) when (e.Error is ClientError.NotFound)
```

```java
var client = new AcmeClient(options);
client.connect().thenCompose(v -> client.getCustomer("4711"));
CompletableFuture<Customer> customer = client.getCustomer("4711");
Flow.Publisher<ClientEvent> events = client.events();
// Fehler: CompletionException(ClientException(error = ClientError.NotFound))
```

```swift
let client = AcmeClient(options: options)
try await client.connect()
let customer = try await client.getCustomer(id: "4711")
for try await event in client.events() { … }
// Fehler: catch ClientError.notFound
```

## Projektstruktur

```
wit/client.wit            öffentlicher Vertrag (async func, stream<T>, resource)
wit/abi-ids.lock          append-only Tabelle aller numerischen Wire-IDs
abi/acme_client.h         die native ABI (17 Funktionen)
abi/ABI.md                normative Spezifikation: Ownership, Async, Streams, Threading, Encoding
generator/                acme-wit-gen (Rust, nutzt wit-parser): WIT → C#, Java, Swift, Rust-Core-Glue
generated/{csharp,java,swift}   generierter SDK-Code (committed, --check in CI)
runtime/{csharp,java,swift}     handgeschriebene, generische SDK-Runtime
sdk/{csharp,java,swift}   Paketierung: Runtime + generierter Code (.csproj, build.sh, Package.swift)
core-rust/                Rust-Core
  src/api/                  idiomatischer async-Rust-Client (die eigentliche Logik)
  src/protocol/             Backend-Frames (JSON über WebSocket)
  src/runtime/              Request/Response-Korrelation, Event-Hub
  src/transport/            Transport-Abstraktion (+ mem:// Loopback)
  src/websocket/            tokio-tungstenite
  src/crypto/               HMAC-SHA256 Challenge/Response
  src/mock/                 Mock-Backend (Tests, Beispiele, acme-mock-server)
  src/generated/            aus WIT generiert: Typen, Codec, Trait, Dispatch
  src/ffi/                  generischer ABI-Adapter — die einzige Datei mit extern "C"
alt-cores/go/             alternativer Core in Go (cgo, c-shared)
alt-cores/zig/            alternativer Stub-Core in Zig (eager, single-threaded)
examples/{csharp,java,swift}
tests/abi/conformance.c   sprachneutrale ABI-Konformität (dlopen, jeder Core)
tests/compatibility/      Matrix-Runner + absichtlich inkompatible Fake-Cores
tests/{csharp,java}       SDK-Testsuiten (Swift: sdk/swift/Tests)
scripts/                  test-all.sh, build-native.sh, zig-cc.sh
docs/COMPONENT_MODEL.md   Weg zu client.component.wasm
```

## Kernentscheidungen (Details in [abi/ABI.md](abi/ABI.md))

* **Buffer-Payloads statt typisierter FFI-Structs** (§5.1): `method_id + bytes`
  in einer MessagePack-Teilmenge. Die ABI bleibt bei 17 Funktionen, egal wie
  groß das WIT wird. Records werden als Maps mit Feld-Tags kodiert, sodass
  optionale Felder in beide Richtungen evolvieren können.
* **Future = Signal + `take`** (§4.3): Der Callback meldet nur „fertig“, das
  Ergebnis holt der Host mit `acme_future_take`. Das ist Ownership-einfach,
  der Callback ist O(1), es gibt kein Reentrancy-Problem und Polling
  funktioniert ohne Callback.
* **Erster Übergang gewinnt** (§4.1): löst jede Cancel/Completion-Race; der
  Callback feuert immer genau einmal, damit gibt der Host dort `user_data` frei.
* **Nie wiederverwendete Handles** (§3): Double-Free und Stale-Handles werden
  erkannt statt UB.
* **Stabile IDs über Lock-Datei** (§10): Die Reihenfolge im WIT ist egal,
  entfernte IDs werden nie neu vergeben — beides durch Generator-Tests
  abgesichert.
* **Drei Fehlerebenen** (§9): ABI-Status ≠ Domain-Fehler (Payload `[1, E]`)
  ≠ Abbruch.
* **Keine Thread-Annahmen** (§6): Callbacks kommen von Core-Threads; die SDKs
  dispatchen selbst und setzen nie einen UI-Thread voraus.

## Native Artefakte

`scripts/build-native.sh` legt ab:

| Plattform | Artefakt | In diesem PoC gebaut |
|---|---|---|
| Linux x64 | `build/native/linux-x64/libacme_client.so` | ✅ |
| Linux arm64 | `build/native/linux-arm64/libacme_client.so` (Zig als Cross-Linker) | ✅ |
| Windows x64 | `build/native/win-x64/acme_client.dll` (MSVC oder mingw) | ✅ (mingw, 17 Exporte geprüft) |
| macOS | `build/native/osx/libacme_client.dylib` (universal) | Skript, braucht macOS |
| iOS | `build/native/apple/AcmeClient.xcframework` (Device + Simulator + macOS, inkl. Header/Modulemap) | Skript, braucht Xcode |
| Android | `build/native/android/jniLibs/<abi>/libacme_client.so` (arm64-v8a, armeabi-v7a, x86_64, x86) | Skript, braucht NDK + cargo-ndk |

Ein Go- oder Zig-Core muss lediglich Dateien mit denselben Namen liefern.

## Neue WIT-Methode = kein handgeschriebener Glue-Code

Ausprobiert mit einer Kopie des Repos:
`ping: async func(payload: list<u8>) -> result<u32, client-error>;` einfügen →
`acme-wit-gen` vergibt `0x0108` (angehängt, keine andere ID ändert sich) und
erzeugt `Task<uint> PingAsync(byte[], CancellationToken)`,
`CompletableFuture<Integer> ping(byte[])` und
`func ping(payload: [UInt8]) async throws -> UInt32`. C#- und Java-SDK bauen
ohne Änderung an `runtime/`. Der Rust-Core meldet als Einziges
`error[E0046]: not all trait items implemented, missing: ping` — die
Geschäftslogik ist die einzige Stelle, an der ein Mensch etwas schreiben muss.

---

## Zusammenfassung: wer ist wofür zuständig?

### Was wird aus WIT generiert?
Von `acme-wit-gen`, aus `wit/client.wit` + `wit/abi-ids.lock`:

* **C#:** `sealed record`s, Enums (Wert = Wire-Tag), Variants als geschlossene
  `abstract record`-Hierarchien, `ClientException`, der Payload-Codec
  (`WireCodec`), die Client-Klasse `AcmeClient` (`Task`, `IAsyncEnumerable`,
  `CancellationToken`), `MethodIds`, `Contract`, die P/Invoke-Deklarationen.
* **Java:** Records, Enums, `sealed interface`s, `ClientException`, Codec,
  `AcmeClient` (`CompletableFuture`, `Flow.Publisher`), `MethodIds`,
  `Contract`, Panama-FFM-Downcall-Bindings (`NativeAbi`).
* **Swift:** `struct`s, `enum`s (mit Associated Values), `Error`-Konformität,
  Codec-Extensions, `AcmeClient` (`async throws`, `AcmeStream`), ABI-Konstanten.
* **Rust-Core:** Typen, Codec, ein Trait pro Ressource (`ClientResource`), der
  `method_id`-Dispatcher.
* **Lock-Datei:** neue IDs werden angehängt.

Die P/Invoke- und FFM-Tabellen kommen aus einer einzigen ABI-Tabelle im
Generator, die gegen `acme_client.h` geprüft wird. Sie wachsen nicht mit dem WIT.

### Was ist handgeschrieben?
* `wit/client.wit` (der Vertrag), `abi/acme_client.h` + `ABI.md`.
* Pro Sprache genau eine **generische Runtime** (`runtime/*`): MessagePack,
  Object-Handles, Buffer-Ownership, Future→Task/CompletableFuture/async,
  Stream→IAsyncEnumerable/Flow/AsyncSequence, Cancellation, Fehlerdekodierung,
  Versions- und Capability-Prüfung. Nichts darin kennt `Customer` oder `connect`.
* Der Generator selbst.
* Die Core-Logik (`core-rust/src/api` usw.) und die alternativen Cores.

### Was ist Core-spezifisch?
Alles unterhalb der ABI: Geschäftslogik, Tokio vs. Goroutinen vs. eager,
WebSocket-Implementierung, Backend-Protokoll (JSON-Frames), Korrelation,
Krypto, Handle-Tabelle, Future-Zustandsautomat, Allocator (`Box<[u8]>` /
`C.malloc` / `c_allocator`) und damit auch `acme_buffer_free`.

### Was ist ABI-spezifisch?
`acme_client.h` / `ABI.md`: 17 Funktionen, Status- und Capability-Codes,
Handle-Regeln, Future/Stream-Protokoll, Threading-Regeln, Payload-Encoding v1,
Method-ID-Schema. Sowohl Cores als auch SDK-Runtimes implementieren das; es ist
die einzige gemeinsame Abhängigkeit.

### Was ist sprachspezifisch?
Nur die SDK-Runtime-Abbildung auf die Idiome der Sprache: C# `GCHandle` +
`UnmanagedCallersOnly` + `TaskCompletionSource`; Java Panama-Upcall + Key-Map
+ `CompletableFuture`-Unterklasse + Reactive-Streams-konformer Publisher;
Swift `Unmanaged` + `CheckedContinuation` + `withTaskCancellationHandler`.
Dazu kommen die Namens- und Typabbildung im jeweiligen Emitter des Generators.

### Was bleibt bei Rust → Go/Zig unverändert?
**Alles oberhalb der ABI:** `client.wit`, `abi-ids.lock`, `acme_client.h`,
der gesamte generierte SDK-Code, die SDK-Runtimes, die Pakete, die Beispiele
und die Testsuiten, und zwar bis auf die **kompilierten Binaries**. Die
Matrix lädt dieselbe `Acme.Client.Tests.dll` und dieselbe `acme-client.jar`
gegen `libacme_client.so` aus Rust, Go und Zig. Ersetzt wird ausschließlich
`core-rust/` (Logik + `src/ffi`-Adapter) durch `alt-cores/go/`
(Logik + `abi.go`/`dispatch.go`) bzw. `alt-cores/zig/`.

## Bekannte Grenzen des PoC

* **Swift** ist nicht lokal kompiliert (siehe oben; CI auf macOS).
* **Android + Java:** Android hat kein Panama FFM. Dafür braucht es ein
  `JniBackend` (ebenfalls generisch: ein kleiner C-Shim leitet die 17
  ABI-Funktionen weiter). Die generierten Klassen bleiben gleich, weil sie nur
  `ClientBackend` sehen. Das Java-SDK unterstützt nur 64-Bit-Targets.
* **Go-Dispatch ist handgeschrieben:** Der Generator hat heute nur einen
  Rust-Core-Emitter; ein Go-Emitter würde `dispatch.go` erzeugen.
* **WIT-Teilmenge:** `future<T>` als *Wert*, Ressourcen-Handles als Parameter,
  statische und freie Funktionen, `flags` und in Java auch Tupel meldet der
  Generator als „nicht unterstützt“. `async func` und `stream<T>` sind voll
  unterstützt.
* **`wss://`:** Der Rust-Core baut tokio-tungstenite ohne TLS-Feature; für
  Produktion `rustls-tls-webpki-roots` aktivieren.
* **Unbekannte Variant-Cases** eines neueren Cores führen in älteren SDKs zu
  einem Protokollfehler. Ein generierter `Unknown`-Case wäre eine mögliche
  Erweiterung.

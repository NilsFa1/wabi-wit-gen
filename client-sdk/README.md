# Wabi Client SDK — WIT-first, sprachübergreifend, Core austauschbar

Proof of Concept für ein Client-SDK, dessen **öffentlicher Vertrag WIT** ist
und dessen **Implementierungssprache austauschbar** bleibt.

```
                        wit/client.wit                 ← öffentlicher Vertrag
                              │  wabi-wit-gen
      ┌─────────────────┬───────┴─────────┬─────────────────┐
      ▼                 ▼                 ▼                 ▼
   C# SDK            Java SDK          Swift SDK        Dart SDK      ← generiert + generische Runtime
 Task /            CompletableFuture/ async throws /   Future /        (Dart auch für Flutter)
 IAsyncEnumerable  Flow.Publisher     AsyncSequence    Stream
      └─────────────────┴───────┬─────────┴─────────────────┘
                              ▼
                 abi/wabi_abi.h  (17 Funktionen)          ← stabiles Implementierungsprotokoll
          ┌───────────────────┼───────────────────┐
          ▼                   ▼                   ▼
      Rust Core        Go Core        Zig Core       C# Core      ← austauschbar
   (tokio, WS)      (goroutines)   (eager Stub)   (NativeAOT)
```

Später zusätzlich: `client.wit → Canonical ABI → client.component.wasm`
(siehe [docs/COMPONENT_MODEL.md](docs/COMPONENT_MODEL.md)).

## Was nachgewiesen ist

`tests/compatibility/run.sh` führt **dieselben, unveränderten** Test-Binaries
(C#, Java) bzw. dieselbe Testsuite (Dart) gegen alle drei Cores aus:

| Core | C ABI Conformance | C# SDK (13 Tests) | Java SDK Panama (13) | Java SDK JNI / Android-Pfad (13) | Dart SDK (13 Tests) |
|---|---|---|---|---|---|
| Rust (`core-rust`, tokio) | ✅ ~2950 Checks | ✅ 13/13 | ✅ 13/13 | ✅ 13/13 | ✅ 13/13 |
| Go (`alt-cores/go`, cgo) | ✅ ~2980 Checks | ✅ 13/13 | ✅ 13/13 | ✅ 13/13 | ✅ 13/13 |
| Zig (`alt-cores/zig`, eager Stub) | ✅ basic-Profil | ✅ 7/7 basic | ✅ 7/7 basic | ✅ 7/7 basic | ✅ 7/7 basic |
| **C#** (`alt-cores/csharp`, NativeAOT → `.so`) | ✅ ~2980 Checks | ✅ 13/13 | ✅ 13/13 | ✅ 13/13 | ✅ 13/13 |

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

### Host-Calls (ABI 1.1): der Core ruft die App — Beispiel gematik zero-lab

[`packages/zero`](packages/zero) ist ein zweites WIT-Paket (`gematik:zero`):
Der Core erzeugt DPoP-Proofs (RFC 9449), der private Schlüssel bleibt aber
**in der App** (Secure Enclave, Android Keystore, TPM …). Der Core ruft dazu das
importierte WIT-Interface `host-keys` auf (ABI.md §4a). Es gibt **drei
austauschbare Cores** mit denselben Symbolen (`libgematik_zero`):

* **Go** mit gematik libzero (`core-go/dpop.go` + generiertes `wabi_generated.go`),
* **Rust** (`core-rust/src/lib.rs` + generiertes `generated.rs`, Runtime-Crate `wabi-abi-rt`),
* **C#** NativeAOT (`core-csharp`, Runtime `runtime/csharp-core`).

`packages/zero/run-tests.sh` lässt jede App-Sprache gegen jeden Core laufen;
jede App spielt den Keystore mit ihrer Plattform-Kryptografie und verifiziert
Signatur und Thumbprint unabhängig:

| App-Sprache (Keystore im Test) | Go-Core (libzero) | Rust-Core | C#-Core |
|---|---|---|---|
| C, Protokolltest `tests/abi/host_calls.c` (OpenSSL) | ✅ 129 Checks | ✅ 129 Checks | ✅ 129 Checks |
| C# (`ECDsa`) | ✅ 6/6 | ✅ 6/6 | ✅ 6/6 |
| Java Panama (JDK `Signature`) | ✅ 6/6 | ✅ 6/6 | ✅ 6/6 |
| Java JNI, Android-Pfad | ✅ 6/6 | ✅ 6/6 | ✅ 6/6 |
| Dart/Flutter (pointycastle) | ✅ 6/6 | ✅ 6/6 | ✅ 6/6 |
| TypeScript/Bun (`@noble/curves`) | ✅ 12/12 | ✅ 12/12 | ✅ 12/12 |
| Swift (Security-Framework `SecKey`) | ⏳ nur macOS-CI | ⏳ | ⏳ |

Zig kann noch keine Host-Calls auslösen (siehe [docs/ROADMAP.md](docs/ROADMAP.md)).

### Java als Core-Sprache: gematik ref-GemLibPki als native Bibliothek

> **Hinweis:** Der Java-Core wird **nicht auf allen Plattformen unterstützt**. Er
> wird nur unter Linux gebaut und dient als **Referenz** für Vergleichstests;
> Android und iOS gehen mit GraalVM native-image nicht (der Versuch über Gluon
> Substrate wurde verworfen). Ausgeliefert wird der **Go-Core** (unten), der für
> alle Plattformen gebaut wird.

[`packages/pki`](packages/pki) (`gematik:pki`) hat einen **Core in Java**: einen
kleinen Wrapper um gematik [ref-GemLibPki](https://github.com/gematik/ref-GemLibPki)
mit einer Methode, der Zertifikatsprüfung nach TUC_PKI_018 gegen die CAs einer
TSL. GraalVM `native-image` kompiliert Wrapper, gemLibPki, BouncyCastle und
JAXB zu einer nativen Bibliothek; C#-, Java-, Dart- und Swift-Apps brauchen
**keine JVM**. Generiert werden Dispatcher und Schnittstellen (`wabi-wit-gen`
Target `java-core`); die generische Runtime liegt in `runtime/java-core`
(inkl. C-Shim für die GraalVM-Isolate), Details in
[packages/pki/core-java/README.md](packages/pki/core-java/README.md).

**Go-Core, der Standard-Core** ([packages/pki/core-go](packages/pki/core-go/README.md),
Lizenz **EUPL-1.2**): gematik zero-lab [gempki](https://github.com/gematik/zero-lab/tree/main/go/gempki)
für Parsing (inkl. Brainpool), TSL, Admission und OCSP, darüber die Prüfschritte
von TUC_PKI_018 nach gemLibPki 5.0.2 (gleiche Reihenfolge, Fehlercodes, Texte),
nur ECC. Eine Bibliothek von ~8–10 MB, per `build.sh` für Linux, Windows, macOS,
Android (`jniLibs`) und iOS (`.xcframework`) gebaut (`.github/workflows/pki-go.yml`).
Beide Cores laufen gegen dieselben, unveränderten SDK-Tests. Wie daraus Pakete
(NuGet, Maven/AAR, SwiftPM, pub.dev) und ein eigenes SDK-Repo werden:
[docs/PACKAGING.md](docs/PACKAGING.md).

| App-Sprache | Java-Core (GraalVM native-image) | Go-Core (gempki) |
|---|---|---|
| Core-eigene Tests | ✅ 8/8 (JVM) | ✅ `go test` |
| C# | ✅ 7/7 | ✅ 7/7 |
| Java Panama / JNI (Android-Pfad) | ✅ 7/7 / ✅ 7/7 | ✅ 7/7 / ✅ 7/7 |
| Dart/Flutter | ✅ 7/7 | ✅ 7/7 |
| Swift | ⏳ | ⏳ macOS-CI (`pki-go`) |

**Swift** wird generiert und ist vollständig ausgeschrieben (Package,
Runtime, Tests, Beispiel), konnte in dieser Linux-Umgebung aber **nicht
kompiliert** werden (keine Swift-Toolchain verfügbar). Der CI-Workflow
`.github/workflows/client-sdk.yml` baut und testet es auf macOS.

## Generator per npm (eigene Projekte)

Für eigene Verträge braucht es dieses Repository nicht: `wabi-wit-gen` gibt es
als npm-Paket `@nfails/wabi-wit-gen` (der Generator als WebAssembly, läuft mit Node ≥ 20 und Bun auf
jeder Plattform).

```bash
npx @nfails/wabi-wit-gen wit/ --out sdk --target ts,dart,java,csharp,swift --core rust
```

`--target` wählt die SDK-Sprachen, `--core` die Sprache des Cores (`rust`, `go`,
`csharp`, `java`). In `sdk/` landen die ABI-Header, die SDKs samt Runtime und
der Core-Glue samt Core-Runtime (z. B. das Crate `wabi-abi-rt`). Für Dart ist das
ein vollständiges Paket (`flutter pub add <paket> --path sdk/dart`), für C# ein
Projekt, das `dotnet pack` zu einem NuGet-Paket mit den nativen Bibliotheken
macht. Rust- und Go-Cores baut `sdk/core-<lang>/build-native.sh` für alle
Plattformen nach `sdk/native/`, von wo der Dart-Build-Hook (Flutter) und das
NuGet-Paket sie ohne weitere Konfiguration übernehmen. Details,
Optionen und ein lauffähiges Beispiel (WIT → Rust-Core → TypeScript in Bun):
[generator/npm/README.md](generator/npm/README.md). Bauen und testen:
`generator/npm/build.sh`, `generator/npm/test.sh`; veröffentlicht wird per Tag
`wabi-wit-gen-v<version>` (`.github/workflows/wabi-wit-gen-release.yml`).

## Schnellstart

```bash
scripts/test-all.sh                     # Generator, Header, Rust, Matrix aller Cores
tests/compatibility/run.sh              # nur die Kompatibilitätsmatrix
CORES="rust go" tests/compatibility/run.sh

cd generator && cargo run -- --root ..  # nach Änderungen an wit/client.wit
cargo run -- --root .. --check          # CI: Lock-Datei/generierter Code aktuell?

scripts/build-native.sh all             # Artefakte je Plattform (siehe unten)

# Beispiele (Endpoint mem://… = In-Process-Backend, ws://… = echter WebSocket)
export WABI_NATIVE_LIB=$PWD/core-rust/target/release/libwabi_client.so
dotnet run --project examples/csharp
sdk/java/build.sh && javac --release 22 -cp build/java/wabi-client.jar -d build/ex examples/java/Example.java \
  && java --enable-native-access=ALL-UNNAMED -cp build/java/wabi-client.jar:build/ex Example
cargo run --manifest-path core-rust/Cargo.toml --bin wabi-mock-server   # ws://127.0.0.1:9001
```

Benötigt: Rust ≥ 1.80, .NET 8, JDK ≥ 22 (Panama FFM ist ab 22 final), Dart ≥ 3.3,
Go ≥ 1.22, Zig 0.16 (optional), Swift ≥ 5.9 (macOS/Linux).

```bash
(cd examples/dart && dart pub get && dart run bin/main.dart)
(cd sdk/dart && dart test)                               # WABI_TEST_PROFILE=basic für den Zig-Stub
```

## Die öffentliche API in den Zielsprachen

```csharp
var client = new WabiClient(options);
await client.ConnectAsync();
Customer customer = await client.GetCustomerAsync("4711");
await foreach (var evt in client.Events(cancellationToken)) { … }
await client.DisconnectAsync();
// Fehler: catch (ClientException e) when (e.Error is ClientError.NotFound)
```

```java
var client = new WabiClient(options);
client.connect().thenCompose(v -> client.getCustomer("4711"));
CompletableFuture<Customer> customer = client.getCustomer("4711");
Flow.Publisher<ClientEvent> events = client.events();
// Fehler: CompletionException(ClientException(error = ClientError.NotFound))
```

```swift
let client = WabiClient(options: options)
try await client.connect()
let customer = try await client.getCustomer(id: "4711")
for try await event in client.events() { … }
// Fehler: catch ClientError.notFound
```

```dart
final client = WabiClient(options);
await client.connect();
final customer = await client.getCustomer('4711');
await for (final event in client.events()) { … }            // Stream<ClientEvent>
await client.getCustomer('4711', cancellationToken: token);   // optionaler Abbruch
// Fehler: on ClientErrorNotFound { … } bzw. switch (e) { ClientErrorNotFound() => … }
```

Für Flutter siehe [docs/FLUTTER.md](docs/FLUTTER.md): Die Core-Bibliothek
bündelt der Build-Hook des Pakets (Dart native assets) automatisch; dazu
Isolates/UI und ein Beispiel-Widget ([examples/flutter](examples/flutter)).

## Projektstruktur

```
wit/client.wit            öffentlicher Vertrag (async func, stream<T>, resource)
wit/abi-ids.lock          append-only Tabelle aller numerischen Wire-IDs
abi/wabi_abi.h            die native ABI: Typen, Status, Capabilities
abi/wabi_abi_functions.h  die 17 Funktionen, pro WIT-Paket unter eigenem Symbol-Präfix
generated/c/wabi_client.h generierter Header des Pakets (wabi_client_call_sync, …)
abi/ABI.md                normative Spezifikation: Ownership, Async, Streams, Threading, Encoding
generator/                wabi-wit-gen (Rust, nutzt wit-parser): WIT → C#, Java, Swift, Dart, TypeScript, Rust-/Go-Core-Glue
generator/npm/            npm-Paket wabi-wit-gen (Generator als WebAssembly/WASI + Launcher, Beispiel, Tests)
generator/runtime/{csharp,java,swift,dart,ts}   handgeschriebene, generische App-Runtimes; in den Generator
                          eingebettet und bei jedem Lauf in generated/<lang> geschrieben (siehe unten)
generated/{csharp,java,swift,dart,ts}   generierter SDK-Code inkl. Runtime (committed, --check in CI)
sdk/{csharp,java,swift,dart}   Paketierung des generierten Codes (.csproj, build.sh, Package.swift, pubspec.yaml)
core-rust/                Rust-Core
  src/api/                  idiomatischer async-Rust-Client (die eigentliche Logik)
  src/protocol/             Backend-Frames (JSON über WebSocket)
  src/runtime/              Request/Response-Korrelation, Event-Hub
  src/transport/            Transport-Abstraktion (+ mem:// Loopback)
  src/websocket/            tokio-tungstenite
  src/crypto/               HMAC-SHA256 Challenge/Response
  src/mock/                 Mock-Backend (Tests, Beispiele, wabi-mock-server)
  src/generated/            aus WIT generiert: Typen, Codec, Trait, Dispatch
runtime/rust/             generischer ABI-Adapter für Rust-Cores (Crate wabi-abi-rt, inkl. Host-Calls)
runtime/go/               generischer ABI-Adapter für Go-Cores (Modul wabi.example/wabiabi, inkl. Host-Calls)
runtime/csharp-core/      generischer ABI-Adapter für C#-Cores (NativeAOT, inkl. Host-Calls)
alt-cores/go/             alternativer Core in Go (cgo, c-shared; Glue generiert)
alt-cores/zig/            alternativer Stub-Core in Zig (eager, single-threaded)
alt-cores/csharp/         alternativer Core in C#, per NativeAOT zu einer nativen .so kompiliert
runtime/java-core/        generische Runtime für Java-Cores (GraalVM native-image, C-Shim)
packages/pki/             3. WIT-Paket gematik:pki: Java-Core um gemLibPki (GraalVM), SDKs + Tests
packages/zero/            2. WIT-Paket gematik:zero (Host-Calls): Cores Go (libzero), Rust, C#; SDKs + Tests
examples/{csharp,java,swift,dart,flutter}
tests/abi/conformance.c   sprachneutrale ABI-Konformität (dlopen, jeder Core)
tests/compatibility/      Matrix-Runner + absichtlich inkompatible Fake-Cores
tests/{csharp,java}       SDK-Testsuiten (Swift: sdk/swift/Tests, Dart: sdk/dart/test)
scripts/                  test-all.sh, build-native.sh, zig-cc.sh
docs/COMPONENT_MODEL.md   Weg zu client.component.wasm
docs/FLUTTER.md           Dart-SDK in Flutter-Apps
docs/CORE_LANGUAGES.md    Bewertung: C# (NativeAOT, nachgewiesen) und Kotlin/Native als Core-Sprache
docs/ROADMAP.md           geplant: Generator-Emitter in Bun/TypeScript, JNI-Backend, …
docs/PACKAGING.md         Plan: gematik:pki als eigenes SDK-Repo mit NuGet/Maven/AAR/SwiftPM/pub-Paketen
```

## Die Runtime reist mit dem generierten Code

Die Runtimes für App-Sprachen (C#, Java inkl. JNI-Shim, Swift, Dart,
TypeScript) sind handgeschrieben und vertragsneutral. Sie liegen in
`generator/runtime/` und werden beim Bauen in `wabi-wit-gen` eingebettet
(`generator/build.rs`). Bei jedem Lauf schreibt der Generator sie neben den
generierten Code des Pakets, **im Namensraum des Pakets**:

| Sprache | Ziel | Namensraum / Name |
|---|---|---|
| C# | `generated/csharp/Runtime/` | `<Ns>.Runtime`, z. B. `Gematik.Pki.Runtime` |
| Java | `generated/java/<paket>/runtime/`, `generated/java/jni/` | `com.gematik.pki.runtime`; JNI-Shim `libgematik_pki_jni` |
| Swift | `generated/swift/Runtime/` | Modul des SDK; C-Modul `C<Modul>`, z. B. `CGematikPki` |
| Dart | `generated/dart/runtime/` | Bibliothek des SDK-Pakets |
| TypeScript | `generated/ts/runtime/` | Modul des SDK-Pakets |

Damit braucht ein SDK-Build nur den Generator (keine zweite Abhängigkeit, die
zur Generator-Version passen muss), und zwei mit `wabi-wit-gen` erzeugte SDKs,
etwa `gematik:zero` und `gematik:pki`, lassen sich in **einer** App nutzen, ohne
dass Klassen, Namespaces, C-Module oder JNI-Symbole kollidieren. Die Kopien
tragen den Hinweis „DO NOT EDIT“; geändert wird nur `generator/runtime/`.

Die Runtimes für Cores (`runtime/rust`, `runtime/go`, `runtime/csharp-core`,
`runtime/java-core`) bleiben Bibliotheken: Jeder Core ist eine eigene native
Bibliothek, Kollisionen gibt es dort nicht.

## Java: zwei Bindings, eine API

Die generierten Klassen und die gesamte Async-Logik sehen nur `AbiCalls` (die
17 ABI-Funktionen). Darunter liegen zwei austauschbare Bindings:

| | Panama FFM | JNI |
|---|---|---|
| Wo | JDK 22+ (Desktop/Server) | **Android** (ART) und JDK 17+ |
| Code | `PanamaAbi` + generiertes `NativeAbi` | `JniAbi` + C-Shim `generated/java/jni/<präfix>_jni.c` (`libwabi_client_jni`, je Paket eigener Name) |
| Artefakt | `build/java/wabi-client.jar` | `wabi-client-android.jar` (Java-17-Bytecode, ohne Panama-Klassen) + `libwabi_client_jni.so` je ABI (`generated/java/jni/CMakeLists.txt`) |
| Auswahl | automatisch; erzwingen mit `-Dwabi.binding=panama\|jni`; Shim-Pfad für Tests: `-D<paket>.runtime.jniLib=…` | |

Der Shim ist vertragsneutral: Er lädt den Core per `dlopen` und leitet die 17
Funktionen weiter. Neue WIT-Methoden und ein Wechsel der Core-Sprache
erfordern kein Neubauen des Shims. Callbacks von Core-Threads hängt er einmalig
als Daemon an die JVM und löst sie beim Thread-Ende wieder ab (Android bricht
sonst ab).

## Kernentscheidungen (Details in [abi/ABI.md](abi/ABI.md))

* **Buffer-Payloads statt typisierter FFI-Structs** (§5.1): `method_id + bytes`
  in einer MessagePack-Teilmenge. Die ABI bleibt bei 17 Funktionen, egal wie
  groß das WIT wird. Records werden als Maps mit Feld-Tags kodiert, sodass
  optionale Felder in beide Richtungen evolvieren können.
* **Future = Signal + `take`** (§4.3): Der Callback meldet nur „fertig“, das
  Ergebnis holt der Host mit `wabi_future_take`. Das ist Ownership-einfach,
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
| Linux x64 | `build/native/linux-x64/libwabi_client.so` | ✅ |
| Linux arm64 | `build/native/linux-arm64/libwabi_client.so` (Zig als Cross-Linker) | ✅ |
| Windows x64 | `build/native/win-x64/wabi_client.dll` (MSVC oder mingw) | ✅ (mingw, 17 Exporte geprüft) |
| macOS | `build/native/osx/libwabi_client.dylib` (universal) | Skript, braucht macOS |
| iOS | `build/native/apple/WabiClient.xcframework` (Device + Simulator + macOS, inkl. Header/Modulemap) | Skript, braucht Xcode |
| Android | `build/native/android/jniLibs/<abi>/libwabi_client.so` (arm64-v8a, armeabi-v7a, x86_64, x86) | Skript, braucht NDK + cargo-ndk |

Ein Go- oder Zig-Core muss lediglich Dateien mit denselben Namen liefern.

## Neue WIT-Methode = kein handgeschriebener Glue-Code

Ausprobiert mit einer Kopie des Repos:
`ping: async func(payload: list<u8>) -> result<u32, client-error>;` einfügen →
`wabi-wit-gen` vergibt `0x0108` (angehängt, keine andere ID ändert sich) und
erzeugt `Task<uint> PingAsync(byte[], CancellationToken)`,
`CompletableFuture<Integer> ping(byte[])` und
`func ping(payload: [UInt8]) async throws -> UInt32`. C#- und Java-SDK bauen
ohne Änderung an `runtime/`. Der Rust-Core meldet als Einziges
`error[E0046]: not all trait items implemented, missing: ping` — die
Geschäftslogik ist die einzige Stelle, an der ein Mensch etwas schreiben muss.

---

## Zusammenfassung: wer ist wofür zuständig?

### Was wird aus WIT generiert?
Von `wabi-wit-gen`, aus `wit/client.wit` + `wit/abi-ids.lock`:

* **C#:** `sealed record`s, Enums (Wert = Wire-Tag), Variants als geschlossene
  `abstract record`-Hierarchien, `ClientException`, der Payload-Codec
  (`WireCodec`), die Client-Klasse `WabiClient` (`Task`, `IAsyncEnumerable`,
  `CancellationToken`), `MethodIds`, `Contract`, die P/Invoke-Deklarationen.
* **Java:** Records, Enums, `sealed interface`s, `ClientException`, Codec,
  `WabiClient` (`CompletableFuture`, `Flow.Publisher`), `MethodIds`,
  `Contract`, Panama-FFM-Downcall-Bindings (`NativeAbi`).
* **Swift:** `struct`s, `enum`s (mit Associated Values), `Error`-Konformität,
  Codec-Extensions, `WabiClient` (`async throws`, `WabiStream`), ABI-Konstanten.
* **Dart:** unveränderliche Klassen mit Wertgleichheit, Enhanced Enums,
  `sealed class`-Hierarchien (Fehlertypen `implements Exception`), Codec,
  `WabiClient` (`Future`, `Stream`, `CancellationToken`), `dart:ffi`-Bindings.
* **TypeScript (Bun):** Interfaces für Records, String-Unions für Enums,
  diskriminierte Unions `{ tag, value }` für Variants, `<E>Exception`-Klassen,
  Codec, Client-Klasse (`Promise`, `AsyncIterableIterator`, `AbortSignal`,
  `Symbol.dispose`), Host-Registrierung und die `bun:ffi`-Bindings der ABI
  (Target `ts`; bisher für `packages/zero` eingeschaltet).
* **Rust-Core:** Typen, Codec, ein Trait pro Ressource (`ClientResource`), der
  `method_id`-Dispatcher.
* **Go-, C#- und Java-Core:** ebenso Interfaces pro Ressource und der
  Dispatcher; beim C#-Core (Target `csharp-core`) außerdem Symbol-Präfix,
  Contract-ID und typisierte Clients für die WIT-Imports (Host-Calls). Ein
  C#-Core besteht damit nur noch aus `Core : ICore` und der Geschäftslogik.
* **Lock-Datei:** neue IDs werden angehängt.

Die P/Invoke- und FFM-Tabellen kommen aus einer einzigen ABI-Tabelle im
Generator, die gegen `wabi_abi.h`/`wabi_abi_functions.h` geprüft wird. Sie wachsen nicht mit dem WIT.

### Was ist handgeschrieben?
* `wit/client.wit` (der Vertrag), `abi/wabi_abi.h`, `abi/wabi_abi_functions.h` + `ABI.md`.
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
`C.malloc` / `c_allocator`) und damit auch `wabi_buffer_free`.

### Was ist ABI-spezifisch?
`wabi_abi.h` / `wabi_abi_functions.h` / `ABI.md`: 17 Funktionen (mit Paket-Präfix), Status- und Capability-Codes,
Handle-Regeln, Future/Stream-Protokoll, Threading-Regeln, Payload-Encoding v1,
Method-ID-Schema. Sowohl Cores als auch SDK-Runtimes implementieren das; es ist
die einzige gemeinsame Abhängigkeit.

### Was ist sprachspezifisch?
Nur die SDK-Runtime-Abbildung auf die Idiome der Sprache: C# `GCHandle` +
`UnmanagedCallersOnly` + `TaskCompletionSource`; Java Panama-Upcall + Key-Map
+ `CompletableFuture`-Unterklasse + Reactive-Streams-konformer Publisher;
Swift `Unmanaged` + `CheckedContinuation` + `withTaskCancellationHandler`;
Dart `NativeCallable.listener` + `Completer` + pull-basierter `StreamController`
+ `Finalizer`.
Dazu kommen die Namens- und Typabbildung im jeweiligen Emitter des Generators.

### Was bleibt bei Rust → Go/Zig unverändert?
**Alles oberhalb der ABI:** `client.wit`, `abi-ids.lock`, die ABI-Header,
der gesamte generierte SDK-Code, die SDK-Runtimes, die Pakete, die Beispiele
und die Testsuiten, und zwar bis auf die **kompilierten Binaries**. Die
Matrix lädt dieselbe `Wabi.Client.Tests.dll` und dieselbe `wabi-client.jar`
gegen `libwabi_client.so` aus Rust, Go und Zig. Ersetzt wird ausschließlich
`core-rust/` (Logik + generierter Glue; der ABI-Adapter liegt paketunabhängig in `runtime/rust`) durch `alt-cores/go/`
(Logik `client.go` + generiertes `wabi_generated.go` + Runtime `runtime/go`) bzw. `alt-cores/zig/`.

## Bekannte Grenzen des PoC

* **Swift** ist nicht lokal kompiliert (siehe oben; CI auf macOS).
* **Android + Java:** Das JNI-Binding ist implementiert und auf dem Desktop-JVM
  gegen alle Cores getestet; auf einem echten Android-Gerät bzw. Emulator lief
  es noch nicht (kein Android-SDK in dieser Umgebung).
* **Host-Calls im Core:** Go, Rust und C# können Host-Calls auslösen, Zig noch
  nicht (Roadmap). Host-Calls kennen noch keinen eigenen Abbruch in Richtung App
  und keine Streams/Ressourcen als Parameter.
* **libzero:** Genutzt wird bisher nur `util.Jwk` (Thumbprint); das `dpop`-Paket
  von zero-lab ist in libzero v0.1.0 nicht veröffentlicht, der Proof entsteht
  deshalb mit jwx (das libzero selbst verwendet).
* **WIT-Teilmenge:** `future<T>` als *Wert*, Ressourcen-Handles als Parameter,
  statische und freie Funktionen, `flags` und in Java auch Tupel meldet der
  Generator als „nicht unterstützt“. `async func` und `stream<T>` sind voll
  unterstützt.
* **`wss://`:** Der Rust-Core baut tokio-tungstenite ohne TLS-Feature; für
  Produktion `rustls-tls-webpki-roots` aktivieren.
* **Unbekannte Variant-Cases** eines neueren Cores führen in älteren SDKs zu
  einem Protokollfehler. Ein generierter `Unknown`-Case wäre eine mögliche
  Erweiterung.

## Lizenz

Apache-2.0 (`LICENSE` und `NOTICE` im Wurzelverzeichnis). Ausnahme:
`packages/pki/core-go` steht unter EUPL-1.2 (baut auf gematik gempki auf, siehe
`packages/pki/core-go/README.md`). Die Go-Cores binden gematik zero-lab
(gempki, libzero, beide EUPL-1.2) ein; daraus gebaute native Bibliotheken
enthalten diesen Code.

**Generierter Code:** Was `wabi-wit-gen` erzeugt, einschließlich der Runtimes,
die der Generator neben den generierten Code kopiert (`generated/*/runtime`,
JNI-Shim, `wabi-abi-rt` usw.), gehört dem, der den Generator ausführt. Es darf
ohne Lizenzhinweis unter jeder Lizenz verwendet werden, auch in proprietären
Apps und SDKs.

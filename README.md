# UniWtiGen (`wabi-wit-gen`)

Ein Vertrag in [WIT](https://component-model.bytecodealliance.org/design/wit.html),
daraus **SDKs** für C#, Java/Android, Swift, Dart/Flutter und TypeScript (Bun) und
der **Glue-Code für den Core** in Rust, Go oder C#. Alle SDKs sprechen über
dieselbe stabile native ABI mit dem Core; die Logik steckt genau einmal im Core,
und dessen Sprache bleibt austauschbar.

```
                          client.wit                    ← öffentlicher Vertrag
                              │  wabi-wit-gen
      ┌────────────┬──────────┼───────────┬────────────┐
      ▼            ▼          ▼           ▼            ▼
   C# SDK      Java SDK   Swift SDK    Dart SDK     TS SDK       ← generiert, inkl. Runtime
   Task /      Completable async       Future /     Promise /
   IAsyncEnum. Future/Flow throws      Stream       AsyncIterator
      └────────────┴──────────┬───────────┴────────────┘
                              ▼
                 abi/wabi_abi.h  (17 Funktionen + 3 für Host-Calls)   ← stabiles Implementierungsprotokoll
            ┌─────────────────┼─────────────────┐
            ▼                 ▼                 ▼
        Rust-Core          Go-Core           C#-Core                  ← austauschbar
        (tokio)          (goroutines)       (NativeAOT)
```

## Was im Repository liegt

```
generator/    wabi-wit-gen (Rust, nutzt wit-parser): WIT → SDKs + Core-Glue
  runtime/      die generischen App-Runtimes (C#, Java inkl. JNI-Shim, Swift, Dart, TypeScript);
                eingebettet und bei jedem Lauf neben den generierten Code geschrieben
  npm/          der Generator als npm-Paket @nfails/wabi-wit-gen (WebAssembly)
runtime/      die generischen Core-Runtimes: rust (Crate wabi-abi-rt), go, csharp-core
abi/          die native ABI: wabi_abi.h, wabi_abi_functions.h und die normative ABI.md
example/      das eine Beispiel: HTTP-Client mit Verschlüsselung + Mock-Service (siehe unten)
docs/         Hintergrund: Flutter, API-Kompatibilität, Core-Sprachen, Component Model, Roadmap
scripts/      test-all.sh
```

## Eigene Projekte: der Generator per npm

Für eigene Verträge braucht es dieses Repository nicht. `wabi-wit-gen` gibt es als
npm-Paket (der Generator als WebAssembly, läuft mit Node ≥ 20 und Bun auf jeder
Plattform):

```bash
npx @nfails/wabi-wit-gen wit/ --out sdk --target ts,dart,java,csharp,swift --core rust
```

`--target` wählt die SDK-Sprachen, `--core` die Sprache des Cores (`rust`, `go`,
`csharp`, auch mehrere). In `sdk/` landen die ABI-Header, der Core-Glue samt
Core-Runtime und pro Sprache ein fertiges Paket aus generiertem Code, Runtime und
Paketdateien:

| Sprache | Was in `sdk/` entsteht |
|---|---|
| C# | Projekt, das `dotnet pack` zu einem NuGet-Paket mit den nativen Bibliotheken macht |
| Java | Quellen, JNI-Shim und `build.sh` für Jar und Android-Jar |
| Swift | SwiftPM-Paket (`swift/<Modul>`) |
| Dart/Flutter | Paket mit Build-Hook (`flutter pub add <paket> --path sdk/dart`) |
| TypeScript | Paket für Bun (`package.json`, `index.ts`) |

Rust- und Go-Cores baut `sdk/core-<lang>/build-native.sh` für alle Plattformen
nach `sdk/native/`, von wo Dart- und NuGet-Paket sie ohne weitere Konfiguration
übernehmen. Details: [generator/npm/README.md](generator/npm/README.md).

## Das Beispiel

[`example/`](example/README.md) ist ein kleiner **HTTP-Client**:

* Jeder Request- und Response-Body ist mit **AES-256-GCM** verschlüsselt; der
  Schlüssel wird aus einem gemeinsamen Geheimnis abgeleitet.
* Jeder Request ist mit einem **EC-Schlüssel signiert, der die App nie verlässt**
  (Secure Enclave, Android Keystore, TPM). Der Core fragt die App über einen
  Host-Call nach jeder Signatur.
* Ereignisse des Backends kommen als Stream (Server-Sent Events).
* Mehrere Clients können sich einen `transport` (Verbindungspool) teilen: ein
  zweites Objekt, das als Parameter und als Rückgabewert über die ABI geht.
* Dazu gehört der passende **Mock-Service**: als `demo-mock-server` über echtes
  HTTP und im Rust-Core auch in-process (`mem://`), damit Tests und Apps ohne
  Netzwerk laufen.

Der Vertrag steht in [`example/wit/client.wit`](example/wit/client.wit). Drei
Cores implementieren ihn unabhängig voneinander (Rust mit tokio/reqwest, Go mit
net/http, C# mit HttpClient als NativeAOT-Bibliothek), fünf SDKs werden daraus
generiert. So sieht die API in den Sprachen aus:

```csharp
await using var client = new DemoClient(options);
ClientHost.Register(client, appKeys);               // die App hält den Schlüssel (oder prozessweit: Register(appKeys))
await client.ConnectAsync();
Customer customer = await client.GetCustomerAsync("4711");
await foreach (var evt in client.Events(cancellationToken)) { … }
// Fehler: catch (ClientException e) when (e.Error is ClientError.NotFound)
```

```java
var client = new DemoClient(options);
CompletableFuture<Customer> customer = client.connect().thenCompose(v -> client.getCustomer("4711"));
Flow.Publisher<ClientEvent> events = client.events();
// Fehler: CompletionException(ClientException(error = ClientError.NotFound))
```

```swift
let client = DemoClient(options: options)
try await client.connect()
let customer = try await client.getCustomer(id: "4711")
for try await event in client.events() { … }
// Fehler: catch ClientError.notFound
```

```dart
final client = DemoClient(options);
await client.connect();
final customer = await client.getCustomer('4711', cancellationToken: token);
await for (final event in client.events()) { … }            // Stream<ClientEvent>
// Fehler: on ClientErrorNotFound { … }
```

```ts
using client = new DemoClient(options);
await client.connect();
const customer = await client.getCustomer("4711", abortSignal);
for await (const event of client.events()) { … }
// Fehler: e instanceof ClientException && e.error.tag === "not-found"
```

### Was die Tests nachweisen

`example/tests/compatibility/run.sh` führt **dieselben, unveränderten**
Test-Binaries bzw. Testsuiten gegen jeden Core aus:

| Suite | Rust-Core, Mock in-process | Rust-Core über HTTP | Go-Core über HTTP | C#-Core über HTTP |
|---|---|---|---|---|
| C: ABI-Konformität (`tests/abi/conformance.c`, ~3000 Checks) | ✅ | ✅ | ✅ | ✅ |
| C: Host-Call-Protokoll (`tests/abi/host_calls.c`, OpenSSL als Keystore) | ✅ | ✅ | ✅ | ✅ |
| C# SDK (20 Tests) | ✅ | ✅ | ✅ | ✅ |
| Java SDK, Panama (20 Tests) | ✅ | ✅ | ✅ | ✅ |
| Java SDK, JNI / Android-Pfad (20 Tests) | ✅ | ✅ | ✅ | ✅ |
| Dart SDK | ✅ | ✅ | ✅ | ✅ |
| TypeScript SDK in Bun (24 Tests) | ✅ | ✅ | ✅ | ✅ |

Abgedeckt: Sync Call, Async Call, Async Error, Cancellation, Cancel/Completion-
Race, Stream, Stream Cancellation, Object Lifetime, Double Drop, Concurrent
Calls (500 parallel), Callback während Dispose, Core-Version-Mismatch (drei
absichtlich inkompatible Fake-Cores), 4 MiB Payloads, Unicode, Ressourcen als
Parameter und Rückgabewert (geliehen und übergeben), Host-Calls (64 parallel mit
Antworten in anderer Reihenfolge, Fehler der App, gefälschte Signatur, eigene
Implementierung pro Instanz, Abbruch bis in die App). Swift (inkl. Host-Calls über das Security-Framework) und Flutter
(Android-Emulator, iOS-Simulator, macOS, Windows, Linux) laufen in der CI
(`.github/workflows/ci.yml`).

## Schnellstart

```bash
scripts/test-all.sh                          # Generator, Runtimes, Cores, Matrix
example/tests/compatibility/run.sh           # nur die Kompatibilitätsmatrix
CORES="rust go" example/tests/compatibility/run.sh

example/generate.sh                          # nach Änderungen an example/wit/client.wit
example/generate.sh --check                  # CI: Lock-Datei/generierter Code aktuell?
```

Benötigt: Rust ≥ 1.80, Go ≥ 1.22, .NET 8 (für den C#-Core zusätzlich clang und
zlib-Header), JDK ≥ 22 (Panama FFM ist ab 22 final),
Dart ≥ 3.10, Bun ≥ 1.4, OpenSSL-Header (für den C-Test der Host-Calls), Swift ≥ 5.9
(macOS). Fehlt eine Toolchain, überspringt die Matrix die betroffenen Zeilen.

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
  entfernte IDs werden nie neu vergeben. Eine ID steht für genau eine Signatur:
  Ändern sich Parameter- oder Ergebnistypen, bekommt die Methode eine neue ID
  und die alte wird stillgelegt. Zusätzlich prüft der Generator jede
  Vertragsänderung gegen die veröffentlichte API
  ([docs/API_COMPATIBILITY.md](docs/API_COMPATIBILITY.md)).
* **Drei Fehlerebenen** (§9): ABI-Status ≠ Domain-Fehler (Payload `[1, E]`)
  ≠ Abbruch.
* **Keine Thread-Annahmen** (§6): Callbacks kommen von Core-Threads; die SDKs
  dispatchen selbst und setzen nie einen UI-Thread voraus.
* **Symbol-Präfix und Runtime pro Paket** (§1.1): Zwei mit `wabi-wit-gen`
  erzeugte SDKs laufen in einer App, ohne dass Klassen, C-Module, JNI- oder
  ABI-Symbole kollidieren (`example/tests/multi-package`).

## Wer ist wofür zuständig?

**Generiert** aus WIT + `abi-ids.lock`:

* **C#:** `sealed record`s, Enums, Variants als geschlossene Record-Hierarchien,
  `<E>Exception`, Codec, Client-Klasse (`Task`, `IAsyncEnumerable`,
  `CancellationToken`), P/Invoke-Deklarationen.
* **Java:** Records, Enums, `sealed interface`s, Codec, Client-Klasse
  (`CompletableFuture`, `Flow.Publisher`), Panama-FFM-Bindings; darunter
  wahlweise das JNI-Binding für Android.
* **Swift:** `struct`s, `enum`s mit Associated Values, Codec, Client-Klasse
  (`async throws`, `AsyncSequence`).
* **Dart:** unveränderliche Klassen, `sealed class`-Hierarchien, Codec,
  Client-Klasse (`Future`, `Stream`, `CancellationToken`), `dart:ffi`-Bindings,
  Build-Hook für Dart native assets.
* **TypeScript (Bun):** Interfaces, diskriminierte Unions, Codec, Client-Klasse
  (`Promise`, `AsyncIterableIterator`, `AbortSignal`), `bun:ffi`-Bindings.
* **Core-Glue (Rust, Go, C#):** Typen, Codec, ein Trait bzw. Interface pro
  Ressource, der `method_id`-Dispatcher und typisierte Clients für importierte
  Interfaces (Host-Calls).
* Für **importierte** Interfaces in jeder SDK-Sprache: das Interface, das die App
  implementiert, und die Registrierung (`<Paket>Host`), prozessweit oder für
  eine einzelne Instanz.

**Handgeschrieben:** der Vertrag, die ABI-Header und `ABI.md`, pro Sprache genau
eine generische Runtime (MessagePack, Handles, Buffer-Ownership, Futures,
Streams, Cancellation, Versions- und Capability-Prüfung; nichts darin kennt
`Customer` oder `connect`), der Generator und die Geschäftslogik des Cores.

**Unverändert beim Wechsel der Core-Sprache** bleibt alles oberhalb der ABI:
Vertrag, Lock-Datei, generierter SDK-Code, SDK-Runtimes, Pakete, Apps und
Testsuiten, bis auf die kompilierten Binaries.

## Bekannte Grenzen

* **WIT-Teilmenge:** `future<T>` als Wert, statische und freie Funktionen,
  `flags` und in Java auch Tupel meldet der Generator als „nicht unterstützt“.
  `async func` und `stream<T>` sind voll unterstützt. Ressourcen gehen als
  ganzer Parameter (`borrow<R>`, `R`) und als Ergebnis über die ABI, nicht in
  Listen, Optionen, Records oder Streams.
* **Host-Calls** kennen keine Streams und keine Ressourcen als Parameter.
* **Java als Core-Sprache** (GraalVM native-image) ist nicht enthalten; der
  unfertige Stand liegt auf dem Branch `java-core`.
* **Android + Java:** Das JNI-Binding ist auf dem Desktop-JVM gegen beide Cores
  getestet, auf einem Gerät läuft es bisher nur über Flutter/Dart.
* **`https://`:** Der Rust-Core des Beispiels baut reqwest ohne TLS-Feature.
* **Unbekannte Variant-Cases** eines neueren Cores führen in älteren SDKs zu
  einem Protokollfehler.

Geplantes steht in [docs/ROADMAP.md](docs/ROADMAP.md).

## Lizenz

[Apache-2.0](LICENSE), siehe auch [NOTICE](NOTICE).

**Generierter Code:** Was `wabi-wit-gen` erzeugt, einschließlich der Runtimes,
die der Generator neben den generierten Code kopiert, gehört dem, der den
Generator ausführt. Es darf ohne Lizenzhinweis unter jeder Lizenz verwendet
werden, auch in proprietären Apps und SDKs.

## Initialisierung und erneute Generierung

Beim ersten Lauf können bearbeitbare Beispiel-Apps und Core-Gerüste angelegt werden:

```bash
npx @nfails/wabi-wit-gen wit/ --out sdk --target ts,dart,csharp --core csharp --scaffold examples
```

Dabei entsteht im aktuellen Projektverzeichnis eine `wabi-gen.conf` mit relativen
Pfaden. Danach reichen im selben Verzeichnis:

```bash
npx @nfails/wabi-wit-gen          # SDKs nach WIT-Änderungen aktualisieren
npx @nfails/wabi-wit-gen --check  # CI: nur prüfen, keinerlei Dateien ändern
```

Beispielkonfiguration (von Hand bearbeitbar):

```ini
wit = wit
out = sdk
targets = ts,dart,csharp
cores = csharp
lock = wit/abi-ids.lock
```

Mit `--config pfad/wabi-gen.conf` lässt sich die Konfiguration aus einem anderen
Verzeichnis verwenden. Pfade darin beziehen sich auf den Ort der Konfiguration;
explizite CLI-Optionen überschreiben gespeicherte Werte. Eine explizite WIT-Datei
lädt die Standardkonfiguration nur zusammen mit `--config`.

SDK-Code, Runtime-Kopien, ABI-Dateien, Core-Glue, Paketdateien, Build-Helfer und
`sdk/README.md` werden aktualisiert. Der Wire-ID-Lock wird kompatibel fortgeschrieben.
Bearbeitbare Dateien unter `sdk/examples/` und Core-Gerüste wie `Core.cs`,
`src/lib.rs`, `starter.go` und deren Projektdateien werden nur bei `--scaffold examples`
und nur dann angelegt, wenn die einzelne Datei fehlt. Vorhandene Inhalte und die
Konfiguration werden auch bei erneutem Scaffolding niemals überschrieben.
`--check` ignoriert diese bearbeitbaren Dateien. Nicht mehr benötigte generierte
Dateien werden derzeit nicht automatisch gelöscht.

Die Beispiel-Apps laden den Core und prüfen dessen Metadaten. Fachliche Aufrufe
müssen mit gültigen Parametern ergänzt werden. Core-Gerüste enthalten ausdrücklich
unimplementierte Methoden; neue WIT-Methoden erfordern eine Ergänzung im eigenen
Core. Die paketbezogene `sdk/README.md` beschreibt Build und Bibliotheksablage für
die tatsächlich gewählten Cores. Ohne gewählten Core ist eine passende native
Bibliothek bereitzustellen.

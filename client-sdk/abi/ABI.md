# Wabi Native ABI v1.1 — Spezifikation

> 1.1 ergänzt optionale Host-Calls (§4a). Alles aus 1.0 gilt unverändert.

Normative Beschreibung von [`wabi_abi.h`](wabi_abi.h) (Typen, Konstanten) und
[`wabi_abi_functions.h`](wabi_abi_functions.h) (die 17 Funktionen). Die ABI ist das
**Implementierungsprotokoll** zwischen Sprach-SDKs (C#, Java, Swift, …) und
einem nativen Core (Rust, Go, Zig, …). Sie ist **nicht** die öffentliche API —
die steht ausschließlich in [`wit/client.wit`](../wit/client.wit).

Schlüsselwörter MUSS / DARF / SOLL im Sinne von RFC 2119.
Konformität prüft [`tests/abi/conformance.c`](../tests/abi/conformance.c)
(sprachneutral, lädt jeden Core per `dlopen`).

---

## 1. Grundprinzipien

1. **Klein und generisch.** 17 Funktionen (pro WIT-Paket unter eigenem Symbol-Präfix, §1.1). Eine neue WIT-Funktion ändert die
   ABI nicht: sie bekommt eine Method-ID und wird über `wabi_call_sync` /
   `wabi_call_async` aufgerufen.
2. **Nur feste C-Typen.** `uint32_t`, `uint64_t`, `int32_t`, `size_t`,
   Zeiger, zwei `struct {ptr,len}`, ein Funktionszeiger. Keine
   Rust-/Go-/Zig-Typen, keine Exceptions, kein `errno`, kein Thread-Local-State.
3. **Payloads statt Strukturen.** Argumente und Ergebnisse sind Bytes in
   *Payload-Encoding v1* (§5, MessagePack-Teilmenge).
4. **Eine Async-Abstraktion.** Jede async-Funktion und jedes `stream.next()`
   liefert dasselbe Future-Handle (§4).
5. **Nie undefiniertes Verhalten durch Handle-Fehler.** Ungültige, fremde oder
   doppelt freigegebene Handles liefern `WABI_ERR_INVALID_HANDLE` (§3).

### 1.1 Symbol-Präfix pro WIT-Paket

Jedes WIT-Paket exportiert die 17 Funktionen unter einem **eigenen Präfix**,
abgeleitet aus dem Paketnamen: `<namespace>_<name>` in snake_case.

| WIT-Paket | Präfix | Symbole | Bibliothek |
|---|---|---|---|
| `wabi:client` | `wabi_client` | `wabi_client_call_sync`, `wabi_client_future_take`, … | `libwabi_client.so` / `wabi_client.dll` |
| `geo:lib` | `geo_lib` | `geo_lib_call_sync`, … | `libgeo_lib.so` |

* **Warum:** Mehrere Cores bzw. Pakete können im selben Prozess leben, auch
  **statisch in eine Binary gelinkt** (iOS-xcframework, Rust-Core mit
  eingebetteter Go/Zig-Bibliothek). Mit gemeinsamen Namen würde das Linken
  fehlschlagen (`multiple definition of …`) oder, bei dynamischen
  Bibliotheken, ein Core still den anderen überdecken.
  `tests/abi/multi_package.c` linkt zwei Cores unter `wabi_client` und
  `geo_lib` statisch in ein Programm und ruft beide auf.
* **Unverändert gemeinsam** sind die Typen (`wabi_bytes`, `wabi_buffer`,
  `wabi_status`, `wabi_future_callback`, …), Statuscodes und Capabilities in
  `wabi_abi.h`. Sie sind für alle Pakete identisch.
* **Header:** `wabi_abi_functions.h` deklariert die Funktionen mit
  `WABI_FN(name)` für das jeweils definierte `WABI_PREFIX` und darf pro Paket
  einmal eingebunden werden. Der Generator erzeugt zusätzlich einen
  ausgeschriebenen Header je Paket (`generated/c/<präfix>.h`).
* **SDKs** kennen den Präfix aus dem generierten Code (`Contract.SymbolPrefix`,
  `AbiConstants.SYMBOL_PREFIX`, …). Der JNI-Shim bekommt ihn zur Laufzeit und
  ist deshalb für alle Pakete derselbe.
* Zwei *Major-Versionen* desselben Pakets im selben Prozess sind nicht
  vorgesehen (gleicher Präfix).

## 2. Versionierung und Kompatibilität

| Funktion | Bedeutung |
|---|---|
| `wabi_abi_version()` | `(major << 16) \| minor` der ABI |
| `wabi_abi_capabilities()` | Bitmaske `WABI_CAP_*` |
| `wabi_contract_id()` | WIT-Paket inkl. Version, z. B. `wabi:client@1.0.0` |
| `wabi_core_info()` | Freitext zur Diagnose, z. B. `wabi-go-core/0.1.0` |

Ein SDK MUSS beim Laden prüfen und bei Verletzung mit einem eigenen Fehlertyp
abbrechen (`WabiIncompatibleLibraryException` / `WabiRuntimeError.incompatibleLibrary`),
bevor es irgendeine andere Funktion aufruft:

* **ABI:** gleiche Major-Version, Core-Minor ≥ SDK-Minor.
* **Contract:** gleiches WIT-Paket (`wabi:client`), gleiche Major-Version,
  Core-Minor ≥ SDK-Minor. Ein neuerer Core darf Funktionen *hinzufügen*
  (Method-IDs ändern sich nie, §10), ein älterer SDK nutzt sie nur nicht.
* **Capabilities:** Pflicht sind `SYNC_CALLS`, `ASYNC_CALLS`, `STREAMS`,
  `PAYLOAD_MSGPACK_1`. `CANCELLATION` und `THREADED_CALLBACK` sind
  informativ: `wabi_future_cancel` ist immer aufrufbar und sicher; ein
  „eager“ Core (alle Futures sind bei Rückgabe schon fertig, z. B. der
  Zig-Stub) hat nichts abzubrechen. Unbekannte Bits MÜSSEN ignoriert werden.

Minor-Erhöhung = nur Ergänzungen (neue Capability-Bits, neue Statuscodes, die
alte Hosts als generischen Fehler behandeln). Alles andere = Major.

## 3. Handles und Ownership

```c
typedef uint64_t wabi_object_handle;  // Ressourcen (Client, Session, …)
typedef uint64_t wabi_future_handle;  // Ergebnis einer async-Operation
typedef uint64_t wabi_stream_handle;  // Pull-Stream
```

| Regel | |
|---|---|
| **Wert 0** | ist nie gültig (`WABI_NULL_HANDLE`). Konstruktoren und statische Funktionen werden mit `object = 0` aufgerufen. |
| **Opak** | Hosts DÜRFEN Handles nicht interpretieren. (Rust/Go/Zig kodieren die Art im obersten Byte — nur zur Lesbarkeit in Logs.) |
| **Nie wiederverwendet** | Ein Core MUSS Handle-Werte innerhalb eines Prozesses eindeutig halten (z. B. monotoner 64-Bit-Zähler). Dadurch wird jeder Zugriff auf ein freigegebenes Handle *erkannt* statt ein neues Objekt zu treffen (ABA-frei). |
| **Besitz** | Jedes Handle, das der Host erhält (`out_future`, Handle im Response-Payload eines Konstruktors oder einer `stream<T>`-Funktion), gehört dem Host. Er MUSS es genau einmal mit der passenden `*_drop`-Funktion freigeben. |
| **Ungültig wird** | ein Handle ausschließlich durch seinen eigenen `*_drop`-Aufruf. `wabi_object_drop` invalidiert *nicht* die Futures/Streams des Objekts: laufende Futures werden abgeschlossen (Domain-Fehler wie `disconnected` oder `WABI_ERR_CANCELLED`), Streams enden (`WABI_STREAM_END` bzw. `WABI_ERR_CANCELLED`). Ihre Handles bleiben gültig, bis der Host sie freigibt. |
| **Double-Free** | Zweites `*_drop` → `WABI_ERR_INVALID_HANDLE`. Kein UB, kein Absturz. |
| **Falsche Art** | Future-Handle an `wabi_stream_next` usw. → `WABI_ERR_INVALID_HANDLE`. |
| **Thread-Safety** | Alle Funktionen sind von beliebigen Threads gleichzeitig aufrufbar, auch für dasselbe Handle. `drop` parallel zu einem anderen Aufruf auf demselben Handle ist erlaubt: der andere Aufruf sieht entweder das lebende Objekt oder `WABI_ERR_INVALID_HANDLE`. |

SDKs kapseln Objekt-Handles in einem Typ, der genau einmal freigibt
(C#: `ObjectHandle` mit `Interlocked.Exchange` + Finalizer, Java: `Cleaner`,
Swift: `deinit`). Futures und Streams gibt das SDK in `finally` frei.

## 4. Futures

```c
wabi_status wabi_call_async(object, method_id, request, &future);
wabi_status wabi_future_subscribe(future, callback, user_data);
wabi_status wabi_future_poll(future);                  // PENDING | READY
wabi_status wabi_future_take(future, &out_result);     // Ergebnis herausbewegen
wabi_status wabi_future_cancel(future);
wabi_status wabi_future_drop(future);
```

### 4.1 Zustandsautomat

```
            complete (Core)              take (Host)
 PENDING ───────────────────▶ READY(outcome) ──────────▶ TAKEN
    │                              ▲
    └── cancel / drop (Host) ──────┘  outcome = CANCELLED
```

**Der erste Übergang aus `PENDING` gewinnt, alle späteren sind No-ops.** Das
löst jede Race-Condition „Operation endet genau gleichzeitig mit `cancel()`“:

* `cancel` vor Abschluss → Outcome `WABI_ERR_CANCELLED`, spätere Fertigmeldung
  des Cores wird verworfen.
* Abschluss vor `cancel` → `cancel` ist ein No-op, `take` liefert das Ergebnis
  („completion wins“). SDKs MÜSSEN daher damit rechnen, dass ein
  abgebrochener Aufruf trotzdem ein Ergebnis liefert — die C#-/Java-/Swift-Layer
  melden in dem Fall das, was `take` sagt.
* Der Callback feuert in beiden Fällen **genau einmal**.

Die Conformance-Suite erzwingt beide Reihenfolgen (200 Iterationen mit
gestreutem Timing; Rust/Go liefern gemischte Ausgänge, alle korrekt).

### 4.2 Callback-Vertrag

```c
typedef void (*wabi_future_callback)(void *user_data, wabi_future_handle future);
```

* `wabi_future_subscribe` liefert
  * `WABI_OK` → Callback wird **genau einmal** aufgerufen, *auch* wenn das
    Future abgebrochen oder freigegeben wird. Damit ist der Callback der eine,
    sichere Ort, an dem der Host `user_data` freigibt (C#: `GCHandle.Free`,
    Swift: `Unmanaged.takeRetainedValue`, Java: Eintrag aus der Map entfernen).
  * `WABI_READY` → schon fertig, Callback wird **nie** aufgerufen.
  * `WABI_ERR_BUSY` (zweites Subscribe) / `WABI_ERR_INVALID_HANDLE` → nie aufgerufen.
* Der Callback trägt **kein Ergebnis**; der Host ruft danach `wabi_future_take`.
* Er wird **nie innerhalb eines ABI-Aufrufs des Hosts** ausgeführt (kein
  Reentrancy-Risiko für Host-Frames). Muss ein Core wegen `cancel`/`drop`
  benachrichtigen, geschieht das asynchron auf einem Core-Thread.
* Siehe §6 für Threads.

### 4.3 Bewertung: Ergebnis im Callback vs. separates `take`

| | Ergebnis im Callback (`cb(ud, status, ptr, len)`) | **Nur Signal + `take` (gewählt)** |
|---|---|---|
| FFI-Aufrufe pro Operation | 1 weniger | +1 (`take`) — bei HTTP/WebSocket-Latenzen irrelevant |
| Lebensdauer des Puffers | nur während des Callbacks gültig → Host muss *im fremden Thread* sofort kopieren | Host kopiert, wann und wo er will; Puffer gehört ihm bis `wabi_buffer_free` |
| Arbeit im Callback | Deserialisierung/Kopie im Core-Thread (blockiert den Core) | nur „Task fertig“-Signal, O(1) |
| Polling-Hosts (WASM, Game-Loops, Tests) | brauchen trotzdem eine zweite API | `poll` + `take` ohne Callback funktionieren direkt |
| Eager Cores | müssten Callback synchron aus `subscribe` rufen (Reentrancy) | `subscribe` → `WABI_READY`, fertig |
| Go/cgo | Übergabe von Zeigern an C-Callbacks mit Pinning-Regeln | nur `uint64_t` + `void*` |

Der eine zusätzliche Aufruf kostet Nanosekunden; die gewonnene Einfachheit bei
Ownership und Threading ist für den Use Case (HTTP, WebSocket, Krypto) klar
wertvoller.

### 4.4 `take`

`take` bewegt das Outcome genau einmal heraus:
`WABI_OK` + Payload, `WABI_STREAM_END` (nur bei `next`-Futures),
`WABI_ERR_CANCELLED`, `WABI_PENDING` (noch nicht fertig, nichts entnommen),
`WABI_ERR_ALREADY_TAKEN` oder ein anderer Fehler (Payload = UTF-8-Diagnose).

### 4.5 Mapping in den SDKs

| Sprache | Typ | Abbruch |
|---|---|---|
| C# | `Task<T>` / `Task` | `CancellationToken` → `UnsafeRegister(wabi_future_cancel)`; ein bereits abgebrochener Token startet die Operation gar nicht |
| Java | `CompletableFuture<T>` | Unterklasse `NativeFuture`: `cancel()` ruft zusätzlich `wabi_future_cancel` |
| Swift | `async throws -> T` | `withTaskCancellationHandler { … } onCancel: { wabi_future_cancel }`; `Task.checkCancellation()` vor dem Start |
| Python | `Awaitable[T]` | `asyncio.Future` + `loop.call_soon_threadsafe` im Callback |
| TypeScript (Bun) | `Promise<T>` | `AbortSignal` → `wabi_future_cancel`; Callback als `bun:ffi` `JSCallback` mit `threadsafe: true` (läuft später auf dem Event-Loop); `wabi_bytes`/`wabi_buffer` per Wert: zwei u64-Argumente (SysV, AAPCS64) bzw. Zeiger auf eine Kopie (Win64) |
| JS (Node) | `Promise<T>` | `AbortSignal` → `wabi_future_cancel`; Callback via `napi_threadsafe_function` |

## 4a. Host-Calls (ABI 1.1): der Core ruft die App

Importiert eine WIT-Welt ein Interface (`import host-keys;`), implementiert
**die App** dessen Funktionen und der Core ruft sie auf, z. B. „signiere mit dem
Schlüssel im Secure Enclave / Android Keystore“. Dafür gibt es zwei **optionale**
Funktionen und das Capability-Bit `WABI_CAP_HOST_CALLS`; Pakete ohne Imports
brauchen weiterhin nur ABI 1.0.

```c
typedef void (*wabi_host_handler)(void *user_data, uint32_t method_id,
                                  const uint8_t *request, size_t request_len, uint64_t call);
wabi_status <prefix>_host_register(wabi_host_handler handler, void *user_data); /* NULL: abmelden */
wabi_status <prefix>_host_reply(uint64_t call, wabi_status status, wabi_bytes response);
```

1. **Method-IDs** der Import-Funktionen stehen wie alle anderen in `abi-ids.lock`
   (eigene Gruppe pro Interface). `request` sind die Parameter als Array (§5),
   die Antwort ist der kodierte Rückgabewert (bei `result<T,E>`: `[0, T]` / `[1, E]`).
2. Der Core MUSS den Handler auf einem **eigenen Thread** aufrufen, nie innerhalb
   eines ABI-Aufrufs des Hosts (also nur aus async-Arbeit). Mehrere Host-Calls
   DÜRFEN gleichzeitig offen sein.
3. Der Handler MUSS schnell zurückkehren. Die App antwortet **später, von einem
   beliebigen Thread, genau einmal** mit `host_reply`. `request` bleibt bis zu
   dieser Antwort gültig; der Core kopiert `response`.
4. `status != WABI_OK` meldet einen Transportfehler (keine Implementierung, App-Bug;
   `response` = UTF-8-Diagnose). Domänenfehler laufen als `[1, E]` mit `WABI_OK`.
5. Unbekannte oder bereits beantwortete `call`-IDs → `WABI_ERR_INVALID_HANDLE`.
6. Bricht der Host die umgebende Operation ab, gibt der Core den Host-Call auf,
   akzeptiert (und verwirft) die späte Antwort aber trotzdem — erst dann gibt er
   `request` frei.
7. Ohne registrierten Handler MUSS die Operation mit einem Fehler enden, nie hängen.

| Sprache | App implementiert | Registrierung | Handler-Mechanik |
|---|---|---|---|
| C# | `IHostKeys` (`Task<T>`, Domänenfehler als `ZeroException`) | `ZeroHost.Register(app)` | `[UnmanagedCallersOnly]` → Thread-Pool |
| Java | `HostKeys` (`CompletionStage<T>`) | `ZeroHost.register(app)` | Panama-Upcall bzw. JNI-Shim → `ForkJoinPool` |
| Dart/Flutter | `HostKeys` (`Future<T>`, wirft `ZeroError…`) | `ZeroHost.register(app)` | `NativeCallable.listener` → Isolate-Event-Loop |
| Swift | `HostKeys` (`async throws`) | `ZeroHost.register(hostKeys:)` | `@convention(c)`-Closure → `Task.detached` |
| Go (Core) | ruft `HostKeysSign(ctx, …)` (generiert) | – | `runtime/go` (`HostCall`) |
| Rust (Core) | ruft `host_keys::sign(…).await` (generiert) | – | `runtime/rust` (`host_calls::call`, tokio) |
| C# (Core, NativeAOT) | ruft `HostKeys.SignAsync(…)` | – | `runtime/csharp-core` (`HostCalls.CallAsync`) |

Protokolltest (sprachneutral): [`tests/abi/host_calls.c`](../tests/abi/host_calls.c);
Beispiel-Paket: [`packages/zero`](../packages/zero).

## 5. Payload-Encoding v1

Eine strikte **MessagePack-Teilmenge**. Jede Seite (SDK-Runtime, Core) bringt
einen eigenen ~200-Zeilen-Codec mit — keine Abhängigkeit von einer
MessagePack-Bibliothek und deren Eigenheiten.

| WIT | Wire |
|---|---|
| `bool` | `true`/`false` |
| `u8…u64`, `s8…s64` | kleinste Integer-Kodierung; Leser prüfen den Wertebereich |
| `f32`, `f64` | `float32` / `float64` |
| `char` | Unicode-Scalar als uint |
| `string` | `str`, **immer UTF-8**; ungültiges UTF-8 und Lone-Surrogates werden abgewiesen, nie ersetzt |
| `list<u8>` | `bin` |
| `list<T>` | `array` |
| `option<T>` | `nil` oder `T` (`option<option<T>>` ist unzulässig) |
| `tuple<…>` | `array` fester Länge |
| `record` | `map { field-tag: value }` — Tags aus `abi-ids.lock`; `none`-Felder werden weggelassen; **unbekannte Tags werden übersprungen** |
| `variant` | `array [case-tag]` oder `[case-tag, payload]`; zusätzliche Elemente werden übersprungen |
| `enum` | case-tag als uint |
| `result<T, E>` | `array [0, T]` oder `[1, E]`; fehlender Typ → `nil` |
| Rückgabe ohne Wert | `nil` |
| `stream<T>`, Ressource | Handle als uint64 (Besitz geht an den Empfänger) |

**Requests** sind ein `array` mit genau einem Element pro Parameter
(Signaturen sind eingefroren; neue Parameter = neue Funktion).
**Responses** sind der kodierte Rückgabewert. Leser begrenzen
Verschachtelungstiefe (64) und prüfen Längen gegen die Restlänge, bevor sie
allozieren.

**Evolution:** Records mit Feld-Tags statt Positionen erlauben neue optionale
Felder in beide Richtungen (alte Leser überspringen sie, neue Leser sehen
`none`). Neue Variant-/Enum-Cases erzeugen bei alten Lesern einen sauberen
Protokollfehler (keine Fehlinterpretation).

### 5.1 Bewertung: typisierte FFI-Strukturen (A) vs. Buffer-Payloads (B)

| Kriterium | A: typisierte Structs | **B: `method_id` + Bytes (gewählt)** |
|---|---|---|
| Overhead pro Aufruf | geringer (keine Serialisierung) | ~µs für Kodierung — gegenüber Netzwerk-Latenz (ms) vernachlässigbar |
| ABI-Größe | wächst mit jedem Typ/jeder Methode | konstant 17 Funktionen |
| Versionierung | Struct-Layouts einfrieren; neue Felder = neue Structs | Feld-Tags, optionale Felder, überspringbar |
| Core-Austausch | jeder Core muss exakte Layouts, Alignment, Ownership pro Typ nachbauen | ein Codec + ein Dispatcher |
| Generator | pro Sprache Marshalling-Code für Structs, Listen, Optionals, Strings | nur „Wert → Bytes“ |
| Speicher-Ownership | pro Feld zu regeln (wer gibt den `char*` in `customer.email` frei?) | genau zwei Puffertypen, eine Regel (§7) |
| Debugbarkeit | Speicherdumps | Payloads mit jedem MessagePack-Tool lesbar |
| Component Model | eigenes Layout ≠ Canonical ABI | Werte-Modell 1:1 aus WIT, leicht auf Canonical ABI abbildbar |

Für HTTP/WebSocket/Krypto mit wenigen tausend Aufrufen pro Sekunde ist B
eindeutig besser. Sollte ein Hot-Path entstehen (z. B. Bulk-Krypto), kann eine
spätere Minor-Version gezielt eine typisierte Zusatzfunktion ergänzen, ohne
das Modell aufzugeben. JSON wurde verworfen (keine Binärdaten, mehrdeutige
Zahlen, größer, langsamer).

## 6. Threading

* **Aufrufe:** Jede ABI-Funktion ist von jedem Thread aus aufrufbar, parallel.
* **Callback-Threads:** Callbacks kommen von **Core-eigenen Threads** (Tokio-
  Worker, Goroutinen) — nie von einem Host-Thread, nie synchron aus einem
  Host-Aufruf. Verschiedene Futures können ihre Callbacks **parallel** melden.
* **Callbacks MÜSSEN schnell zurückkehren:** kein Blockieren, kein Warten auf
  andere Futures, keine Deserialisierung. Die SDKs signalisieren nur:
  C# `TaskCompletionSource` mit `RunContinuationsAsynchronously`, Java
  `Executor.execute`, Swift `CheckedContinuation.resume`.
* **Reentrancy:** Aus einem Callback DARF jede ABI-Funktion aufgerufen werden
  (z. B. `take`). Cores dürfen während eines Callbacks keine Locks halten.
  Hosts müssen *nicht* reentrant sein, weil nie ein Callback in einen
  laufenden Host-Aufruf hinein erfolgt.
* **UI-/Main-Thread:** Die ABI setzt **nie** einen .NET-`SynchronizationContext`,
  den Android-Main-Looper oder den iOS-Main-Thread voraus. Die SDK-Runtime
  landet auf dem Threadpool/Executor; zurück in den UI-Kontext kommt die App
  mit den üblichen Mitteln (`await` ohne `ConfigureAwait(false)` in C#,
  `thenAcceptAsync(fn, uiExecutor)` in Java, `@MainActor` in Swift).
* **Entladen:** Ein Host DARF die Bibliothek nicht entladen, solange Handles
  leben oder Callbacks ausstehen.

## 7. Speicher-Ownership

| Daten | Alloziert von | Freigegeben von | Lebensdauer / Kopie |
|---|---|---|---|
| `wabi_bytes` (Request) | Host | Host | nur während des Aufrufs gültig; der Core kopiert, was er behält |
| `wabi_buffer` (Response, `take`, Diagnosen) | **Core** | **Host** mit `wabi_buffer_free` *derselben* Bibliothek, exakt `{ptr,len}` wie erhalten | gehört dem Host bis zum Free; SDKs kopieren sofort in verwaltete Arrays und geben frei |
| `{NULL, 0}` | — | Free ist No-op | leerer Puffer |
| Statische Strings (`contract_id`, `core_info`, `status_name`) | Core | nie | prozessweit gültig, UTF-8, NUL-terminiert |
| `user_data` des Callbacks | Host | Host, **im Callback** (oder sofort, wenn `subscribe` ≠ `WABI_OK`) | Core fasst den Zeiger nie an, reicht ihn nur durch |
| Future-Ergebnis | Core | Core beim `take` (Übergabe als `wabi_buffer`) bzw. beim `drop` | bis `take` oder `drop` |

* Alle `out`-Parameter werden auf **jedem** Pfad beschrieben (auch im Fehlerfall),
  `NULL`-Out-Zeiger → `WABI_ERR_INVALID_ARGUMENT`.
* Ein Host DARF `wabi_buffer_free` nie mit einem eigenen Allocator mischen:
  Rust gibt `Box<[u8]>` frei, Go `C.free`, Zig `c_allocator.free` — genau
  deshalb besitzt jede Bibliothek ihr eigenes `free`.
* Strings sind überall UTF-8. C#/Java-Strings (UTF-16) werden strikt
  kodiert: unpaarige Surrogates → `ArgumentException`/`IllegalArgumentException`
  bevor irgendetwas die ABI überquert.

## 8. Streams

```c
wabi_status wabi_stream_next(stream, &future);   // Pull: Future auf das nächste Element
wabi_status wabi_stream_cancel(stream);          // Producer stoppen
wabi_status wabi_stream_drop(stream);            // cancel + freigeben
```

* Streams entstehen als Rückgabewert von WIT-Funktionen (`events: func() ->
  stream<client-event>`), das Handle steht im Response-Payload.
* **Pull-basiert = Backpressure:** Der Core liefert nur, wenn der Host fragt.
  Pro Stream höchstens ein offenes `next`-Future (`WABI_ERR_BUSY` sonst).
* `next`-Futures enden mit `WABI_OK` + Element, `WABI_STREAM_END` oder
  `WABI_ERR_CANCELLED`.
* **Abbruch eines einzelnen `next`** verwirft nur diesen Pull, **kein Element
  geht verloren** (Rust: `Stream::poll_next` ist cancel-safe; Go: ein bereits
  entnommenes Element wird zurückgelegt).
* `wabi_stream_cancel`: offenes `next` → `WABI_ERR_CANCELLED`, alle späteren →
  `WABI_STREAM_END`. Idempotent.
* SDK-Mapping: C# `IAsyncEnumerable<T>`, Java `Flow.Publisher<T>` (Demand →
  `next`), Swift `AsyncSequence` (`WabiStream<T>`), Python `AsyncIterator`,
  JS `AsyncIterable`.

## 9. Fehler — drei getrennte Ebenen

1. **ABI-Status** (`wabi_status`, <0): Programmier-/Integrationsfehler —
   ungültiges Handle, kaputter Payload, unbekannte Methode, Panic im Core
   (`WABI_ERR_INTERNAL`; Cores fangen Panics an der Grenze ab). SDKs werfen
   `WabiNativeException` / `WabiRuntimeError.native`.
2. **Domain-Fehler** aus WIT (`result<customer, client-error>`) sind **Daten**
   im Payload (`[1, E]`), nie ein Status. SDKs mappen sie auf
   `ClientException` (C#, Java; in Java innerhalb von
   `CompletionException`/`ExecutionException`) bzw. `throws ClientError` (Swift).
3. **Abbruch** ist `WABI_ERR_CANCELLED` → `OperationCanceledException`,
   `CancellationException`, `CancellationError`.

Keine sprachspezifischen Exceptions überqueren die ABI; `extern "C"`-Grenzen
fangen Panics (Rust `catch_unwind`, Go `recover`).

## 10. Stabile Method-IDs

`method_id = (group << 8) | index`. Beides steht in
[`wit/abi-ids.lock`](../wit/abi-ids.lock), das `wabi-wit-gen` pflegt:

* Beim ersten Auftreten bekommt ein Element die **nächste freie Nummer** in
  seinem Scope (Methoden: pro Ressource; Felder/Cases: pro Typ) und wird in
  die Lock-Datei geschrieben (committed).
* Danach ist **nur die Lock-Datei maßgeblich** — Umordnen in `client.wit` ändert
  nichts (Generator-Test `reordering_the_wit_never_changes_ids`).
* Entfernte Elemente bleiben als `retired` stehen; ihre Nummer wird **nie**
  neu vergeben (Test `new_functions_are_appended_and_removed_ids_are_never_reused`).
* `wabi-wit-gen --check` schlägt in CI fehl, wenn Lock-Datei oder generierter
  Code veraltet sind.

Verworfen: Deklarationsreihenfolge (bricht beim Umsortieren), Hash des Namens
(Kollisionen, nicht lesbar, Rename = neue ID ohne Kontrolle), Strings über die
ABI (langsamer, fehleranfällig).

## 11. Checkliste: einen neuen Core implementieren

1. Die 17 Funktionen aus `wabi_abi_functions.h` **unter dem Präfix des
   Pakets** exportieren (C-Linkage, `WABI_API`), z. B. `wabi_client_call_sync`
   (§1.1). Rust: generierte `#[export_name]`-Wrapper; Go: `//export wabi_client_…`
   mit nur `wabi_abi.h` im cgo-Preamble; Zig: `@export(…, .{ .name = prefix ++ "_…" })`;
   C#: `[UnmanagedCallersOnly(EntryPoint = Prefix + "…")]`.
2. Handle-Tabelle mit nie wiederverwendeten 64-Bit-Handles (§3).
3. Future-Zustandsautomat mit „erster Übergang gewinnt“ (§4.1), Callback
   genau einmal, nie synchron aus Host-Aufrufen.
4. Payload-Codec v1 (§5) und Dispatch `method_id → Implementierung`
   (für Rust generiert; siehe `core-rust/src/generated`).
5. Panics/Exceptions an der Grenze abfangen → `WABI_ERR_INTERNAL`.
6. `tests/abi/conformance.c` und `tests/compatibility/run.sh` grün.

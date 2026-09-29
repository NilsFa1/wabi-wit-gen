# Acme Native ABI v1.0 — Spezifikation

Normative Beschreibung von [`acme_client.h`](acme_client.h). Die ABI ist das
**Implementierungsprotokoll** zwischen Sprach-SDKs (C#, Java, Swift, …) und
einem nativen Core (Rust, Go, Zig, …). Sie ist **nicht** die öffentliche API —
die steht ausschließlich in [`wit/client.wit`](../wit/client.wit).

Schlüsselwörter MUSS / DARF / SOLL im Sinne von RFC 2119.
Konformität prüft [`tests/abi/conformance.c`](../tests/abi/conformance.c)
(sprachneutral, lädt jeden Core per `dlopen`).

---

## 1. Grundprinzipien

1. **Klein und generisch.** 17 Funktionen. Eine neue WIT-Funktion ändert die
   ABI nicht: sie bekommt eine Method-ID und wird über `acme_call_sync` /
   `acme_call_async` aufgerufen.
2. **Nur feste C-Typen.** `uint32_t`, `uint64_t`, `int32_t`, `size_t`,
   Zeiger, zwei `struct {ptr,len}`, ein Funktionszeiger. Keine
   Rust-/Go-/Zig-Typen, keine Exceptions, kein `errno`, kein Thread-Local-State.
3. **Payloads statt Strukturen.** Argumente und Ergebnisse sind Bytes in
   *Payload-Encoding v1* (§5, MessagePack-Teilmenge).
4. **Eine Async-Abstraktion.** Jede async-Funktion und jedes `stream.next()`
   liefert dasselbe Future-Handle (§4).
5. **Nie undefiniertes Verhalten durch Handle-Fehler.** Ungültige, fremde oder
   doppelt freigegebene Handles liefern `ACME_ERR_INVALID_HANDLE` (§3).

## 2. Versionierung und Kompatibilität

| Funktion | Bedeutung |
|---|---|
| `acme_abi_version()` | `(major << 16) \| minor` der ABI |
| `acme_abi_capabilities()` | Bitmaske `ACME_CAP_*` |
| `acme_contract_id()` | WIT-Paket inkl. Version, z. B. `acme:client@1.0.0` |
| `acme_core_info()` | Freitext zur Diagnose, z. B. `acme-go-core/0.1.0` |

Ein SDK MUSS beim Laden prüfen und bei Verletzung mit einem eigenen Fehlertyp
abbrechen (`AcmeIncompatibleLibraryException` / `AcmeRuntimeError.incompatibleLibrary`),
bevor es irgendeine andere Funktion aufruft:

* **ABI:** gleiche Major-Version, Core-Minor ≥ SDK-Minor.
* **Contract:** gleiches WIT-Paket (`acme:client`), gleiche Major-Version,
  Core-Minor ≥ SDK-Minor. Ein neuerer Core darf Funktionen *hinzufügen*
  (Method-IDs ändern sich nie, §10), ein älterer SDK nutzt sie nur nicht.
* **Capabilities:** Pflicht sind `SYNC_CALLS`, `ASYNC_CALLS`, `STREAMS`,
  `PAYLOAD_MSGPACK_1`. `CANCELLATION` und `THREADED_CALLBACK` sind
  informativ: `acme_future_cancel` ist immer aufrufbar und sicher; ein
  „eager“ Core (alle Futures sind bei Rückgabe schon fertig, z. B. der
  Zig-Stub) hat nichts abzubrechen. Unbekannte Bits MÜSSEN ignoriert werden.

Minor-Erhöhung = nur Ergänzungen (neue Capability-Bits, neue Statuscodes, die
alte Hosts als generischen Fehler behandeln). Alles andere = Major.

## 3. Handles und Ownership

```c
typedef uint64_t acme_object_handle;  // Ressourcen (Client, Session, …)
typedef uint64_t acme_future_handle;  // Ergebnis einer async-Operation
typedef uint64_t acme_stream_handle;  // Pull-Stream
```

| Regel | |
|---|---|
| **Wert 0** | ist nie gültig (`ACME_NULL_HANDLE`). Konstruktoren und statische Funktionen werden mit `object = 0` aufgerufen. |
| **Opak** | Hosts DÜRFEN Handles nicht interpretieren. (Rust/Go/Zig kodieren die Art im obersten Byte — nur zur Lesbarkeit in Logs.) |
| **Nie wiederverwendet** | Ein Core MUSS Handle-Werte innerhalb eines Prozesses eindeutig halten (z. B. monotoner 64-Bit-Zähler). Dadurch wird jeder Zugriff auf ein freigegebenes Handle *erkannt* statt ein neues Objekt zu treffen (ABA-frei). |
| **Besitz** | Jedes Handle, das der Host erhält (`out_future`, Handle im Response-Payload eines Konstruktors oder einer `stream<T>`-Funktion), gehört dem Host. Er MUSS es genau einmal mit der passenden `*_drop`-Funktion freigeben. |
| **Ungültig wird** | ein Handle ausschließlich durch seinen eigenen `*_drop`-Aufruf. `acme_object_drop` invalidiert *nicht* die Futures/Streams des Objekts: laufende Futures werden abgeschlossen (Domain-Fehler wie `disconnected` oder `ACME_ERR_CANCELLED`), Streams enden (`ACME_STREAM_END` bzw. `ACME_ERR_CANCELLED`). Ihre Handles bleiben gültig, bis der Host sie freigibt. |
| **Double-Free** | Zweites `*_drop` → `ACME_ERR_INVALID_HANDLE`. Kein UB, kein Absturz. |
| **Falsche Art** | Future-Handle an `acme_stream_next` usw. → `ACME_ERR_INVALID_HANDLE`. |
| **Thread-Safety** | Alle Funktionen sind von beliebigen Threads gleichzeitig aufrufbar, auch für dasselbe Handle. `drop` parallel zu einem anderen Aufruf auf demselben Handle ist erlaubt: der andere Aufruf sieht entweder das lebende Objekt oder `ACME_ERR_INVALID_HANDLE`. |

SDKs kapseln Objekt-Handles in einem Typ, der genau einmal freigibt
(C#: `ObjectHandle` mit `Interlocked.Exchange` + Finalizer, Java: `Cleaner`,
Swift: `deinit`). Futures und Streams gibt das SDK in `finally` frei.

## 4. Futures

```c
acme_status acme_call_async(object, method_id, request, &future);
acme_status acme_future_subscribe(future, callback, user_data);
acme_status acme_future_poll(future);                  // PENDING | READY
acme_status acme_future_take(future, &out_result);     // Ergebnis herausbewegen
acme_status acme_future_cancel(future);
acme_status acme_future_drop(future);
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

* `cancel` vor Abschluss → Outcome `ACME_ERR_CANCELLED`, spätere Fertigmeldung
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
typedef void (*acme_future_callback)(void *user_data, acme_future_handle future);
```

* `acme_future_subscribe` liefert
  * `ACME_OK` → Callback wird **genau einmal** aufgerufen, *auch* wenn das
    Future abgebrochen oder freigegeben wird. Damit ist der Callback der eine,
    sichere Ort, an dem der Host `user_data` freigibt (C#: `GCHandle.Free`,
    Swift: `Unmanaged.takeRetainedValue`, Java: Eintrag aus der Map entfernen).
  * `ACME_READY` → schon fertig, Callback wird **nie** aufgerufen.
  * `ACME_ERR_BUSY` (zweites Subscribe) / `ACME_ERR_INVALID_HANDLE` → nie aufgerufen.
* Der Callback trägt **kein Ergebnis**; der Host ruft danach `acme_future_take`.
* Er wird **nie innerhalb eines ABI-Aufrufs des Hosts** ausgeführt (kein
  Reentrancy-Risiko für Host-Frames). Muss ein Core wegen `cancel`/`drop`
  benachrichtigen, geschieht das asynchron auf einem Core-Thread.
* Siehe §6 für Threads.

### 4.3 Bewertung: Ergebnis im Callback vs. separates `take`

| | Ergebnis im Callback (`cb(ud, status, ptr, len)`) | **Nur Signal + `take` (gewählt)** |
|---|---|---|
| FFI-Aufrufe pro Operation | 1 weniger | +1 (`take`) — bei HTTP/WebSocket-Latenzen irrelevant |
| Lebensdauer des Puffers | nur während des Callbacks gültig → Host muss *im fremden Thread* sofort kopieren | Host kopiert, wann und wo er will; Puffer gehört ihm bis `acme_buffer_free` |
| Arbeit im Callback | Deserialisierung/Kopie im Core-Thread (blockiert den Core) | nur „Task fertig“-Signal, O(1) |
| Polling-Hosts (WASM, Game-Loops, Tests) | brauchen trotzdem eine zweite API | `poll` + `take` ohne Callback funktionieren direkt |
| Eager Cores | müssten Callback synchron aus `subscribe` rufen (Reentrancy) | `subscribe` → `ACME_READY`, fertig |
| Go/cgo | Übergabe von Zeigern an C-Callbacks mit Pinning-Regeln | nur `uint64_t` + `void*` |

Der eine zusätzliche Aufruf kostet Nanosekunden; die gewonnene Einfachheit bei
Ownership und Threading ist für den Use Case (HTTP, WebSocket, Krypto) klar
wertvoller.

### 4.4 `take`

`take` bewegt das Outcome genau einmal heraus:
`ACME_OK` + Payload, `ACME_STREAM_END` (nur bei `next`-Futures),
`ACME_ERR_CANCELLED`, `ACME_PENDING` (noch nicht fertig, nichts entnommen),
`ACME_ERR_ALREADY_TAKEN` oder ein anderer Fehler (Payload = UTF-8-Diagnose).

### 4.5 Mapping in den SDKs

| Sprache | Typ | Abbruch |
|---|---|---|
| C# | `Task<T>` / `Task` | `CancellationToken` → `UnsafeRegister(acme_future_cancel)`; ein bereits abgebrochener Token startet die Operation gar nicht |
| Java | `CompletableFuture<T>` | Unterklasse `NativeFuture`: `cancel()` ruft zusätzlich `acme_future_cancel` |
| Swift | `async throws -> T` | `withTaskCancellationHandler { … } onCancel: { acme_future_cancel }`; `Task.checkCancellation()` vor dem Start |
| Python | `Awaitable[T]` | `asyncio.Future` + `loop.call_soon_threadsafe` im Callback |
| JS (Node) | `Promise<T>` | `AbortSignal` → `acme_future_cancel`; Callback via `napi_threadsafe_function` |

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
| `acme_bytes` (Request) | Host | Host | nur während des Aufrufs gültig; der Core kopiert, was er behält |
| `acme_buffer` (Response, `take`, Diagnosen) | **Core** | **Host** mit `acme_buffer_free` *derselben* Bibliothek, exakt `{ptr,len}` wie erhalten | gehört dem Host bis zum Free; SDKs kopieren sofort in verwaltete Arrays und geben frei |
| `{NULL, 0}` | — | Free ist No-op | leerer Puffer |
| Statische Strings (`contract_id`, `core_info`, `status_name`) | Core | nie | prozessweit gültig, UTF-8, NUL-terminiert |
| `user_data` des Callbacks | Host | Host, **im Callback** (oder sofort, wenn `subscribe` ≠ `ACME_OK`) | Core fasst den Zeiger nie an, reicht ihn nur durch |
| Future-Ergebnis | Core | Core beim `take` (Übergabe als `acme_buffer`) bzw. beim `drop` | bis `take` oder `drop` |

* Alle `out`-Parameter werden auf **jedem** Pfad beschrieben (auch im Fehlerfall),
  `NULL`-Out-Zeiger → `ACME_ERR_INVALID_ARGUMENT`.
* Ein Host DARF `acme_buffer_free` nie mit einem eigenen Allocator mischen:
  Rust gibt `Box<[u8]>` frei, Go `C.free`, Zig `c_allocator.free` — genau
  deshalb besitzt jede Bibliothek ihr eigenes `free`.
* Strings sind überall UTF-8. C#/Java-Strings (UTF-16) werden strikt
  kodiert: unpaarige Surrogates → `ArgumentException`/`IllegalArgumentException`
  bevor irgendetwas die ABI überquert.

## 8. Streams

```c
acme_status acme_stream_next(stream, &future);   // Pull: Future auf das nächste Element
acme_status acme_stream_cancel(stream);          // Producer stoppen
acme_status acme_stream_drop(stream);            // cancel + freigeben
```

* Streams entstehen als Rückgabewert von WIT-Funktionen (`events: func() ->
  stream<client-event>`), das Handle steht im Response-Payload.
* **Pull-basiert = Backpressure:** Der Core liefert nur, wenn der Host fragt.
  Pro Stream höchstens ein offenes `next`-Future (`ACME_ERR_BUSY` sonst).
* `next`-Futures enden mit `ACME_OK` + Element, `ACME_STREAM_END` oder
  `ACME_ERR_CANCELLED`.
* **Abbruch eines einzelnen `next`** verwirft nur diesen Pull, **kein Element
  geht verloren** (Rust: `Stream::poll_next` ist cancel-safe; Go: ein bereits
  entnommenes Element wird zurückgelegt).
* `acme_stream_cancel`: offenes `next` → `ACME_ERR_CANCELLED`, alle späteren →
  `ACME_STREAM_END`. Idempotent.
* SDK-Mapping: C# `IAsyncEnumerable<T>`, Java `Flow.Publisher<T>` (Demand →
  `next`), Swift `AsyncSequence` (`AcmeStream<T>`), Python `AsyncIterator`,
  JS `AsyncIterable`.

## 9. Fehler — drei getrennte Ebenen

1. **ABI-Status** (`acme_status`, <0): Programmier-/Integrationsfehler —
   ungültiges Handle, kaputter Payload, unbekannte Methode, Panic im Core
   (`ACME_ERR_INTERNAL`; Cores fangen Panics an der Grenze ab). SDKs werfen
   `AcmeNativeException` / `AcmeRuntimeError.native`.
2. **Domain-Fehler** aus WIT (`result<customer, client-error>`) sind **Daten**
   im Payload (`[1, E]`), nie ein Status. SDKs mappen sie auf
   `ClientException` (C#, Java; in Java innerhalb von
   `CompletionException`/`ExecutionException`) bzw. `throws ClientError` (Swift).
3. **Abbruch** ist `ACME_ERR_CANCELLED` → `OperationCanceledException`,
   `CancellationException`, `CancellationError`.

Keine sprachspezifischen Exceptions überqueren die ABI; `extern "C"`-Grenzen
fangen Panics (Rust `catch_unwind`, Go `recover`).

## 10. Stabile Method-IDs

`method_id = (group << 8) | index`. Beides steht in
[`wit/abi-ids.lock`](../wit/abi-ids.lock), das `acme-wit-gen` pflegt:

* Beim ersten Auftreten bekommt ein Element die **nächste freie Nummer** in
  seinem Scope (Methoden: pro Ressource; Felder/Cases: pro Typ) und wird in
  die Lock-Datei geschrieben (committed).
* Danach ist **nur die Lock-Datei maßgeblich** — Umordnen in `client.wit` ändert
  nichts (Generator-Test `reordering_the_wit_never_changes_ids`).
* Entfernte Elemente bleiben als `retired` stehen; ihre Nummer wird **nie**
  neu vergeben (Test `new_functions_are_appended_and_removed_ids_are_never_reused`).
* `acme-wit-gen --check` schlägt in CI fehl, wenn Lock-Datei oder generierter
  Code veraltet sind.

Verworfen: Deklarationsreihenfolge (bricht beim Umsortieren), Hash des Namens
(Kollisionen, nicht lesbar, Rename = neue ID ohne Kontrolle), Strings über die
ABI (langsamer, fehleranfällig).

## 11. Checkliste: einen neuen Core implementieren

1. Die 17 Funktionen aus `acme_client.h` exportieren (C-Linkage, `ACME_API`).
   cgo: `#define ACME_CLIENT_NO_PROTOTYPES` vor dem Include.
2. Handle-Tabelle mit nie wiederverwendeten 64-Bit-Handles (§3).
3. Future-Zustandsautomat mit „erster Übergang gewinnt“ (§4.1), Callback
   genau einmal, nie synchron aus Host-Aufrufen.
4. Payload-Codec v1 (§5) und Dispatch `method_id → Implementierung`
   (für Rust generiert; siehe `core-rust/src/generated`).
5. Panics/Exceptions an der Grenze abfangen → `ACME_ERR_INTERNAL`.
6. `tests/abi/conformance.c` und `tests/compatibility/run.sh` grün.

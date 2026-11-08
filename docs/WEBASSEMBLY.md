# WebAssembly im JavaScript-Browser: Umsetzungsplan

Stand: 8. November 2026. **Planung, noch nicht implementiert.**

## Ziel und Umfang

Ein aus WIT erzeugtes JavaScript-/TypeScript-SDK soll einen Rust-Core als
WebAssembly im Browser ausführen können. Die Fachlogik wird zwischen nativen
Apps und Browser geteilt. `Promise`, `AsyncIterableIterator`, `AbortSignal`,
Domain-Fehler und explizites Schließen von Ressourcen bleiben die SDK-Oberfläche.
Die Initialisierung des WASM-Backends ist asynchron; danach bleiben synchrone
WIT-Methoden synchron.

Erstes Lieferziel: Rust-Core + Browser-SDK, als ESM mit Typdeklarationen und
WASM-Asset, mit einem lauffähigen HTTP-/SSE-Beispiel. Go- und C#-Cores als WASM,
Flutter Web und ein nativer Node-N-API-Backend sind eigene Vorhaben.

Davon getrennt ist **der Generator im Browser**: Das npm-Paket enthält schon
einen `wasm32-wasip1`-Generator, sein Launcher benötigt aber `node:wasi`,
`node:fs` und einen Dateisystemzugriff. Dieses Paket ist bisher keine Browser-API.
Eine spätere In-Memory-Generator-API ist unten separat geplant.

## Befund im Repository

| Stelle | Wiederverwendbar / notwendige Änderung |
|---|---|
| `example/wit/client.wit`, Lock und API-Baseline | Vertrag und IDs bleiben für den Browser-MVP gleich. |
| `generator/runtime/ts/backend.ts` | `ClientBackend` ist bereits die passende Grenze für Aufrufe, Streams und Ressourcen. |
| `generator/runtime/ts/index.ts`, `generator/src/gen_ts.rs` | Runtime-Barrel und generierte Klassen importieren `NativeBackend`; ein injizierter Backend allein verhindert daher keinen Import von `bun:ffi`. |
| Host-Registrierung in `gen_ts.rs` | Ist konkret auf `NativeBackend` typisiert; muss eine gemeinsame Host-Backend-Schnittstelle nutzen. |
| `generator/src/gen_rust.rs` | Vermischt Typen, Traits, Codec, Dispatcher und native Exporte; referenziert direkt `wabi_abi_rt`. |
| `runtime/rust/src/host.rs` | `Send`-Futures/-Streams, `Arc<dyn Any + Send + Sync>` und native Handle-Verwaltung; nicht unverändert für JS-Werte geeignet. |
| `runtime/rust/src/runtime.rs` und `exports.rs` | Core-eigene Tokio-Runtime, native Callback-/Pointer-ABI; braucht einen anderen Browser-Adapter. |
| `example/core-rust/src/api`, `session.rs`, `events.rs` | Fachlogik vorhanden, aber direkte Tokio-, Runtime- und generierte Typabhängigkeiten. Portierung betrifft auch diese Module. |
| `example/core-rust/src/transport` | Abstraktionsgrenze vorhanden, HTTP noch konkret reqwest; `BoxStream` setzt `Send` voraus. |
| `example/core-rust/src/crypto` | AES/HMAC/SHA wiederverwendbar; sichere Zufallsquelle muss für das WASM-Ziel konfiguriert werden. |

## Architekturentscheidung

**Empfehlung: zuerst Core-WASM (`wasm32-unknown-unknown`) mit `wasm-bindgen`,
Component Model später als zusätzlicher Adapter.**

```text
WIT + abi-ids.lock
    ├── gemeinsame TS-Typen, Klassen, Codec
    │     ├── Bun-Einstieg → NativeBackend → native Wabi-ABI
    │     └── Web-Einstieg → WasmBackend → wasm-bindgen-Brücke
    └── gemeinsame Rust-Typen, Traits, Codec, Dispatch
          ├── native Runtime → Tokio / nativer HTTP-Transport
          └── Web-Runtime → lokaler Executor / Fetch / Browser-Timer
                         └── dieselbe Fachlogik
```

Der Browser-Pfad übernimmt `method_id + MessagePack` und die Ownership-Regeln.
Die native C-ABI mit Rohzeigern, Callback-Funktionszeigern und `user_data` wird
nicht als Browser-Schnittstelle kopiert. Die sichere Brücke übergibt Bytes,
`bigint`-Handles und Promises; ein generischer Adapter genügt pro Vertrag.
Das kostet weiterhin Serialisierung, hält aber den ersten Umbau begrenzt.

`wasm-bindgen` dokumentiert die Übersetzung zwischen JS-Promises und Rust-Futures
sowie ESM-Ausgabe für Browser. Das begründet die Empfehlung; eine erfolgreiche
Portierung dieses Repos ist damit noch nicht nachgewiesen.
[Promises/Futures](https://rustwasm.github.io/docs/wasm-bindgen/reference/js-promises-and-rust-futures.html),
[Deployment](https://rustwasm.github.io/docs/wasm-bindgen/reference/deployment.html).

Die Alternative Component Model + `jco` passt gut zu WIT. Vor einer Einführung
müssen aber der vollständige Vertrag mit Async, Ressourcen, Streams und
Host-Imports sowie die gewählten Browser praktisch nachgewiesen werden.
`jco` bietet Transpilation und konfigurierbare Instanziierung; seine dokumentierten
JSPI-Optionen sind experimentell. WASI-Shims besitzen zusätzliche Browsergrenzen.
Der MVP erhält deshalb keine Abhängigkeit von JSPI oder WASI-Shims.
[jco](https://github.com/bytecodealliance/jco/blob/main/docs/src/transpiling.md),
[Browsergrenzen](https://github.com/bytecodealliance/jco/blob/main/packages/preview2-shim/README.md).

## Backend-Vertrag und Lebenszyklus

* `callSync`, `callAsync`, `openStream` und `dropObject` bleiben die gemeinsame
  TS-Grenze. Host-Registrierung, Abmeldung und Instanzzuordnung werden in ein
  backendneutrales Interface ausgelagert; alle Call-Sites verwenden dieses.
* Rust-Adapter stellt zusätzlich Operations- und Stream-Handles mit explizitem
  Cancel/Drop bereit. Eine Promise allein kann keine Rust-Arbeit abbrechen.
  Abbruch erreicht Fetch, Timer, laufende Stream-Pulls und wartende Host-Calls.
* Erster terminaler Übergang gewinnt auch im Browser. Späte Antworten werden
  verworfen; Listener, JS-Closures, Buffer und Handle-Einträge werden freigegeben.
* `openStream` öffnet die Subscription sofort, `next()` liest bedarfsgesteuert.
  `return()`/`break`/Abort schließen auch bei ausstehendem Pull. Queue-Limits und
  das Verhalten bei Überlauf werden vor Umsetzung festgelegt und getestet;
  keine unbeschränkte Ereignissammlung im Hintergrund.
* Handle-Typ bleibt `bigint`, ohne Umwandlung über JS-`number`. Nie wiederverwendete
  IDs, Double Drop, geliehene und übergebene Ressourcen gelten pro Backend.
  Ressourcen verschiedener Backend-Instanzen dürfen nicht vermischt werden.
* Byte-Ergebnisse erhalten eigenes JS-Ownership. Keine langlebigen Views auf
  WASM-Speicher, die durch Freigabe oder `memory.grow` ungültig werden.
* Explizites `close()` bleibt zuverlässig. Finalizer sind nur ein Fallback;
  `Symbol.dispose` wird für Browser ohne diese Unterstützung optional behandelt.
* Initialisierung prüft Vertragsversion und Capabilities vor dem ersten Client.
  Backend-Shutdown schließt alle Ressourcen und meldet Host-Handler ab. Nach
  einem WASM-Trap wird die betroffene Backend-Instanz als unbrauchbar markiert.

## Rust-Portierung

1. Generator in gemeinsame Typen/Traits/Codec/Dispatch und zielabhängige Exporte
   aufteilen. Native Exporte bleiben beim nativen Adapter; die Browser-Ausgabe
   bekommt eine eigene Runtime. Änderungen gehören in `generator/src` und
   `generator/runtime` bzw. `runtime`, nicht in kopierte SDK-Dateien.
2. Kleine Plattformgrenze für Task-Start, Timer, Cancellation, Host-Calls und
   Transport einziehen. Native Implementierung nutzt Tokio weiter; Browser
   nutzt lokale Futures und JS-Event-Loop. Browser-Futures/-Streams dürfen
   `!Send` sein. Keine unsicheren `Send`-Implementierungen für JS-Objekte.
3. Plattformabhängige Aliase/Traits für Objektablage und Streams vorsehen.
   Native `Send + Sync`-Garantien erhalten; keine globale Abschwächung nur für
   WASM. Kurze synchrone Locks können bleiben, sofern sie nie über `await`
   gehalten werden und ohne blockierendes Warten auskommen.
4. Cargo-Features und Target-Dependencies trennen: Tokio-Netzwerk,
   Multithread-Runtime, Axum-Mockserver und native Exporte nicht in den
   Browser-Build ziehen. `getrandom` mit geeignetem Browser-Backend konfigurieren.
5. Session-Pump, Event-Fanout und Connect/Disconnect-Races auf dem lokalen
   Executor verifizieren. Gleiche Fachlogik bedeutet nicht unveränderte Dateien.

## HTTP, Streams und Signaturen des Beispiels

Browser-HTTP verwendet Fetch mit `AbortController`; SSE wird aus
`Response.body` gelesen. Ein eigener inkrementeller Decoder verarbeitet
aufgeteilte UTF-8-Zeichen und SSE-Nachrichten. Der bestehende Transportzähler
zählt logische Requests; einen nativen Connection-Pool kontrolliert der Browser
nicht. Streams und Transport-Ressourcen behalten trotzdem ihre API-Semantik.

Der Mockserver benötigt eine Browser-Testkonfiguration: Same-Origin oder explizite
CORS-Freigabe für Origin, Methoden und die tatsächlich verwendeten Signatur- und
Session-Header, einschließlich Preflight. Response-Header werden nur soweit
benötigt exponiert. HTTPS/WASM-Asset-Auslieferung, CSP und Streaming-Verhalten
gehören zum Beispiel. WASM hebt Browser-Netzwerkregeln nicht auf.

**Signaturen sind ein früher Machbarkeitstest:** `host-keys.sign` erwartet
ECDSA P-256 über einen bereits berechneten SHA-256-Digest, als DER-Signatur.
WebCrypto-ECDSA hasht seinen Eingang selbst und liefert `r || s`. Nur die
Signatur nach DER umzupacken reicht deshalb nicht: `sign(digest)` würde einen
anderen Hash signieren. Siehe die Algorithmen der
[WebCrypto-Spezifikation](https://www.w3.org/TR/webcrypto/).

Für den MVP bleibt der Vertrag gleich: ein explizit als Demo ausgewiesener
JS-Signer mit Prehash-Unterstützung und DER-Ausgabe, geprüft gegen den bestehenden
Mockserver. Dabei liegt der Demo-Schlüssel im JS-Speicher. Für nicht exportierbare
WebCrypto-Schlüssel braucht es separat einen kompatibel ergänzten Import, der
die zu hashende Nachricht entgegennimmt. Kein stiller Protokollwechsel und keine
Zusage von Secure-Enclave-Schutz im Browser. Die synchronen WIT-Host-Imports
werden im MVP wie bisher durch die eigene asynchrone Host-Call-Brücke bedient.

## Pakete und geplante Nutzung

Ein vorgeschlagener zusätzlicher Generator-Target ist `ts-web`; `ts` bleibt
kompatibel zum Bun-Target. Paket-Subpfade trennen browserneutrale Dateien,
`/bun` und `/web`. Der bisherige Bun-Einstieg bleibt erreichbar; kein Browser-
Importgraph darf `bun:ffi` oder Node-Builtins enthalten, auch nicht über Barrels.

Web-Ausgabe enthält ausführbares `.js`, `.d.ts`, WASM und einen Build-Helfer.
Generieren allein kompiliert keinen kundenspezifischen Core. Der Helfer baut
diesen Core, erzeugt die `wasm-bindgen --target web`-Ausgabe und legt die Assets
in das verteilbare Paket. Dependency-/CLI-Versionen werden zueinander passend
gepinnt. WASM-URLs sind überschreibbar; Standardauflösung über `import.meta.url`.
MIME-Typ `application/wasm`, Ladefehler und ein ArrayBuffer-Fallback bei fehlendem
Streaming-Compile werden berücksichtigt.

Zielbild, noch keine existierende API:

```ts
import { DemoClient, createWasmBackend } from "<sdk-paket>/web";

const backend = await createWasmBackend({ wasmUrl: "/assets/demo_client_bg.wasm" });
const client = new DemoClient({
  endpoint: "https://api.example.test",
  requestTimeoutMs: 5000,
}, backend);
try {
  await client.connect();
  console.log(await client.getCustomer("4711"));
} finally {
  client.close();
  backend.close();
}
```

Ein Worker ist eine spätere, eigene API-Variante: RPC über `postMessage` kann
die heutigen synchronen Konstruktoren und Methoden nicht transparent erhalten.
Der MVP läuft im aufrufenden JS-Kontext, ohne Threads, SharedArrayBuffer oder
COOP/COEP als Voraussetzung. Lange CPU-Arbeit ist dort zu vermeiden.

## Arbeitspakete und Abnahme

| Reihenfolge | Ergebnis und betroffene Stellen | Abnahme |
|---|---|---|
| 1. Vertikaler Spike | Minimale WIT-Fixture, Rust-WASM-Adapter und Browser-Harness unter `example/tests` | Echter Browser: Konstruktor, Sync, Async, Host-Promise, Stream und Abbruch; einschließlich `!Send`-Future, Disposal während Host-Call und Prehash-Signatur gegen Mock. Versionen und Ergebnis dokumentiert. |
| 2. TS entkoppeln | `gen_ts.rs`, `packaging.rs`, `runtime/ts`, Generator-Target/Config/Scaffold | Web-ESM und Deklarationen bauen; keine Bun-/Node-Imports im Browsergraph; bisherige Bun-API und Tests funktionieren. |
| 3. Rust trennen | `gen_rust.rs`, Core-Runtimes, Beispiel-Cargo und Plattformadapter | Gleiche Fachlogik baut nativ und für `wasm32-unknown-unknown`; Browser benutzt keine native Runtime. |
| 4. Vollständiger Browser-Pfad | Generischer `WasmBackend`, Handles, Host-Scopes, Cancel, Streams, Metadaten | Ressourcen-Ownership, Race-Verhalten und Fehler entsprechen den bestehenden SDK-Verträgen. |
| 5. Demo und Distribution | Browser-App, Fetch/SSE, CORS-Testserver, Build-Helfer und npm-Packtest | Installiertes Paket funktioniert in Vite und per direktem Browser-ESM; kein Repository-Pfad nötig. |
| 6. CI und Dokumentation | `.github/workflows/ci.yml`, `scripts/test-all.sh`, Beispiel-Anleitung | Chromium, Firefox und WebKit bestehen die Browser-Suite; native Checks und Regenerierungsprüfung bleiben grün. |

Die Fixture aus Paket 1 ist bewusst klein; sie klärt die Plattformgrenzen, bevor
der volle Beispiel-Core umgebaut wird. Paket 3 und 4 liefern anschließend die
vollständige Funktionalität. Jeder Schritt wird als getrennte Änderung reviewbar.

Testfälle aus `example/tests/ts` in eine gemeinsam verwendbare Vertragssuite
und plattformspezifischen Start/Transport aufteilen. Die C-ABI-Tests bleiben
native Tests; Browser-Gleichwertigkeit wird auf SDK-Ebene geprüft. Zusätzlich:

* Unicode, 4-MiB-Payloads, `u64` oberhalb `Number.MAX_SAFE_INTEGER`, Domain- und
  Protokollfehler, unpassende Vertragsversion.
* Parallele Calls, Cancel/Completion-Race, Abbruch vor Start und während Fetch,
  Timeout, Stream-`break` bei ausstehendem Pull, zwei unabhängige Subscriptions.
* Ressourcen als Borrow/Own/Rückgabe, Double Drop, Backend-Mischung, zwei SDKs
  und mehrere Backends in einer Seite, Host-Handler pro Instanz, Abmeldung bei
  laufender Arbeit und Antworten in anderer Reihenfolge.
* Deterministische Handle-/Listener-Zähler nach Stressläufen, WASM-Memory-Growth,
  fragmentiertes SSE, Queue-Überlauf und Netzwerkverlust. Nicht auf GC warten.
* HTTP und Signaturen gegen denselben externen Mockserver wie native Tests;
  echte Preflights sowie Asset-404/CSP-/Initialisierungsfehler im Browser testen.

Bundlegröße (JS und WASM, roh/komprimiert), Ladezeit und Call-/Stream-Durchsatz
werden zunächst gemessen und dokumentiert. Belastbare Grenzwerte werden aus
dem Spike abgeleitet. Keine Performance- oder Browserkompatibilitätszusage nur
auf Grundlage eines erfolgreichen Rust-Builds.

## Spätere Ausbaustufen

**Component Model:** Separater Spike mit dem echten `client-sdk`-World und
gepinnten `wit-bindgen`-/`jco`-Versionen. Insbesondere synchrone `host-keys`-Imports
gegen asynchrone JS-Signer auflösen, WIT-Streams und Own/Borrow testen und benötigte
WASI-Imports inventarisieren. Async im Parser ist kein Nachweis der gesamten
Toolchain. `wasm32-wasip2` allein löst das nicht; `wit-bindgen` beschreibt eine
eigene Async-ABI. Danach anhand der Browsermatrix über einen direkten
Component-Backend mit generiertem Typ-/Ressourcenadapter entscheiden.
[Rust-Ziel](https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip2.html),
[wit-bindgen Async](https://docs.rs/wit-bindgen/latest/wit_bindgen/macro.generate.html).

**Generator selbst im Browser:** Parser, Modellbildung, Lock-/Baseline-Prüfung
und Emitter aus `main.rs` in eine Bibliotheks-API extrahieren. Eingabe sind
virtuelle relative Dateipfade und Inhalte einschließlich WIT-Dependencies,
Konfiguration, Lock und Baseline; Ausgabe ist eine Dateimap plus Diagnostik.
CLI-Dateisystemzugriffe bleiben außen. WASM-Bindings liefern eine Worker-taugliche
JS-API; Download/ZIP und Persistenz übernimmt die Anwendung. Golden-Vergleich
gegen CLI-Ausgabe und gleiche `--check`-/Kompatibilitätsregeln sichern das ab.
Kein automatisches Mounten eines echten Host-Dateisystems im Browser.

# WebAssembly Component Model als weiterer Backend

Stand: 8. November 2026. **Architekturoption, noch nicht implementiert.**
Der konkrete Browser-Umsetzungsplan steht in [WEBASSEMBLY.md](WEBASSEMBLY.md).
Er sieht zunächst `wasm-bindgen` vor und hält einen Component-Backend offen.

```text
client.wit ──┬──▶ Wabi Native ABI ──▶ Rust / Go / C# Core
             ├──▶ Wabi Browser-Backend ──▶ Rust Core-WASM (geplant)
             └──▶ Canonical ABI ──▶ client.component.wasm (später prüfen)
```

## Wiederverwendbare Grundlagen

Der öffentliche Vertrag beschreibt Typen, Ressourcen, Async-Methoden und Streams
in WIT. Das erleichtert eine spätere Component-Umsetzung. Die SDKs besitzen
Backend-Schnittstellen; einige Sprachen außerdem noch unimplementierte
`ComponentModelBackend`-Platzhalter.

Die Rust-Fachlogik ist jedoch nicht vollständig plattformneutral: generierte
Traits und Typen hängen an `wabi_abi_rt`, und API, Session und Events verwenden
Tokio sowie `Send`-Streams. Auch Transport, Timer, Zufallsquelle und Host-Calls
müssen angepasst werden. Der vorherige Entwurf unterschätzte diese Arbeit.

## Voraussetzung: die gesamte Toolchain nachweisen

Ein erfolgreicher WIT-Parse beweist weder den Component-Build noch die
Browser-Ausführung. Vor einer Architekturentscheidung muss ein Spike mit dem
echten `client-sdk`-World Folgendes zeigen:

* Ressourcen-Konstruktoren, synchrone und asynchrone Methoden, `own`/`borrow`
  sowie WIT-Streams funktionieren mit gepinnten Werkzeugversionen.
* Die in WIT synchron deklarierten `host-keys`-Imports lassen sich mit den
  asynchronen JS-Host-Handlern verbinden. Die native Runtime tut das heute über
  ein eigenes Host-Call-Protokoll; daraus folgt keine automatische Canonical-
  ABI-Unterstützung. Gegebenenfalls braucht es eine explizite Adapter-World
  oder eine kompatibel geplante Vertragserweiterung.
* Browser führen die von `jco` erzeugten Module mit allen benötigten Features
  aus. Async-Instanziierung allein beweist keine Async-Aufrufunterstützung.
* HTTP, Event-Streams und Cancellation funktionieren über die tatsächlich
  verfügbaren Host-Imports. WASI-HTTP oder ein Shim wird nicht allein deshalb
  vorausgesetzt, weil die Komponente gebaut werden kann.

`wit-bindgen` dokumentiert Bindings für die Component-Async-ABI.
`wasm32-wasip2` ist ein Component-Ziel; ein bloßer Wechsel des Cargo-Targets
portiert den bestehenden Tokio-Core nicht. `jco` dokumentiert ESM-Transpilation,
Instanziierung und experimentelle JSPI-Optionen. Die geeignete Kombination
wird im Spike festgelegt und in der Browsermatrix geprüft.
[wit-bindgen](https://docs.rs/wit-bindgen/latest/wit_bindgen/macro.generate.html),
[Rust-Ziel](https://doc.rust-lang.org/rustc/platform-support/wasm32-wasip2.html),
[jco](https://github.com/bytecodealliance/jco/blob/main/docs/src/transpiling.md).

## Zwei mögliche Integrationen

**Direkter JS-Backend:** Ein generierter Adapter übersetzt zwischen vorhandener
SDK-Oberfläche und Component-Exports. Typdarstellung, Domain-Fehler, Ressourcen-
Ownership, Host-Zuordnung, Abbruch und Streams benötigen explizite Abbildungen.
Ein `jco`-ES-Modul allein ist noch kein kompatibler `ClientBackend`. Alternativ
wäre eine neue, direkte Component-SDK-Oberfläche möglich; das wäre eine separate
API-Entscheidung.

**Nativer Component-Host:** Ein Rust-/Wasmtime-Host könnte dieselbe native Wabi-
ABI exportieren und Aufrufe in Component-Werte übersetzen. Ein aus WIT und Lock
abgeleiteter Adapter ordnet Methoden und Typen zu. Zusätzlich erforderlich sind
Ressourcentabellen, Host-Imports, Async-/Stream-Lebenszyklen und Cancellation.
Dieser Core müsste die bestehende native Kompatibilitätsmatrix bestehen.

Die beiden Integrationen können später getrennt eingeführt werden. Der Browser-
MVP benötigt keinen nativen Component-Host; die gemeinsame Plattformgrenze aus
[WEBASSEMBLY.md](WEBASSEMBLY.md) dient auch einer späteren Component-Portierung.

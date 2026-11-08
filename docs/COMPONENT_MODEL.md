# WebAssembly Component Model — wie es später ergänzt wird

```
client.wit ──┬──▶ Wabi Native ABI (dieses Repo) ──▶ Rust / Go Core
             └──▶ Canonical ABI ──▶ client.component.wasm
```

`client.wit` ist bereits gültiges Component-Model-WIT (`async func`,
`stream<T>`, `resource`, geparst mit `wit-parser` 0.259, dem Parser von
`wasm-tools`/`wit-bindgen`). Nichts im öffentlichen Vertrag ist
Native-spezifisch. Deshalb bleibt der Weg zur Komponente offen.

## Was schon vorbereitet ist

| Baustein | Stand |
|---|---|
| WIT nutzt nur Component-Model-Konstrukte | ✔ (`world client-sdk { import host-keys; export api; }`) |
| Rust-Core-Logik ist frei von FFI | ✔ `example/core-rust/src/api` implementiert den generierten Trait `ClientResource` mit normalem async Rust |
| SDK-Klassen hängen nur an einer Backend-Schnittstelle | ✔ `IClientBackend` (C#), `ClientBackend` (Java, Swift) — `DemoClient(options, backend)` |
| Platzhalter `ComponentModelBackend` | ✔ in C# und Swift (wirft „not implemented“) |
| Async/Stream-Modell | ✔ Handles für Futures/Streams — dasselbe Denkmodell wie die async Canonical ABI (future/stream-Handles, waitables) |

## Schritt 1: den Core als Komponente bauen

Die Geschäftslogik des Beispiels (`api/`, `session`, `protocol/`, `crypto/`)
bleibt. Es ändern sich nur die Ränder:

* **Export-Adapter statt `ffi/`:** `wit-bindgen` (mit async-Support) erzeugt
  den Guest-Trait für `demo:client/api`. Er entspricht dem heute generierten
  `ClientResource`-Trait; die Implementierung ist ein Einzeiler pro Methode
  auf `Client` bzw. kann von `wabi-wit-gen` erzeugt werden.
* **Transport:** `transport/` bekommt neben reqwest eine Implementierung über
  `wasi:http` (WASI 0.3) bzw. einen Host-Import; `transport/` ist bereits die
  Abstraktionsgrenze.
* **Host-Calls:** Das importierte Interface `host-keys` wird ein echter
  Komponenten-Import; der generierte Client (`host_keys::sign`) bleibt.
* **Runtime:** Tokio → der async-Executor des Guests (`wit-bindgen`-Runtime).
  Session, Verschlüsselung und Event-Kanal (`session.rs`) bleiben unverändert.

```
cargo build --target wasm32-wasip2 --release   # → client.component.wasm
wasm-tools component wit client.component.wasm # zeigt exakt demo:client/api
```

## Schritt 2: die Komponente von den SDKs aus nutzen

Zwei Wege, beide ohne Änderung an den generierten SDK-Klassen:

### A. Komponente als „weiterer Core“ hinter der Native ABI (empfohlen für .NET/JVM/Swift)

Ein kleiner nativer Host (Rust + `wasmtime`) exportiert **dieselben 17
ABI-Funktionen** und leitet generisch weiter:

```
wabi_call_async(obj, method_id, msgpack)
   │  method_id ─▶ Funktionsname   (Tabelle aus abi-ids.lock, generiert)
   │  msgpack   ─▶ wasmtime::component::Val (generisch aus dem WIT-Typ)
   ▼
Component-Aufruf (Canonical ABI) ─▶ Val ─▶ msgpack ─▶ wabi_future_take
```

Die Übersetzung ist tabellengetrieben (WIT-Typen sind zur Laufzeit bekannt),
also wieder **kein Code pro Methode**. Für C#, Java und Swift ist das nur
eine andere `libdemo_client` — die Compatibility-Matrix
(`example/tests/compatibility/run.sh`) prüft sie wie jeden anderen Core.

### B. Direktes Component-Backend (Browser/Node)

Für JavaScript erzeugt `jco transpile client.component.wasm` direkt ein
ES-Modul; `async func` wird zu `Promise`, `stream<T>` zu einem async
Iterable. Die native ABI entfällt dort vollständig. Ein JS-SDK auf Basis
desselben Generators würde gegen dieses Modul statt gegen N-API binden.

Für .NET/JVM gibt es bisher keine ausgereiften
Component-Model-Hosts mit async-Support; daher Weg A. Sobald es sie gibt,
implementiert `ComponentModelBackend` `IClientBackend` direkt: `CallAsync`
mappt `method_id` → exportierte Funktion, der Payload wird in
Component-Werte übersetzt (derselbe Codec, andere Senke).

## Warum die Architektur das nicht verbaut

* Die öffentliche API ist WIT, nicht C: Typen, Fehler (`result`), Async
  (`async func`) und Streams sind Component-Model-Semantik; die native ABI
  bildet sie nur ab.
* Keine Native-Besonderheit ist in die SDK-Oberfläche durchgesickert: keine
  Zeiger, keine Handles, keine Statuscodes in öffentlichen Typen.
* Ressourcen-Lebenszyklus (Konstruktor, Methoden, `drop`) entspricht
  `resource`-Semantik des Component Model.

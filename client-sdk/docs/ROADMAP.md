# Roadmap / geplante Erweiterungen

## Generator: WIT-Parsing in Rust, Emitter in Bun/TypeScript

**Status:** geplant, nicht umgesetzt.

Heute ist `acme-wit-gen` vollständig in Rust geschrieben. Das Parsen von WIT
übernimmt `wit-parser`, die Emitter pro Sprache sind reines String-Templating.
Eine Aufteilung lohnt sich, sobald

* das Team die Templates lieber in TypeScript pflegt,
* ein JS/TS-SDK (Node/N-API oder Component Model via `jco`) dazukommt,
* Templates ohne Rust-Build iteriert werden sollen (`bun run gen`).

### Schnitt

```
acme-wit-gen (Rust, bleibt klein)            generator-ts (Bun)
  WIT parsen (wit-parser)                      liest model.json
  IDs aus abi-ids.lock vergeben/prüfen    ──▶  Emitter: C#, Java, Swift, Dart, Rust-Core, TS
  Header gegen ABI-Tabelle prüfen              schreibt generated/*, --check
  schreibt model.json (IR + fertige IDs)
```

* **ID-Vergabe bleibt in Rust**: genau eine Stelle schreibt `abi-ids.lock`;
  `model.json` enthält nur fertige, stabile Nummern.
* `model.json` ist die serialisierte IR aus `generator/src/ir.rs` plus die
  ABI-Tabelle aus `abi_table.rs`, damit die TS-Emitter P/Invoke/FFM/FFI-Bindings
  ohne eigene Header-Kenntnis erzeugen.
* **Migrationssicherung:** Golden-Test: die TS-Emitter müssen byte-identisch
  dieselben Dateien erzeugen wie die Rust-Emitter; danach muss
  `tests/compatibility/run.sh` unverändert grün sein. Erst dann die
  Rust-Emitter entfernen.

## Weitere Punkte

* Android/Java: generisches `JniBackend` (C-Shim leitet die 17 ABI-Funktionen weiter).
* Go-Emitter für den Core-seitigen Dispatch (`alt-cores/go/dispatch.go`).
* Flutter-Plugin-Paketierung des Dart-SDKs über Dart „native assets“ (`hook/build.dart`).
* `wss://` im Rust-Core (tokio-tungstenite mit `rustls-tls-webpki-roots`).
* Optionaler `Unknown`-Case für Variants zur Vorwärtskompatibilität.

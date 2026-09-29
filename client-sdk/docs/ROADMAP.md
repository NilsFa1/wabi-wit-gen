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

## Priorität HOCH (als Nächstes): Flutter-Paketierung über Dart „native assets“

**Ziel:** Eine Flutter-App trägt nur `acme_client` in die `pubspec.yaml` ein.
Es gibt keine manuellen Schritte für jniLibs, xcframework, DLL oder dylib mehr
(heute nötig, siehe `docs/FLUTTER.md`).

* `sdk/dart/hook/build.dart` (Dart native assets / `package:hooks`) liefert pro
  Zielplattform und Architektur die passende `libacme_client` als `CodeAsset`.
  Quelle: vorgebaute Artefakte aus `scripts/build-native.sh` (CI-Release) oder
  optional ein lokaler `cargo build`.
* Das SDK lädt die Bibliothek dann über das Asset (`@Native`/`DynamicLibrary`
  per Asset-ID) statt per Namens- und Pfadkonvention; `ACME_NATIVE_LIB` bleibt
  als Override für Tests.
* Abnahme: eine Beispiel-App (`examples/flutter`) baut und läuft auf Android
  (arm64 + x86_64-Emulator), iOS-Simulator, macOS, Windows und Linux, und die
  Dart-Testsuite läuft in CI über `flutter test`.
* Braucht eine Umgebung mit Flutter-SDK, Android-NDK und Xcode (CI-Runner).

## Erledigt

* ~~Android/Java: generisches JNI-Backend~~ → `runtime/java/JniAbi.java` +
  `runtime/java/jni/acme_jni.c`; Android-Jar mit Java-17-Bytecode; in der
  Kompatibilitätsmatrix gegen alle Cores getestet.

## Weitere Punkte

* Go-Emitter für den Core-seitigen Dispatch (`alt-cores/go/dispatch.go`).
* `wss://` im Rust-Core (tokio-tungstenite mit `rustls-tls-webpki-roots`).
* Optionaler `Unknown`-Case für Variants zur Vorwärtskompatibilität.

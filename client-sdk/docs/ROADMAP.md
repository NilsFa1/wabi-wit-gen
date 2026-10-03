# Roadmap / geplante Erweiterungen

## Generator: WIT-Parsing in Rust, Emitter in Bun/TypeScript

**Status:** geplant, nicht umgesetzt.

Heute ist `wabi-wit-gen` vollständig in Rust geschrieben. Das Parsen von WIT
übernimmt `wit-parser`, die Emitter pro Sprache sind reines String-Templating.
Eine Aufteilung lohnt sich, sobald

* das Team die Templates lieber in TypeScript pflegt,
* ein JS/TS-SDK (Node/N-API oder Component Model via `jco`) dazukommt,
* Templates ohne Rust-Build iteriert werden sollen (`bun run gen`).

### Schnitt

```
wabi-wit-gen (Rust, bleibt klein)            generator-ts (Bun)
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

## Flutter-Paketierung über Dart „native assets“

**Status:** umgesetzt; auf Linux verifiziert, die übrigen Plattformen laufen in
den neuen CI-Jobs (`flutter-android`, `flutter-apple`, `flutter-windows`) und
sind beim ersten Lauf zu bestätigen.

Eine Flutter-App trägt nur noch das SDK-Paket in die `pubspec.yaml` ein; der
Build-Hook bündelt die Core-Bibliothek (siehe [`docs/FLUTTER.md`](FLUTTER.md)).

* Generator: `generated/dart/build_hook.dart` je WIT-Paket (Voreinstellungen aus
  `wabi-gen.conf`: `dart_prebuilt_dir`, `dart_release_url`) und `@Native`-
  Bindings (`NativeAbi.asset()`); `hook/build.dart` der Pakete `wabi_client`,
  `gematik_pki` und `gematik_zero` leitet nur weiter.
* Generische Hook-Logik `generator/runtime/dart/native_assets.dart`: Quellen `library`,
  `prebuilt_dir`, `release_url` (SHA-256 aus `native/SHA256SUMS`), `required`;
  Universal-Mach-O wird für Flutter pro Architektur ausgedünnt.
* Laufzeit: `WABI_NATIVE_LIB` → Code-Asset → Namenskonvention.
* Build-Skripte legen für iOS zusätzlich dynamische Bibliotheken ab
  (`ios/`, `ios-simulator/`), weil native assets keine statischen unterstützt.
* Nachgewiesen (Linux): Hook-Tests für alle Zielplattformen
  (`sdk/dart/test/build_hook_test.dart`), `dart test` der Pakete ohne
  `WABI_NATIVE_LIB`, `dart run` des Dart-Beispiels, `flutter test` und
  `flutter test integration_test -d linux` von `examples/flutter`,
  `flutter build linux` (Bibliothek in `bundle/lib/`).
* Offen: erster grüner Lauf der CI-Jobs für Android (APK + x86_64-Emulator),
  iOS-Simulator, macOS und Windows; die iOS-dylib aus dem Go-Core
  (`core-go/build.sh ios`, aus dem c-archive gelinkt) ist ungetestet;
  Veröffentlichen mit `release_url` + `native/SHA256SUMS` gehört zum
  Paketierungsplan ([`docs/PACKAGING.md`](PACKAGING.md)).

## Offen: Host-Calls (ABI 1.1) im Zig-Core

**Status:** bewusst offen gelassen.

Host-Calls funktionieren in allen SDK-Sprachen (C#, Java Panama/JNI, Dart,
Swift) und in den Cores Go, Rust und C#. Der Zig-Core (`alt-cores/zig`) ist ein
eager Stub mit ABI 1.0 und kann noch keine Host-Calls auslösen. Dafür braucht es:

* `<prefix>_host_register` / `<prefix>_host_reply` in `wabi_core.zig`
  (Handler + `user_data` global, offene Calls in einer Tabelle, Request bis zur
  Antwort halten, späte Antworten nach Abbruch akzeptieren),
* echte asynchrone Futures (heute sind Futures „born complete“): ein Host-Call
  wartet auf die App, also muss die Arbeit auf einem eigenen Thread laufen,
* einen zero-Core in Zig (oder ein Zig-Beispielpaket mit Imports) und die
  Aufnahme in `packages/zero/run-tests.sh`.

## Erledigt

* ~~TypeScript-SDK~~ → für **Bun**: `generator/src/gen_ts.rs` (Target `ts`) +
  `generator/runtime/ts` (bun:ffi, threadsafe `JSCallback`s, Structs per Wert je nach
  Aufrufkonvention); Paket `packages/zero/sdk/ts` (inkl. Host-Calls), getestet auf
  Linux, macOS arm64 und Windows. Offen: Browser (Core als WebAssembly), Node
  (bräuchte N-API statt bun:ffi).

* ~~Android/Java: generisches JNI-Backend~~ → `JniAbi.java` +
  `jni/wabi_jni.c` (heute in `generator/runtime/java`); Android-Jar mit Java-17-Bytecode; in der
  Kompatibilitätsmatrix gegen alle Cores getestet.
* ~~C#-Core-Emitter~~ → `generator/src/gen_csharp_core.rs` (Target
  `csharp-core`, `csharp_core_out`): Interfaces pro Ressource, `ICore`,
  Dispatcher, Import-Clients für Host-Calls, Symbol-Präfix/Contract-ID.
  `alt-cores/csharp` und `packages/zero/core-csharp` enthalten nur noch
  Geschäftslogik; getestet mit C-Konformität, C#- und Dart-SDK, Host-Calls.
* ~~Go-Emitter für den Core-seitigen Dispatch~~ → `generator/src/gen_go.rs` +
  `runtime/go`; `alt-cores/go` enthält nur noch Geschäftslogik.
* ~~Host-Calls (ABI 1.1): der Core ruft die App~~ → SDKs: C#, Java (Panama,
  JNI), Dart, Swift; Cores: Go (`runtime/go`), Rust (`runtime/rust`,
  Crate `wabi-abi-rt`), C# (`runtime/csharp-core`). Beispiel: `packages/zero`
  mit drei austauschbaren Cores (Go mit gematik libzero, Rust, C#).

## Java-Cores (GraalVM native-image)

**Status:** PoC mit `packages/pki` (gemLibPki), dort nur noch **Referenz-Core**
für Vergleichstests auf Linux; ausgeliefert wird der Go-Core. Java-Cores werden
**nicht auf allen Plattformen unterstützt**: GraalVM native-image hat keine
Android/iOS-Targets; der Versuch über Gluon Substrate wurde verworfen (öffentliche
Toolchain steht auf GraalVM 23 von 2024). Offen:

* Streams und Host-Calls für Java-Cores (Runtime + `java-core`-Emitter),
* native Builds für macOS/Windows (Android/iOS: siehe oben, nicht geplant),
* Größe: das Image ist ~65 MB (gemLibPki zieht dss/xades4j/Guice mit);
  Abhängigkeiten ausdünnen oder `-Os`/`--gc` Optionen prüfen,
* TSL-Signaturprüfung (TUC_PKI_001) und OCSP im Test (Mock-Responder),
* gemLibPki aktualisieren: der Core nutzt 0.9.3 (alte groupId
  `de.gematik.pki.gemlibpki`, 2022); aktuell ist `de.gematik.pki:gemLibPki:5.0.2`
  (mit dss), native-image-Konfiguration neu aufzeichnen.

## gematik:pki Go-Core (gempki, EUPL-1.2)

**Status:** TUC_PKI_018 im TSL-Vertrauensmodell (TI 1.0) nach gemLibPki 5.0.2,
nur ECC; Builds für Linux/Windows/macOS/Android/iOS. Offen:

* ~~TSL-Manager: `set-tsl`, `refresh-tsl` (`.sha2`-Abgleich), `tsl-info`,
  atomarer Wechsel~~ und ~~OCSP-Cache (CertID, nextUpdate, LRU, gebündelte
  Anfragen)~~ — erledigt, auch im Java-Core (dort gemLibPki-`OcspRespCache`),
* TSL-Signatur prüfen (TUC_PKI_001, XAdES), Sequenznummer darf nicht sinken,
  NextUpdate mit Karenz erzwingen (heute nur `tsl-info.expired`),
* OCSP-Nonce (TE_1057); HTTP für TSL/OCSP optional über Host-Calls der App,
* `refresh-tsl` im Java-Core als native-image ungetestet (nur auf der JVM),
* Differenztests gegen den Java-Core mit gemLibPki 5.0.2 und dessen Testdaten,
* rechtliche Prüfung: EUPL bei statischem Linken (iOS) und App-Store-Bedingungen.

## Weitere Punkte

* Host-Calls: Abbruch in Richtung App (die App erfährt heute nicht, dass ein
  offener Host-Call aufgegeben wurde), Streams/Ressourcen als Parameter.
* Swift lokal nicht kompiliert (nur macOS-CI).
* `wss://` im Rust-Core (tokio-tungstenite mit `rustls-tls-webpki-roots`).
* Optionaler `Unknown`-Case für Variants zur Vorwärtskompatibilität.

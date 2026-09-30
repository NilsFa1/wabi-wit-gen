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
* Gilt genauso für `gematik:pki`; der Hook sollte vom Generator für jedes
  WIT-Paket erzeugt werden. Gesamtplan für Pakete und SDK-Repo:
  [`docs/PACKAGING.md`](PACKAGING.md).

## Offen: Host-Calls (ABI 1.1) im Zig-Core

**Status:** bewusst offen gelassen.

Host-Calls funktionieren in allen SDK-Sprachen (C#, Java Panama/JNI, Dart,
Swift) und in den Cores Go, Rust und C#. Der Zig-Core (`alt-cores/zig`) ist ein
eager Stub mit ABI 1.0 und kann noch keine Host-Calls auslösen. Dafür braucht es:

* `<prefix>_host_register` / `<prefix>_host_reply` in `acme_core.zig`
  (Handler + `user_data` global, offene Calls in einer Tabelle, Request bis zur
  Antwort halten, späte Antworten nach Abbruch akzeptieren),
* echte asynchrone Futures (heute sind Futures „born complete“): ein Host-Call
  wartet auf die App, also muss die Arbeit auf einem eigenen Thread laufen,
* einen zero-Core in Zig (oder ein Zig-Beispielpaket mit Imports) und die
  Aufnahme in `packages/zero/run-tests.sh`.

## Erledigt

* ~~Android/Java: generisches JNI-Backend~~ → `runtime/java/JniAbi.java` +
  `runtime/java/jni/acme_jni.c`; Android-Jar mit Java-17-Bytecode; in der
  Kompatibilitätsmatrix gegen alle Cores getestet.
* ~~Go-Emitter für den Core-seitigen Dispatch~~ → `generator/src/gen_go.rs` +
  `runtime/go`; `alt-cores/go` enthält nur noch Geschäftslogik.
* ~~Host-Calls (ABI 1.1): der Core ruft die App~~ → SDKs: C#, Java (Panama,
  JNI), Dart, Swift; Cores: Go (`runtime/go`), Rust (`runtime/rust`,
  Crate `acme-abi-rt`), C# (`runtime/csharp-core`). Beispiel: `packages/zero`
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
* C#-Core-Emitter (heute ist die Dispatch-Tabelle der C#-Cores handgeschrieben).
* Swift lokal nicht kompiliert (nur macOS-CI).
* `wss://` im Rust-Core (tokio-tungstenite mit `rustls-tls-webpki-roots`).
* Optionaler `Unknown`-Case für Variants zur Vorwärtskompatibilität.

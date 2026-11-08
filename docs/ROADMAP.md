# Roadmap

## Offen: ABI und Runtimes

* **Ressourcen in zusammengesetzten Typen:** Handles gehen als ganzer Parameter
  und als Ergebnis über die ABI (ABI.md §5.2), noch nicht in `list`, `option`,
  Records oder Streams und nicht in Host-Calls.
* **Streams in Host-Calls.**
* **Shutdown eines Dart-Isolates** mit laufender nativer Arbeit: Abmelden ist
  sauber geregelt (ABI.md §4a), das Beenden des Isolates selbst nicht.
* Optionaler `Unknown`-Case für Variants zur Vorwärtskompatibilität.

## Offen: Generator und Pakete

* **Sprachpakete:** Der Generator erzeugt pro Sprache ein fertiges Paket (NuGet-
  Projekt, Dart-Paket mit Build-Hook, SwiftPM-Paket, Bun-Paket, Java mit
  `build.sh`). Es fehlen Maven/Gradle bzw. AAR für Java/Android und das
  Veröffentlichen in Registries (pub.dev, NuGet, npm).
* **Java als Core-Sprache** (GraalVM native-image): unfertig (keine Streams,
  keine Host-Calls, nur Linux) und deshalb aus diesem Stand entfernt; der Code
  liegt auf dem Branch `java-core`.
* **JavaScript im Browser:** Rust-Core als WebAssembly mit eigenem `WasmBackend`,
  browserfähigem ESM-Paket, Fetch/SSE und bestehender SDK-Oberfläche. Zuerst
  `wasm-bindgen`, Component Model als spätere Option. Konkrete Arbeitspakete,
  technische Grenzen und Abnahmekriterien: [WEBASSEMBLY.md](WEBASSEMBLY.md).
  Der Generator selbst im Browser ist dort als separates Vorhaben beschrieben.
* **Node nativ:** N-API statt `bun:ffi`; getrennt vom Browser-Backend.
* **Emitter in TypeScript:** `wabi-wit-gen` parst WIT mit `wit-parser` und
  vergibt die IDs; die Emitter pro Sprache sind reines String-Templating und
  könnten aus einer serialisierten IR (`model.json`) auch in Bun/TypeScript
  laufen. Lohnt sich, sobald Templates ohne Rust-Build iteriert werden sollen.
  Absicherung: Golden-Test auf byte-gleiche Ausgabe, danach die Matrix.

## Offen: Plattformen

* **Flutter (Dart native assets):** Linux ist verifiziert; die CI-Jobs für
  Android (APK + x86_64-Emulator), iOS-Simulator, macOS und Windows sind beim
  ersten Lauf zu bestätigen. Veröffentlichen mit `release_url` +
  `native/SHA256SUMS` ist vorbereitet, aber noch nicht benutzt.
* **Android + Java:** Das JNI-Binding ist auf dem Desktop-JVM gegen beide Cores
  getestet, auf einem Gerät noch nicht.
* **`https://` im Rust-Core des Beispiels** (reqwest mit `rustls-tls`).

## Erledigt

* SDKs für C#, Java (Panama und JNI), Swift, Dart/Flutter, TypeScript (Bun).
* Core-Glue für Rust, Go und C# (NativeAOT), jeweils mit einem Core im Beispiel.
* Host-Calls in allen SDK-Sprachen und in den Cores Rust, Go und C#; seit ABI 1.2
  mit Zuordnung zur Instanz, Abbruch in Richtung App und sauberem Abmelden.
* Ressourcen als Parameter (`borrow<R>`, `own<R>`) und als Ergebnis (ABI 1.2).
* Eine Wire-ID steht für genau eine Signatur: geänderte Typen bekommen eine neue ID.
* Symbol-Präfix und Runtime pro Paket: mehrere SDKs in einem Prozess.
* Stabile Wire-IDs (`abi-ids.lock`) und Prüfung gegen die veröffentlichte API
  ([API_COMPATIBILITY.md](API_COMPATIBILITY.md)).
* Der Generator als npm-Paket (WebAssembly), mit
  Dart-Paket, NuGet-Projekt und `build-native.sh`.

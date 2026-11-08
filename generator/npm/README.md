# @nfails/wabi-wit-gen

Ein Vertrag in [WIT](https://component-model.bytecodealliance.org/design/wit.html),
daraus **SDKs** für C#, Java/Android, Swift, Dart/Flutter und TypeScript (Bun) und
der **Glue-Code für den Core** in Rust, Go oder C#. Alle SDKs sprechen über
dieselbe stabile native ABI (`wabi_abi.h`) mit dem Core; die Logik steckt genau
einmal im Core.

Das Paket enthält den Generator als WebAssembly (WASI). Es gibt keinen nativen
Download und kein `postinstall`, und die Ausgabe ist auf jeder Plattform
byte-gleich mit dem nativen Build.

```bash
npm install --save-dev @nfails/wabi-wit-gen   # oder: bun add -d @nfails/wabi-wit-gen
npx wabi-wit-gen wit/ --out sdk --target ts,dart,java --core rust
```

Ohne Installation: `npx @nfails/wabi-wit-gen wit/ --out sdk …` bzw.
`bunx @nfails/wabi-wit-gen …`.

Voraussetzung: Node.js ≥ 20 oder Bun. Ein vollständiges Beispiel (HTTP-Client mit
verschlüsselten Bodies, Cores in Rust, Go und C#, Apps in allen fünf Sprachen, dazu
ein Mock-Service) liegt im Repository unter
[`example/`](https://github.com/NilsFa1/wabi-wit-gen/tree/main/example).

## Vertrag

```wit
package demo:greeter@0.1.0;

interface types {
    record greeting { text: string, count: u32 }
    variant greet-error { empty-name, failed(string) }
}

interface api {
    use types.{greeting, greet-error};
    resource greeter {
        constructor(prefix: string);
        greet: async func(name: string) -> result<greeting, greet-error>;
        events: func() -> stream<greeting>;
    }
}

world greeter-sdk {
    export api;
}
```

Unterstützt werden `record`, `variant`, `enum`, `option`, `result`, `list`,
`resource` mit `constructor`, sync und `async func` sowie `stream<T>`.
Ressourcen gehen auch als Parameter (`borrow<r>`, `r`) und als Ergebnis über die
ABI. **Importierte** Interfaces sind Host-Calls: Der Core ruft die App auf, z. B.
für einen Schlüsselspeicher; die App registriert ihre Implementierung
prozessweit oder für eine einzelne Instanz.

## Aufruf

```
wabi-wit-gen <wit-Datei oder -Verzeichnis> --out <dir> [--target <sprachen>] [--core <sprache>]
```

| Option | Bedeutung |
|---|---|
| `--target` | SDK-Sprachen, mit Komma getrennt: `csharp,java,swift,dart,ts` |
| `--core` | Sprachen des Cores, mit Komma getrennt: `rust`, `go`, `csharp`. `go=<dir>` schreibt den Go-Glue direkt in das Verzeichnis des Cores |
| `--out` | Zielverzeichnis; der Generator überschreibt nur seine eigenen Dateien |
| `--lock` | Lock-Datei der Wire-IDs (Standard: `abi-ids.lock` neben der WIT) |
| `--check` | schreibt nichts; Exit-Code ≠ 0, wenn etwas veraltet ist (für CI) |
| `--api-baseline <datei>` | prüft gegen eine veröffentlichte API (inkompatible Änderung = Fehler); erzeugen mit `--write-api-baseline` |

**`abi-ids.lock` gehört ins Repository.** Die Datei legt jede numerische ID fest,
die über die ABI oder im Payload geht (Methoden, Felder, Fälle). Sie wächst nur
an, deshalb bleiben ältere Cores und SDKs kompatibel, auch wenn die WIT umsortiert
wird. Eine ID steht für genau eine Signatur: Ändern sich die Typen einer Methode
oder eines Felds, vergibt der Generator eine neue ID und legt die alte still.

## Was entsteht

```
sdk/
  abi/                 wabi_abi.h, wabi_abi_functions.h, ABI.md (normative Spezifikation)
  c/<präfix>.h         die Symbole dieses Pakets (demo_greeter_call_async, …)
  csharp/              C#-SDK + Runtime (Namespace Demo.Greeter.Runtime), net10.0,
                       Demo.Greeter.csproj -> `dotnet pack` = NuGet-Paket inkl. nativer Bibliotheken
  java/                Java-SDK + Runtime (com.demo.greeter.runtime): Panama (JDK 22+) und JNI;
                       build.sh -> Jar, Android-Jar und JNI-Shim
    jni/               JNI-Shim libdemo_greeter_jni (+ CMakeLists.txt für Android)
  swift/<Modul>/       SwiftPM-Paket: Package.swift, Sources/<Modul> (Code + Runtime),
                       Sources/C<Modul> (C-Modul mit den Headern)
  dart/                vollständiges Dart/Flutter-Paket demo_greeter (pubspec.yaml, hook/build.dart,
                       lib/demo_greeter.dart, lib/src/generated/ mit Runtime)
  ts/                  TypeScript-Paket für Bun (bun:ffi): package.json, index.ts, Code + Runtime
  core-rust/           generated.rs + Crate wabi-abi-rt + build-native.sh
  core-go/             wabi_generated.go + wabi_abi.h + Modul wabi.example/wabiabi + build-native.sh
  core-csharp/         Glue + Runtime für einen NativeAOT-Core
  native/              von build-native.sh gebaute Core-Bibliotheken pro Plattform (siehe unten)
```

Die Runtime jedes SDK liegt im Namensraum des Pakets. Deshalb können zwei mit
`wabi-wit-gen` erzeugte SDKs in derselben App laufen.

## Vom Core zur App: build-native.sh, Flutter, NuGet

Für Rust- und Go-Cores liegt `build-native.sh` bei. Das Skript baut den Core für
die gewünschten Plattformen und legt die Bibliotheken unter `sdk/native/` ab:

```bash
bash sdk/core-rust/build-native.sh core host                  # nur dieser Rechner
bash sdk/core-rust/build-native.sh core linux-x64 linux-arm64 windows-x64 android ios macos
```

| Ziel | Ergebnis | braucht |
|---|---|---|
| `linux-x64`, `linux-arm64` | `native/linux-<arch>/lib<präfix>.so` | Rust bzw. Go; fremde Architektur: zig (`pip install ziglang`) |
| `windows-x64` | `native/windows-x64/<präfix>.dll` | Windows (MSVC) oder zig |
| `macos` | `native/macos/lib<präfix>.dylib` (universal) | macOS |
| `ios` | `native/ios/`, `native/ios-simulator/` (dynamisch, für Flutter) und `native/apple/<Modul>.xcframework` (statisch, für Swift) | macOS + Xcode |
| `android` | `native/android/jniLibs/<abi>/lib<präfix>.so` (4 ABIs) | Rust: cargo-ndk + `ANDROID_NDK_HOME`; Go: `ANDROID_NDK` |

Was dort liegt, nehmen die Pakete ohne weitere Konfiguration mit:

* **Flutter / Dart:** `flutter pub add demo_greeter --path sdk/dart`. Der
  Build-Hook des Pakets (Dart native assets) packt bei `flutter run`/`build` die
  Bibliothek der Zielplattform ein; Gradle, Xcode und CMake bleiben unberührt.
  Statt `sdk/native` geht auch eine andere Quelle über die `pubspec.yaml` der App:
  `hooks: user_defines: demo_greeter: prebuilt_dir | release_url | library`.
* **.NET:** `dotnet pack sdk/csharp -c Release -o feed` erzeugt
  `Demo.Greeter.<version>.nupkg` mit den Bibliotheken unter
  `runtimes/<rid>/native/` (linux-x64, linux-arm64, win-x64, osx-arm64/-x64,
  android-*). .NET wählt zur Laufzeit selbst die passende aus. Während der
  Entwicklung geht auch eine `ProjectReference` auf `Demo.Greeter.csproj`; dann
  wird die Bibliothek dieses Rechners neben die App kopiert. Nicht enthalten:
  iOS für .NET MAUI (bräuchte eine statische Bibliothek mit `NativeReference`).
* **TypeScript (Bun):** `WABI_NATIVE_LIB=sdk/native/<plattform>/<datei>`.

## Core

Der Generator gibt nach dem Lauf die nächsten Schritte aus. Kurzfassung:

* **Rust:** `wabi-abi-rt = { path = "sdk/core-rust/wabi-abi-rt" }` und
  `crate-type = ["cdylib", "staticlib"]` in `Cargo.toml`; in `lib.rs`
  `#[path = "../sdk/core-rust/generated.rs"] pub mod generated;`, dazu
  `pub struct Core;` mit `impl generated::Implementation for Core` (die
  generierten Exports verweisen auf `crate::Core`) und pro Resource ein Typ, der
  das generierte `…Resource`-Trait implementiert:

  ```rust
  pub struct Core;
  impl generated::Implementation for Core {
      const CORE_INFO: &'static std::ffi::CStr = c"greeter-core/0.1.0";
      type Greeter = Greeter;
  }
  pub struct Greeter { prefix: String }
  impl generated::GreeterResource for Greeter {
      fn new(prefix: String) -> Self { Greeter { prefix } }
      async fn greet(&self, name: String) -> Result<generated::Greeting, generated::GreetError> { … }
      fn events(&self) -> wabi_abi_rt::host::AbiStream<generated::Greeting> { Box::pin(futures::stream::empty()) }
  }
  ```
* **Go:** `wabi_generated.go` und `wabi_abi.h` gehören ins `main`-Paket des Cores
  (direkt dorthin schreiben mit `--core go=<verzeichnis>`), in `go.mod`
  `replace wabi.example/wabiabi => ./sdk/core-go/wabiabi`, bauen mit
  `go build -buildmode=c-shared`.
* **C#:** `core-csharp/**`, `csharp/Types.g.cs`, `csharp/WireCodec.g.cs` und
  `csharp/Runtime/{MsgPack,Exceptions}.cs` kompilieren, dazu
  `global using <Ns>.Runtime;`, mit `PublishAot` und `NativeLib=Shared` veröffentlichen.

Die native Bibliothek heißt nach dem Paket (`libdemo_greeter.so`, `.dylib`, `.dll`)
und exportiert nur die präfixierten ABI-Symbole. Mehrere Cores können also
nebeneinander in einem Prozess liegen.

## SDK-Laufzeitvoraussetzungen

| Sprache | Voraussetzung |
|---|---|
| C# | .NET 10 |
| Java | JDK 22+ (Panama) oder JDK 17+/Android (JNI-Shim) |
| Swift | Swift 5.9, macOS 12 / iOS 15 |
| Dart | Dart ≥ 3.10 bzw. Flutter ≥ 3.38 (native assets), Pakete `ffi`, `hooks`, `code_assets`, `crypto` |
| TypeScript | Bun; mit Host-Calls (importierte Interfaces) Bun ≥ 1.4 |

Cores brauchen für Host-Calls ABI 1.2; die mitgelieferten Core-Runtimes (Rust,
Go, C#) bringen das mit.

## Lizenz

Apache-2.0 (`LICENSE`, `NOTICE`). Was `wabi-wit-gen` erzeugt, gehört dir: der
generierte Code und die mitkopierten Runtimes lassen sich unter jeder Lizenz
verwenden, auch in proprietären Apps, ohne Lizenzhinweis.

## Initialisierung und erneute Generierung

Beim ersten Lauf können bearbeitbare Beispiel-Apps und Core-Gerüste angelegt werden:

```bash
npx @nfails/wabi-wit-gen wit/ --out sdk --target ts,dart,csharp --core csharp --scaffold examples
```

Dabei entsteht im aktuellen Projektverzeichnis eine `wabi-gen.conf` mit relativen
Pfaden. Danach reichen im selben Verzeichnis:

```bash
npx @nfails/wabi-wit-gen          # SDKs nach WIT-Änderungen aktualisieren
npx @nfails/wabi-wit-gen --check  # CI: nur prüfen, keinerlei Dateien ändern
```

Beispielkonfiguration (von Hand bearbeitbar):

```ini
wit = wit
out = sdk
targets = ts,dart,csharp
cores = csharp
lock = wit/abi-ids.lock
```

Mit `--config pfad/wabi-gen.conf` lässt sich die Konfiguration aus einem anderen
Verzeichnis verwenden. Pfade darin beziehen sich auf den Ort der Konfiguration;
explizite CLI-Optionen überschreiben gespeicherte Werte. Eine explizite WIT-Datei
lädt die Standardkonfiguration nur zusammen mit `--config`.

SDK-Code, Runtime-Kopien, ABI-Dateien, Core-Glue, Paketdateien, Build-Helfer und
`sdk/README.md` werden aktualisiert. Der Wire-ID-Lock wird kompatibel fortgeschrieben.
Bearbeitbare Dateien unter `sdk/examples/` und Core-Gerüste wie `Core.cs`,
`src/lib.rs`, `starter.go` und deren Projektdateien werden nur bei `--scaffold examples`
und nur dann angelegt, wenn die einzelne Datei fehlt. Vorhandene Inhalte und die
Konfiguration werden auch bei erneutem Scaffolding niemals überschrieben.
`--check` ignoriert diese bearbeitbaren Dateien. Nicht mehr benötigte generierte
Dateien werden derzeit nicht automatisch gelöscht.

Die Beispiel-Apps laden den Core und prüfen dessen Metadaten. Fachliche Aufrufe
müssen mit gültigen Parametern ergänzt werden. Core-Gerüste enthalten ausdrücklich
unimplementierte Methoden; neue WIT-Methoden erfordern eine Ergänzung im eigenen
Core. Die paketbezogene `sdk/README.md` beschreibt Build und Bibliotheksablage für
die tatsächlich gewählten Cores. Ohne gewählten Core ist eine passende native
Bibliothek bereitzustellen.

Ab Generatorversion 0.2.0 zielen die erzeugten C#-SDKs und Core-/App-Gerüste auf
`net10.0`. Bereits vorhandene Scaffold-Projektdateien bleiben unverändert;
ihr `<TargetFramework>` muss bei einer Migration von Hand angepasst werden.
Die normale NativeAOT-Optimierungsstrategie bleibt erhalten; eine zusätzliche
Größenoptimierung wird nicht automatisch aktiviert.

Die mitgelieferten ABI-Dokumente und Header verwenden in SDK-Ausgaben die
Paketnamen, Symbolpräfixe und Beispieltypen aus dem WIT-Vertrag. Record- und
Ergebnisbeispiele zeigen tatsächlich deklarierte Typen; Verträge ohne solche
Typen erhalten eine neutrale Erklärung. Gemeinsame Runtime-Kommentare enthalten
keine fachlichen Typen eines Referenzprojekts.

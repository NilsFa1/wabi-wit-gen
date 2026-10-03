# Beispiel: Greeter

Ein WIT-Vertrag, ein Rust-Core und drei Apps, die ihn nutzen: TypeScript (Bun),
Dart (wie in Flutter) und .NET (NuGet-Paket).

```bash
npx @nfails/wabi-wit-gen wit --out sdk --target ts,dart,csharp --core rust   # sdk/ + wit/abi-ids.lock
bash sdk/core-rust/build-native.sh core host                           # Core -> sdk/native/<plattform>/
```

`build-native.sh` baut auch für andere Plattformen, z. B.
`bash sdk/core-rust/build-native.sh core linux-x64 linux-arm64 windows-x64 android ios macos`
(Cross-Builds per zig, Android per cargo-ndk, Apple auf macOS). Die Bibliotheken
landen in `sdk/native/`, genau dort, wo das Dart- und das NuGet-Paket sie suchen.

**Dart / Flutter:** Das Paket `sdk/dart` bringt einen Build-Hook (native assets)
mit, der die passende Bibliothek aus `sdk/native/` einpackt. Weitere
Konfiguration braucht es nicht.

```bash
cd dart_app && dart run                      # Flutter-App: flutter pub add demo_greeter --path ../sdk/dart
```

**.NET:** `sdk/csharp/Demo.Greeter.csproj` erzeugt ein NuGet-Paket, das die
Bibliotheken unter `runtimes/<rid>/native/` enthält.

```bash
dotnet pack sdk/csharp -c Release -o feed
cd dotnet_app && dotnet run                  # nuget.config zeigt auf ../feed
```

**TypeScript (Bun):**

```bash
WABI_NATIVE_LIB=sdk/native/linux-x64/libdemo_greeter.so bun app.ts
```

Ausgabe jeweils:

```
Hallo Nils (1)
Fehler: … EmptyName …
```

`core/src/lib.rs` ist der ganze Core: `pub struct Core` bindet den Vertrag an
die eigenen Typen, `Greeter` implementiert die Resource. Alles unter `sdk/` ist
generiert; `wit/abi-ids.lock` legt der erste Lauf an und gehört ins Repository.

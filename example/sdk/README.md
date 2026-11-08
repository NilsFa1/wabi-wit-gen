# SDK for demo:client@1.0.0

Generated SDK files, runtime copies, ABI headers, this guide and build helpers are
managed by wabi-wit-gen and updated on every generation.
Keep the WIT contract and its adjacent `abi-ids.lock` under version control.

The [shared ABI specification](abi/ABI.md) defines the native protocol.
The [package C header](c/demo_client.h) declares this package's exports.
The native library is `demo_client`; every ABI export begins with `demo_client_`.
Java/JNI and language namespaces use their own naming rules.

Generation does not build a core or certify its conformance. Test your own
implementation and its SDK integration before distributing it.

## Regeneration and editable starters

Opt in to starters on the first run with `--scaffold examples`. Example apps
under `examples/` and core implementation/project files are created only if
the file does not exist. Existing files are never overwritten, including on
a later scaffold run. These files are yours to edit. Core methods initially
fail explicitly as unimplemented; constructors create empty starter objects.

Scaffolding also creates `wabi-gen.conf` in the invoking project directory
(or at `--config <file>`), only if absent. Paths are relative to that file.
Then run `wabi-wit-gen` or `wabi-wit-gen --check` from the project directory.
Explicit CLI options override saved settings. Normal generation updates managed
files only; `--check` writes nothing and ignores starter contents. After a WIT
change, implement new or changed methods in your core; compile errors identify
missing implementations. Obsolete generated files are not removed automatically.

## Native core

### rust

Use the generated [build helper](core-rust/build-native.sh):

```sh
bash core-rust/build-native.sh <core-project-directory> host
```

With scaffolding and the default core destination, use `core-rust` as
the project directory. Implement the starter methods before calling them.
The helper writes libraries to `native/`; it can also build platform targets
including `ios` (producing `native/apple/DemoClient.xcframework`).

### go

Use the generated [build helper](core-go/build-native.sh):

```sh
bash core-go/build-native.sh <core-project-directory> host
```

With scaffolding and the default core destination, use `core-go` as
the project directory. Implement the starter methods before calling them.
The helper writes libraries to `native/`; it can also build platform targets
including `ios` (producing `native/apple/DemoClient.xcframework`).

Go glue is in the custom directory selected with `go=<dir>`. Add the local
`wabiabi` runtime dependency to your existing `go.mod`; scaffolding preserves
that project's manifest.

### C# / NativeAOT

Compile `core-csharp/generated/*.cs`, `core-csharp/runtime/*.cs`,
`csharp/Types.g.cs`, `csharp/WireCodec.g.cs` and
`csharp/Runtime/{MsgPack,Exceptions}.cs` in your core project.
Add `global using Demo.Client.Runtime;`, implement `Wabi.Core.ICore`
with `Wabi.Core.Core`, and enable unsafe code.

With scaffolding, `core-csharp/Core.csproj` and an editable implementation
are supplied. Otherwise substitute your own core project in this command:

```sh
dotnet publish core-csharp/Core.csproj -c Release -r linux-x64 -p:PublishAot=true -p:NativeLib=Shared -p:AssemblyName=demo_client -o native/linux-x64
# NativeAOT emits demo_client.so; the SDK loader expects libdemo_client.so:
cp native/linux-x64/demo_client.so native/linux-x64/libdemo_client.so
```

For Windows use RID `win-x64`, output directory `native/windows-x64`,
and filename `demo_client.dll`. For macOS publish separately for `osx-arm64` and
`osx-x64`, rename the dylibs to `libdemo_client.dylib` and use `lipo -create`
to place a universal library in `native/macos/libdemo_client.dylib` when shipping
both architectures. C# generation supplies no shell build helper or
Apple xcframework; provide an appropriate artifact for those targets.

### Library locations

Dart hooks and NuGet packaging consume this layout below `native/`:

| Platform | File |
|---|---|
| Linux x64 | `linux-x64/libdemo_client.so` |
| Linux arm64 | `linux-arm64/libdemo_client.so` |
| Windows x64 | `windows-x64/demo_client.dll` |
| macOS universal | `macos/libdemo_client.dylib` |
| Android | `android/jniLibs/<abi>/libdemo_client.so` |

For direct SDK use, set `WABI_NATIVE_LIB` to an absolute library path in
C#, Java, Dart or Bun. Swift links the library at build time.

## SDKs and optional examples

All commands below run from this output directory. Example commands require
a prior `--scaffold examples` run. They load and validate metadata without
calling application methods, whose valid arguments depend on your contract.

- .NET 10: `dotnet pack csharp -c Release`; native files are included under
  `runtimes/<rid>/native/`. Starter: `dotnet run --project examples/csharp`.
- Java: `bash java/build.sh` builds jars and the JNI shim. Use JDK 22+
  for Panama (with native access enabled), or JDK 17+/Android with JNI.
  Starter source: `examples/java/Main.java`; compile and run with the
  generated jar on the classpath and `WABI_NATIVE_LIB` set.
- Swift: local SwiftPM package `swift/DemoClient`. Pass
  `-Xlinker -L<absolute-library-directory>` to the consuming build; ensure
  the dynamic library is also available at runtime. Starter:
  `cd examples/swift && swift run -Xlinker -L<absolute-library-directory>`.
  For iOS supply a compatible xcframework and replace the system library
  target with a binary target.
- Dart/Flutter: depend on the local `dart/` package named `demo_client`. Its hook
  bundles the core from `native/`; alternatively configure the app's
  `hooks.user_defines.demo_client` with `library`, `prebuilt_dir` or `release_url`.
  Starter: `cd examples/dart && dart run bin/main.dart`.
- Bun: SDK in `ts/`; starter: `bun examples/ts/main.ts`. Set `WABI_NATIVE_LIB`.

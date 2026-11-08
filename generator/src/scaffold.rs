//! Opt-in starter files. These become user-owned as soon as they are created.
use crate::{ir::Model, packaging, runtime, Core};
use std::path::{Path, PathBuf};

pub fn files(out: &Path, m: &Model, targets: &[String], cores: &[Core]) -> Vec<(PathBuf, String)> {
    let mut files = vec![];
    let mut custom = vec![];
    let mut add = |path: &str, text: String| files.push((out.join(path), text));
    let ns = crate::gen_csharp::cs_ns(m);
    let module = packaging::swift_module(m);
    let dart = packaging::dart_package_name(m);
    for lang in targets {
        match lang.as_str() {
            "ts" => {
                add("examples/ts/main.ts", "// Starter: edit freely. Set WABI_NATIVE_LIB to the built core library.\nimport { NativeBackend } from '../../ts/index.ts';\nconsole.log(NativeBackend.shared.info.toString());\n// TODO: construct a generated resource and call your application's methods.\n".into());
                add("examples/ts/package.json", "{\n  \"private\": true,\n  \"type\": \"module\",\n  \"scripts\": { \"start\": \"bun main.ts\" }\n}\n".into());
            }
            "csharp" => {
                add("examples/csharp/Program.cs", format!("// Starter: edit freely. Set WABI_NATIVE_LIB to the built core library.\nusing {ns}.Runtime;\nConsole.WriteLine(NativeBackend.Shared.Info);\n// TODO: construct a generated resource and call your application's methods.\n"));
                add("examples/csharp/Example.csproj", format!("<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net8.0</TargetFramework><ImplicitUsings>enable</ImplicitUsings></PropertyGroup>\n  <ItemGroup><ProjectReference Include=\"../../csharp/{ns}.csproj\" /></ItemGroup>\n</Project>\n"));
            }
            "dart" => {
                add("examples/dart/bin/main.dart", format!("// Starter: edit freely. The SDK build hook bundles the native core.\nimport 'package:{dart}/{dart}.dart';\nvoid main() {{\n  print(NativeBackend.shared.info);\n  // TODO: construct a generated resource and call your application's methods.\n}}\n"));
                add("examples/dart/pubspec.yaml", format!("name: {dart}_example\npublish_to: none\nenvironment:\n  sdk: '>=3.10.0 <4.0.0'\ndependencies:\n  {dart}:\n    path: ../../dart\n"));
            }
            "java" => {
                add("examples/java/Main.java", format!("// Starter: edit freely. Set WABI_NATIVE_LIB to the built core library.\nimport {}.NativeBackend;\npublic class Main {{\n    public static void main(String[] args) {{\n        System.out.println(NativeBackend.shared().info());\n        // TODO: construct a generated resource and call your application's methods.\n    }}\n}}\n", runtime::java_package(m)));
            }
            "swift" => {
                add("examples/swift/Sources/Example/main.swift", format!("// Starter: edit freely. Link the built core library.\nimport {module}\nlet backend = try NativeBackend.shared.get()\nprint(backend.info)\n// TODO: construct a generated resource and call your application's methods.\n"));
                add("examples/swift/Package.swift", format!("// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(\n    name: \"{module}Example\",\n    platforms: [.macOS(.v12)],\n    dependencies: [.package(path: \"../../swift/{module}\")],\n    targets: [.executableTarget(name: \"Example\", dependencies: [.product(name: \"{module}\", package: \"{module}\")])]\n)\n"));
            }
            _ => unreachable!(),
        }
    }
    for core in cores {
        match core.lang.as_str() {
            "rust" => {
                add("core-rust/src/lib.rs", crate::gen_rust::scaffold(m));
                add("core-rust/Cargo.toml", format!("[package]\nname = \"{}-core\"\nversion = \"{}\"\nedition = \"2021\"\npublish = false\n[lib]\nname = \"{}\"\ncrate-type = [\"cdylib\", \"staticlib\"]\n[dependencies]\nwabi-abi-rt = {{ path = \"wabi-abi-rt\" }}\n", m.symbol_prefix().replace('_', "-"), m.version, m.symbol_prefix()));
            }
            "csharp" => {
                add("core-csharp/Core.cs", crate::gen_csharp_core::scaffold(m));
                add("core-csharp/GlobalUsings.cs", format!("global using {};\n", runtime::csharp_namespace(m)));
                add("core-csharp/Core.csproj", format!("<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <TargetFramework>net8.0</TargetFramework><Nullable>enable</Nullable><ImplicitUsings>enable</ImplicitUsings>\n    <AllowUnsafeBlocks>true</AllowUnsafeBlocks><PublishAot>true</PublishAot><NativeLib>Shared</NativeLib>\n    <AssemblyName>{}</AssemblyName><InvariantGlobalization>true</InvariantGlobalization>\n    <EnableDefaultCompileItems>false</EnableDefaultCompileItems>\n  </PropertyGroup>\n  <ItemGroup>\n    <Compile Include=\"*.cs\" /><Compile Include=\"generated/*.cs\" /><Compile Include=\"runtime/*.cs\" />\n    <Compile Include=\"../csharp/Types.g.cs\" /><Compile Include=\"../csharp/WireCodec.g.cs\" />\n    <Compile Include=\"../csharp/Runtime/MsgPack.cs\" /><Compile Include=\"../csharp/Runtime/Exceptions.cs\" />\n  </ItemGroup>\n</Project>\n", m.symbol_prefix()));
            }
            "go" => {
                // A custom Go destination belongs to an existing project. Offer the
                // implementation file there without creating a competing go.mod.
                if let Some(dir) = &core.dir {
                    custom.push((dir.join("starter.go"), crate::gen_go::scaffold(m)));
                } else {
                    add("core-go/starter.go", crate::gen_go::scaffold(m));
                    add("core-go/go.mod", format!("module example.local/{}\n\ngo 1.23\n\nrequire wabi.example/wabiabi v0.0.0\nreplace wabi.example/wabiabi => ./wabiabi\n", m.symbol_prefix().replace('_', "-")));
                }
            }
            _ => unreachable!(),
        }
    }
    files.extend(custom);
    files
}

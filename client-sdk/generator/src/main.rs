//! wabi-wit-gen — derives everything language-specific from a WIT contract.
//!
//!   wabi-wit-gen <wit file or dir> --out <dir> --target ts,dart --core rust   (standalone)
//!   wabi-wit-gen --root <package dir> [--check]                              (packages of this repo)
//!
//! Package mode:
//! Reads   wit/*.wit, wit/abi-ids.lock
//! Writes  wit/abi-ids.lock                 (append-only id allocation)
//!         core-rust/src/generated/mod.rs   (Rust core: types, codec, dispatch, prefixed exports)
//!         generated/c/<prefix>.h           (C view of the package's ABI symbols)
//!         generated/csharp/*.g.cs          (C# SDK surface + P/Invoke table)
//!         generated/java/**/*.java         (Java SDK surface + Panama FFM table)
//!         generated/swift/*.swift          (Swift SDK surface)
//!         generated/dart/*.dart            (Dart/Flutter SDK surface + dart:ffi bindings)
//!         generated/ts/*.ts                (TypeScript SDK surface for Bun + bun:ffi bindings)
//!         <csharp_core_out>/*.g.cs         (C# core: interfaces, dispatch, import clients)
//! With --check nothing is written; the exit code is non-zero if any output
//! (including the lock file) is out of date. Use it in CI.

macro_rules! w {
    ($o:expr, $($arg:tt)*) => { { use std::fmt::Write as _; writeln!($o, $($arg)*).unwrap(); } };
}

mod abi_table;
mod api;
mod gen_csharp;
mod gen_csharp_core;
mod gen_dart;
mod gen_go;
mod gen_java;
mod gen_java_core;
mod gen_rust;
mod gen_swift;
mod gen_ts;
mod ir;
mod lock;
mod names;
mod packaging;
mod runtime;

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// `wabi-gen.conf` next to `wit/`: which targets to generate and where.
///
///   abi      = abi                              (dir with wabi_abi.h, relative)
///   targets  = c csharp java swift dart ts rust go
///   rust_out = core-rust/src/generated/mod.rs   (Rust core glue)
///   go_out   = alt-cores/go/wabi_generated.go   (Go core glue)
///   java_core_out = core-java/src/generated/java  (Java core glue, GraalVM native-image)
///   csharp_core_out = alt-cores/csharp/generated  (C# core glue, NativeAOT)
///   dart_prebuilt_dir = ../../build/native       (Dart build hook: default prebuilt libraries,
///                                                  relative to the Dart package root)
///   dart_release_url  = https://.../releases/v1  (Dart build hook: default download base)
struct Config {
    abi: PathBuf,
    targets: Vec<String>,
    rust_out: Option<PathBuf>,
    go_out: Option<PathBuf>,
    java_core_out: Option<PathBuf>,
    csharp_core_out: Option<PathBuf>,
    dart: gen_dart::HookDefaults,
}

impl Config {
    fn load(root: &Path) -> Result<Config> {
        let path = root.join("wabi-gen.conf");
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut c = Config { abi: "abi".into(), targets: vec![], rust_out: None, go_out: None, java_core_out: None, csharp_core_out: None, dart: Default::default() };
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap().trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line.split_once('=').with_context(|| format!("{}:{}: expected key = value", path.display(), n + 1))?;
            let v = v.trim();
            match k.trim() {
                "abi" => c.abi = v.into(),
                "targets" => c.targets = v.split_whitespace().map(String::from).collect(),
                "rust_out" => c.rust_out = Some(v.into()),
                "go_out" => c.go_out = Some(v.into()),
                "java_core_out" => c.java_core_out = Some(v.into()),
                "csharp_core_out" => c.csharp_core_out = Some(v.into()),
                "dart_prebuilt_dir" => c.dart.prebuilt_dir = Some(v.into()),
                "dart_release_url" => c.dart.release_url = Some(v.into()),
                other => bail!("{}:{}: unknown key `{other}`", path.display(), n + 1),
            }
        }
        Ok(c)
    }

    fn has(&self, t: &str) -> bool {
        self.targets.iter().any(|x| x == t)
    }
}

const USAGE: &str = "\
usage:
  wabi-wit-gen <wit file or dir> --out <dir> [--target <langs>] [--core <lang>] [options]
      standalone: everything a project needs, in <out>/
        --target  SDK languages, comma-separated: csharp,java,swift,dart,ts (default: none)
        --core    core language: rust | go | csharp | java | none (default: none)
        --lock    wire-id lock file (default: abi-ids.lock next to the WIT)
        --api-baseline <file>   check against a released API (optional)
  wabi-wit-gen --root <package dir>
      package mode: reads <dir>/wabi-gen.conf and <dir>/wit/, writes <dir>/generated/ etc.
        --api-baseline <path>   default wit/api-baseline.snapshot (required)
common options:
  --check                 write nothing; fail if any output (incl. the lock) is out of date
  --write-api-baseline    (re)write the API baseline instead of checking against it
  --version, --help";

/// Where the outputs go: a package of this repository (`--root`, `wabi-gen.conf`)
/// or a standalone output directory (`<wit> --out`).
enum Mode {
    Package { root: PathBuf },
    Standalone { wit: PathBuf, out: PathBuf, targets: Vec<String>, core: Option<String>, lock: Option<PathBuf> },
}

const SDK_LANGS: [&str; 5] = ["csharp", "java", "swift", "dart", "ts"];
const CORE_LANGS: [&str; 4] = ["rust", "go", "csharp", "java"];

fn main() -> Result<()> {
    let mut root: Option<PathBuf> = None;
    let mut wit: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut targets: Vec<String> = vec![];
    let mut core: Option<String> = None;
    let mut lock_arg: Option<PathBuf> = None;
    let mut check = false;
    let mut api_baseline: Option<PathBuf> = None;
    let mut write_api_baseline = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().with_context(|| format!("{name} needs a value"));
        match a.as_str() {
            "--root" => root = Some(PathBuf::from(value("--root")?)),
            "--out" | "-o" => out = Some(PathBuf::from(value("--out")?)),
            "--target" | "--targets" | "-t" => {
                targets.extend(value("--target")?.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
            }
            "--core" | "-c" => core = Some(value("--core")?),
            "--lock" => lock_arg = Some(PathBuf::from(value("--lock")?)),
            "--check" => check = true,
            "--api-baseline" => api_baseline = Some(PathBuf::from(value("--api-baseline")?)),
            "--write-api-baseline" => write_api_baseline = true,
            "-V" | "--version" => {
                println!("wabi-wit-gen {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other if other.starts_with('-') => bail!("unknown option {other}\n\n{USAGE}"),
            other => {
                if wit.is_some() {
                    bail!("more than one WIT path given ({other})\n\n{USAGE}");
                }
                wit = Some(PathBuf::from(other));
            }
        }
    }
    if check && write_api_baseline { bail!("--check cannot update a released API baseline"); }

    let mode = match (root, wit) {
        (Some(_), Some(_)) => bail!("use either --root <package dir> or <wit path>, not both"),
        (Some(root), None) => Mode::Package { root },
        (None, Some(wit)) => {
            let out = out.context("standalone mode needs --out <dir>")?;
            for t in &targets {
                if !SDK_LANGS.contains(&t.as_str()) {
                    bail!("unknown --target {t} (one of: {})", SDK_LANGS.join(", "));
                }
            }
            let core = match core.as_deref() {
                None | Some("none") => None,
                Some(c) if CORE_LANGS.contains(&c) => Some(c.to_string()),
                Some(c) => bail!("unknown --core {c} (one of: {}, none)", CORE_LANGS.join(", ")),
            };
            Mode::Standalone { wit, out, targets, core, lock: lock_arg }
        }
        (None, None) => {
            println!("{USAGE}");
            bail!("no WIT path and no --root given");
        }
    };

    // --- read the contract ---------------------------------------------------
    let (wit_path, lock_path, baseline_path) = match &mode {
        Mode::Package { root } => {
            let wit = root.join("wit");
            let lock = wit.join("abi-ids.lock");
            (wit, lock, Some(root.join(api_baseline.unwrap_or_else(|| "wit/api-baseline.snapshot".into()))))
        }
        Mode::Standalone { wit, lock, .. } => {
            let dir = if wit.is_dir() { wit.clone() } else { wit.parent().map(Path::to_path_buf).unwrap_or_default() };
            (wit.clone(), lock.clone().unwrap_or_else(|| dir.join("abi-ids.lock")), api_baseline)
        }
    };
    let lock_text = std::fs::read_to_string(&lock_path).unwrap_or_default();
    let mut lock = lock::Lock::parse(&lock_text)?;
    let mut resolve = wit_parser::Resolve::default();
    let (pkg, _) = resolve.push_path(&wit_path).with_context(|| format!("reading WIT from {}", wit_path.display()))?;
    let model = ir::build(&resolve, pkg, &mut lock)?;

    let snapshot = api::Snapshot::of(&model);
    let mut outputs: Vec<(PathBuf, String)> = vec![(lock_path, lock.render())];
    if let Some(baseline_path) = &baseline_path {
        if write_api_baseline {
            outputs.push((baseline_path.clone(), snapshot.render()));
        } else {
            let text = std::fs::read_to_string(baseline_path)
                .with_context(|| format!("reading released API baseline {}; bootstrap explicitly with --write-api-baseline", baseline_path.display()))?;
            snapshot.ensure_compatible(&api::Snapshot::parse(&text)?)?;
        }
    } else if write_api_baseline {
        bail!("--write-api-baseline needs --api-baseline <file> in standalone mode");
    }

    // --- derive everything ---------------------------------------------------
    let summary = match &mode {
        Mode::Package { root } => {
            package_outputs(root, &model, &mut outputs)?;
            None
        }
        Mode::Standalone { out, targets, core, .. } => Some(standalone_outputs(out, &model, targets, core.as_deref(), &mut outputs)?),
    };

    let mut stale = vec![];
    for (path, text) in &outputs {
        let current = std::fs::read_to_string(path).ok();
        if current.as_deref() == Some(text.as_str()) {
            continue;
        }
        if check {
            stale.push(path.display().to_string());
        } else {
            write(path, text)?;
            if summary.is_none() {
                println!("wrote {}", path.display());
            }
        }
    }
    if !stale.is_empty() {
        bail!("generated files are out of date (run wabi-wit-gen):\n  {}", stale.join("\n  "));
    }
    println!(
        "{}: {} types, {} resources, {} host interfaces — up to date",
        model.contract_id(),
        model.types.len(),
        model.resources.len(),
        model.imports.len()
    );
    if let Some(summary) = summary {
        if !check {
            print!("{summary}");
        }
    }
    Ok(())
}

/// Package mode (this repository): what `wabi-gen.conf` asks for, at the configured paths.
fn package_outputs(root: &Path, model: &ir::Model, outputs: &mut Vec<(PathBuf, String)>) -> Result<()> {
    let cfg = Config::load(root)?;
    abi_table::check_header(&root.join(&cfg.abi))?;
    if cfg.has("c") {
        outputs.push((root.join(format!("generated/c/{}.h", model.symbol_prefix())), abi_table::emit_c_header(model)));
    }
    if cfg.has("rust") {
        let out = cfg.rust_out.as_ref().context("target rust needs rust_out")?;
        outputs.push((root.join(out), gen_rust::emit(model)));
    }
    if cfg.has("go") {
        let out = cfg.go_out.as_ref().context("target go needs go_out")?;
        // cgo include path from the generated file's directory to the ABI headers
        let depth = out.parent().map(|p| p.components().count()).unwrap_or(0);
        let abi_rel = format!("{}{}", "../".repeat(depth), cfg.abi.display());
        outputs.push((root.join(out), gen_go::emit(model, &abi_rel)?));
    }
    if cfg.has("java-core") {
        let out = cfg.java_core_out.as_ref().context("target java-core needs java_core_out")?;
        for (name, text) in gen_java::emit_core(model)? {
            outputs.push((root.join(out).join(name), text));
        }
    }
    if cfg.has("csharp-core") {
        let out = cfg.csharp_core_out.as_ref().context("target csharp-core needs csharp_core_out")?;
        for (name, text) in gen_csharp_core::emit(model)? {
            outputs.push((root.join(out).join(name), text));
        }
    }
    for lang in SDK_LANGS {
        if cfg.has(lang) {
            sdk_outputs(lang, model, &cfg.dart, &root.join("generated").join(lang), &format!("../../../{}", cfg.abi.display()), outputs);
        }
    }
    Ok(())
}

/// The SDK of `lang` (generated code + its runtime in the package's namespace) into `dir`.
fn sdk_outputs(lang: &str, model: &ir::Model, dart: &gen_dart::HookDefaults, dir: &Path, abi_from_jni: &str, outputs: &mut Vec<(PathBuf, String)>) {
    let files = match lang {
        "csharp" => gen_csharp::emit(model),
        "java" => gen_java::emit(model),
        "swift" => gen_swift::emit(model),
        "dart" => gen_dart::emit(model, dart),
        "ts" => gen_ts::emit(model),
        other => unreachable!("no SDK emitter for {other}"),
    };
    for (name, text) in files {
        let (name, text) = runtime::localize(lang, model, name, text);
        outputs.push((dir.join(name), text));
    }
    // The runtime travels with the generated code, in the package's namespace.
    for (name, text) in runtime::emit(lang, model, abi_from_jni) {
        outputs.push((dir.join(name), text));
    }
}

/// Standalone mode: ABI headers, C header, one directory per SDK language and the
/// glue + runtime of the chosen core, all below `out`. Returns the summary for the user.
fn standalone_outputs(out: &Path, model: &ir::Model, targets: &[String], core: Option<&str>, outputs: &mut Vec<(PathBuf, String)>) -> Result<String> {
    use std::fmt::Write as _;
    let prefix = model.symbol_prefix();
    let mut targets: Vec<String> = targets.to_vec();
    let mut notes = String::new();
    // C# and Java cores reuse the contract types, codec and MessagePack of their SDK.
    if let Some(c @ ("csharp" | "java")) = core {
        if !targets.iter().any(|t| t == c) {
            targets.push(c.to_string());
            writeln!(notes, "  (--target {c} added: the {c} core reuses the contract types and MessagePack of the {c} SDK)").unwrap();
        }
    }
    for (name, text) in runtime::abi() {
        outputs.push((out.join("abi").join(name), text));
    }
    outputs.push((out.join("c").join(format!("{prefix}.h")), abi_table::emit_c_header(model)));
    for lang in SDK_LANGS {
        if !targets.iter().any(|t| t == lang) {
            continue;
        }
        if lang == "dart" {
            // A complete Dart package; its build hook takes the core from <out>/native.
            let pkg = out.join("dart");
            let gen = pkg.join("lib/src/generated");
            let hook = gen_dart::HookDefaults { prebuilt_dir: Some("../native".into()), release_url: None };
            let first = outputs.len();
            sdk_outputs(lang, model, &hook, &gen, "../../abi", outputs);
            let names: Vec<String> = outputs[first..]
                .iter()
                .filter_map(|(p, _)| p.strip_prefix(&gen).ok().map(|r| r.to_string_lossy().replace('\\', "/")))
                .collect();
            for (name, text) in packaging::dart_package(model, &names) {
                outputs.push((pkg.join(name), text));
            }
            continue;
        }
        sdk_outputs(lang, model, &Default::default(), &out.join(lang), "../../abi", outputs);
        if lang == "csharp" {
            let (name, text) = packaging::csharp_project(model);
            outputs.push((out.join("csharp").join(name), text));
        }
    }
    let swift_module = runtime::swift_c_module(model);
    if targets.iter().any(|t| t == "swift") {
        let dir = out.join("swift-c").join(&swift_module);
        outputs.push((dir.join("module.modulemap"), format!("module {swift_module} [system] {{\n    header \"shim.h\"\n    link \"{prefix}\"\n    export *\n}}\n")));
        outputs.push((
            dir.join("shim.h"),
            format!("// C view of the native ABI for the Swift SDK (generated by wabi-wit-gen — DO NOT EDIT).\n#include \"../../abi/wabi_abi.h\"\n#include \"../../c/{prefix}.h\"\n"),
        ));
    }
    match core {
        Some("rust") => {
            outputs.push((out.join("core-rust/generated.rs"), gen_rust::emit(model)));
            outputs.push((out.join("core-rust/build-native.sh"), packaging::build_script("rust", model).unwrap()));
            for (name, text) in runtime::core("rust") {
                outputs.push((out.join("core-rust/wabi-abi-rt").join(name), text));
            }
        }
        Some("go") => {
            // The header sits next to the glue, so both can be copied into the core's package together.
            outputs.push((out.join("core-go/wabi_generated.go"), gen_go::emit(model, ".")?));
            outputs.push((out.join("core-go/build-native.sh"), packaging::build_script("go", model).unwrap()));
            let header = runtime::abi().into_iter().find(|(n, _)| n == "wabi_abi.h").context("embedded wabi_abi.h")?;
            outputs.push((out.join("core-go/wabi_abi.h"), header.1));
            for (name, text) in runtime::core("go") {
                outputs.push((out.join("core-go/wabiabi").join(name), text));
            }
        }
        Some("csharp") => {
            for (name, text) in gen_csharp_core::emit(model)? {
                outputs.push((out.join("core-csharp/generated").join(name), text));
            }
            for (name, text) in runtime::core("csharp") {
                outputs.push((out.join("core-csharp/runtime").join(name), text));
            }
        }
        Some("java") => {
            for (name, text) in gen_java::emit_core(model)? {
                outputs.push((out.join("core-java/generated").join(name), text));
            }
            for (name, text) in runtime::core("java") {
                outputs.push((out.join("core-java/runtime").join(name), text));
            }
        }
        _ => {}
    }

    let o = out.display();
    let mut s = String::new();
    writeln!(s, "\nwritten to {o}/:").unwrap();
    writeln!(s, "  abi/            native ABI headers (wabi_abi.h, wabi_abi_functions.h, ABI.md)").unwrap();
    writeln!(s, "  c/{prefix}.h    this package's prefixed ABI symbols").unwrap();
    for t in &targets {
        let what = match t.as_str() {
            "csharp" => format!(
                "C# SDK incl. runtime (namespace {}); `dotnet pack {o}/csharp` -> NuGet package with the core from native/",
                runtime::csharp_namespace(model)
            ),
            "java" => format!("Java SDK incl. runtime ({}) and JNI shim (java/jni, lib{})", runtime::java_package(model), runtime::jni_library(model)),
            "swift" => format!("Swift SDK incl. runtime; C module {swift_module} in swift-c/"),
            "dart" => format!(
                "Dart/Flutter package {} incl. runtime; `flutter pub add {} --path {o}/dart` (core from native/ via native assets)",
                packaging::dart_package_name(model),
                packaging::dart_package_name(model)
            ),
            _ => "TypeScript SDK for Bun incl. runtime (bun:ffi)".into(),
        };
        writeln!(s, "  {t:<15} {what}").unwrap();
    }
    s.push_str(&notes);
    match core {
        Some("rust") => writeln!(s, "  core-rust/      generated.rs + crate wabi-abi-rt\n\nRust core: add `wabi-abi-rt = {{ path = \"{o}/core-rust/wabi-abi-rt\" }}` to Cargo.toml,\n  `#[path = \"{o}/core-rust/generated.rs\"] pub mod generated;` in lib.rs, crate-type = [\"cdylib\", \"staticlib\"],\n  `pub struct Core;` at the crate root with `impl generated::Implementation for Core` (the exports refer to crate::Core)\n  and one type per resource implementing its generated `…Resource` trait.\n  Then `bash {o}/core-rust/build-native.sh <crate dir> [host linux-x64 linux-arm64 windows-x64 macos ios android all]`\n  puts the libraries into {o}/native/, where the Dart package and the NuGet package pick them up.").unwrap(),
        Some("go") => writeln!(s, "  core-go/        wabi_generated.go + wabi_abi.h + module wabi.example/wabiabi\n\nGo core: copy wabi_generated.go and wabi_abi.h into your main package; in go.mod:\n  require wabi.example/wabiabi v0.0.0\n  replace wabi.example/wabiabi => {o}/core-go/wabiabi\n  then `bash {o}/core-go/build-native.sh <package dir> [host linux-x64 linux-arm64 windows-x64 macos ios android all]`\n  puts the libraries into {o}/native/, where the Dart package and the NuGet package pick them up.").unwrap(),
        Some("csharp") => writeln!(s, "  core-csharp/    generated glue + C# core runtime (NativeAOT)\n\nC# core: compile core-csharp/generated/*.cs, core-csharp/runtime/*.cs, csharp/Types.g.cs, csharp/WireCodec.g.cs,\n  csharp/Runtime/MsgPack.cs and csharp/Runtime/Exceptions.cs, add `global using {};`,\n  publish with NativeAOT as {prefix}.", runtime::csharp_namespace(model)).unwrap(),
        Some("java") => writeln!(s, "  core-java/      generated glue + Java core runtime (GraalVM native-image + C shim)").unwrap(),
        _ => {}
    }
    writeln!(s, "\nKeep {} under version control: it pins every wire id (append-only).", "abi-ids.lock").unwrap();
    Ok(s)
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIT: &str = include_str!("../../wit/client.wit");

    fn build(src: &str, lock_text: &str) -> (ir::Model, String) {
        let mut lock = lock::Lock::parse(lock_text).unwrap();
        let mut resolve = wit_parser::Resolve::default();
        let pkg = resolve.push_str("client.wit", src).unwrap();
        let model = ir::build(&resolve, pkg, &mut lock).unwrap();
        (model, lock.render())
    }

    fn ids(m: &ir::Model) -> Vec<(String, u32)> {
        let mut v: Vec<_> = m.resources[0].ctor.iter().chain(&m.resources[0].methods).map(|f| (f.name.clone(), f.id)).collect();
        v.sort();
        v
    }

    /// Moves `line` (exact text) to directly after the line containing `after`.
    fn move_line(src: &str, line: &str, after: &str) -> String {
        let without: Vec<&str> = src.lines().filter(|l| l.trim() != line).collect();
        let mut out = vec![];
        for l in without {
            out.push(l.to_string());
            if l.contains(after) {
                out.push(format!("        {line}"));
            }
        }
        out.join("\n")
    }

    #[test]
    fn committed_api_baselines_are_canonical_and_compatible() {
        for (wit, lock_text, baseline) in [
            (WIT, include_str!("../../wit/abi-ids.lock"), include_str!("../../wit/api-baseline.snapshot")),
            (include_str!("../../packages/pki/wit/pki.wit"), include_str!("../../packages/pki/wit/abi-ids.lock"), include_str!("../../packages/pki/wit/api-baseline.snapshot")),
            (include_str!("../../packages/zero/wit/zero.wit"), include_str!("../../packages/zero/wit/abi-ids.lock"), include_str!("../../packages/zero/wit/api-baseline.snapshot")),
        ] {
            let (m, _) = build(wit, lock_text);
            let released = api::Snapshot::parse(baseline).unwrap();
            assert_eq!(released.render(), baseline);
            api::Snapshot::of(&m).ensure_compatible(&released).unwrap();
        }
    }

    #[test]
    fn duplicate_resources_are_rejected_without_mixing_their_groups() {
        let src = "package test:collision@1.0.0;
            interface one { resource client { constructor(); ping: func(); } }
            interface two { resource client { constructor(); pong: func(); } }
            world api { export one; export two; }";
        let mut resolve = wit_parser::Resolve::default();
        let pkg = resolve.push_str("collision.wit", src).unwrap();
        let mut lock = lock::Lock::default();
        let error = ir::build(&resolve, pkg, &mut lock).unwrap_err().to_string();
        assert!(error.contains("collision"), "{error}");
        let text = lock.render();
        assert!(text.lines().any(|l| l.contains("one.client.ping") && l.contains("0x0101")));
        assert!(text.lines().any(|l| l.contains("two.client.pong") && l.contains("0x0201")));
    }

    #[test]
    fn duplicate_record_names_and_generated_support_names_are_rejected() {
        for src in [
            "package test:collision@1.0.0; interface one { record data { x: u32, } } interface two { record data { y: u32, } } world api {}",
            "package test:collision@1.0.0; interface types { record method-ids { x: u32, } } world api {}",
        ] {
            let mut resolve = wit_parser::Resolve::default();
            let pkg = resolve.push_str("collision.wit", src).unwrap();
            let error = ir::build(&resolve, pkg, &mut lock::Lock::default()).unwrap_err().to_string();
            assert!(error.contains("collision"), "{error}");
        }
    }

    #[test]
    fn committed_lock_is_up_to_date() {
        let committed = include_str!("../../wit/abi-ids.lock");
        let (_, rendered) = build(WIT, committed);
        assert_eq!(rendered, committed, "run wabi-wit-gen: abi-ids.lock is stale");
    }

    #[test]
    fn reordering_the_wit_never_changes_ids() {
        let committed = include_str!("../../wit/abi-ids.lock");
        let (original, _) = build(WIT, committed);
        // Move get-customer to the top of the resource and swap two variant cases.
        let reordered = move_line(WIT, "get-customer: async func(id: string) -> result<customer, client-error>;", "constructor(options: client-options);");
        let reordered = move_line(&reordered, "not-found,", "protocol-error(string),");
        assert_ne!(reordered, WIT);
        let (moved, rendered) = build(&reordered, committed);
        assert_eq!(ids(&original), ids(&moved));
        assert_eq!(rendered, committed, "reordering must not touch the lock file");
        let tag = |m: &ir::Model| match &m.types.iter().find(|t| t.name == "client-error").unwrap().kind {
            ir::TypeKind::Variant(cases) => cases.iter().find(|c| c.name == "not-found").unwrap().tag,
            _ => unreachable!(),
        };
        assert_eq!(tag(&original), tag(&moved));
    }

    #[test]
    fn new_functions_are_appended_and_removed_ids_are_never_reused() {
        let committed = include_str!("../../wit/abi-ids.lock");
        let (original, _) = build(WIT, committed);
        let max = ids(&original).iter().map(|(_, id)| *id).max().unwrap();

        // Add a function *in the middle*: it gets max+1, nothing else moves.
        let added = WIT.replacen("state: func() -> connection-state;", "state: func() -> connection-state;\n        ping: async func() -> result<u32, client-error>;", 1);
        let (m2, lock2) = build(&added, committed);
        let ping = ids(&m2).into_iter().find(|(n, _)| n == "ping").unwrap().1;
        assert_eq!(ping, max + 1);
        assert!(ids(&original).iter().all(|e| ids(&m2).contains(e)));

        // Remove it again: it is retired, and the next new function does not reuse its id.
        let (_, lock3) = build(WIT, &lock2);
        assert!(lock3.lines().any(|l| l.contains("client.ping") && l.ends_with("retired")));
        let other = WIT.replacen("state: func() -> connection-state;", "state: func() -> connection-state;\n        pong: func() -> u32;", 1);
        let (m4, _) = build(&other, &lock3);
        assert_eq!(ids(&m4).into_iter().find(|(n, _)| n == "pong").unwrap().1, max + 2);
    }
}

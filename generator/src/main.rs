//! wabi-wit-gen — derives everything language-specific from a WIT contract.
//!
//!   wabi-wit-gen <wit file or dir> --out <dir> --target ts,dart --core rust,go=core-go
//!
//! Reads   the WIT package and `abi-ids.lock` next to it
//! Writes  the lock file (append-only id allocation) and, below `<out>`:
//!         abi/, c/             the ABI headers and the package's prefixed symbols
//!         csharp/              C# SDK + runtime, NuGet project
//!         java/                Java SDK + runtime, JNI shim, build.sh
//!         swift/<Module>/      SwiftPM package
//!         dart/                Dart/Flutter package with build hook
//!         ts/                  TypeScript package for Bun
//!         core-<lang>/         core glue + core runtime (+ build-native.sh)
//! With --check nothing is written; the exit code is non-zero if any output
//! (including the lock file) is out of date. Use it in CI.

macro_rules! w {
    ($o:expr, $($arg:tt)*) => { { use std::fmt::Write as _; writeln!($o, $($arg)*).unwrap(); } };
}

mod abi_table;
mod config;
mod scaffold;
mod api;
mod gen_csharp;
mod gen_csharp_core;
mod gen_dart;
mod gen_go;
mod gen_java;
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

const USAGE: &str = "\
usage:
  wabi-wit-gen <wit file or dir> --out <dir> [--target <langs>] [--core <langs>] [options]
      everything a project needs, in <out>/
        --target  SDK languages, comma-separated: csharp,java,swift,dart,ts (default: none)
        --core    core languages, comma-separated: rust | go | csharp (default: none).
                  go=<dir>: write the Go glue (wabi_generated.go, wabi_abi.h), which must sit
                  in the core's own package, into <dir> instead of <out>/core-go
        --lock    wire-id lock file (default: abi-ids.lock next to the WIT)
        --api-baseline <file>   check against a released API (optional)
        --write-api-baseline    (re)write that baseline instead of checking against it
  --config <file>         project config (default: ./wabi-gen.conf when no WIT is given)
  --scaffold examples     create editable example apps, core skeletons and project config once
  --check                 write nothing; fail if any output (incl. the lock) is out of date
  --version, --help";

const SDK_LANGS: [&str; 5] = ["csharp", "java", "swift", "dart", "ts"];
const CORE_LANGS: [&str; 3] = ["rust", "go", "csharp"];

/// One `--core` entry: the language and, for Go, where its glue goes.
#[derive(Clone)]
struct Core {
    lang: String,
    dir: Option<PathBuf>,
}

fn main() -> Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let cwd = if let Some(i) = raw.iter().position(|a| a == "--working-dir") {
        PathBuf::from(raw.get(i + 1).context("--working-dir needs a value")?)
    } else { std::env::current_dir()? };
    let mut explicit_wit = false;
    let mut scan = raw.iter();
    while let Some(a) = scan.next() {
        if ["--out", "-o", "--target", "--targets", "-t", "--core", "--cores", "-c", "--lock", "--api-baseline", "--config", "--working-dir", "--scaffold"].contains(&a.as_str()) { scan.next(); }
        else if !a.starts_with('-') { explicit_wit = true; }
    }
    let config_arg = raw.iter().position(|a| a == "--config").map(|i| raw.get(i + 1).context("--config needs a value")).transpose()?;
    let config_path = config_arg.map(|p| cwd.join(p)).unwrap_or_else(|| cwd.join("wabi-gen.conf"));
    let help = raw.iter().any(|a| ["--help", "-h", "--version", "-V"].contains(&a.as_str()));
    let creating_config = raw.windows(2).any(|w| w == ["--scaffold", "examples"]) && !config_path.exists();
    let saved = if !help && !creating_config && (config_arg.is_some() || (!explicit_wit && config_path.exists())) { config::read(&config_path)? } else { config::Config::default() };
    let mut wit = saved.wit;
    let mut out = saved.out;
    let mut targets = saved.targets;
    let mut cores = saved.cores;
    let mut lock_arg = saved.lock;
    let mut check = false;
    let mut api_baseline = saved.baseline;
    let mut write_api_baseline = false;
    let mut scaffold_examples = false;
    let mut cli_wit = false;
    let mut cli_targets = false;
    let mut cli_cores = false;
    let mut args = raw.into_iter();
    while let Some(a) = args.next() {
        let mut value = |name: &str| args.next().with_context(|| format!("{name} needs a value"));
        match a.as_str() {
            "--config" | "--working-dir" => { value(&a)?; }
            "--scaffold" => {
                let mode = value("--scaffold")?;
                if mode != "examples" { bail!("unknown scaffold {mode} (expected examples)"); }
                scaffold_examples = true;
            }
            "--out" | "-o" => out = Some(PathBuf::from(value("--out")?)),
            "--target" | "--targets" | "-t" => {
                if !cli_targets { targets.clear(); cli_targets = true; }
                targets.extend(value("--target")?.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
            }
            "--core" | "--cores" | "-c" => {
                if !cli_cores { cores.clear(); cli_cores = true; }
                for entry in value("--core")?.split(',').map(str::trim).filter(|c| !c.is_empty() && *c != "none") {
                    let (lang, dir) = match entry.split_once('=') {
                        Some((lang, dir)) => (lang, Some(PathBuf::from(dir))),
                        None => (entry, None),
                    };
                    if !CORE_LANGS.contains(&lang) {
                        bail!("unknown --core {lang} (one of: {}, none)", CORE_LANGS.join(", "));
                    }
                    if dir.is_some() && lang != "go" {
                        bail!("--core {lang}=<dir>: only the Go glue has to sit in the core's own directory");
                    }
                    cores.push(Core { lang: lang.to_string(), dir });
                }
            }
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
                if cli_wit {
                    bail!("more than one WIT path given ({other})\n\n{USAGE}");
                }
                cli_wit = true;
                wit = Some(PathBuf::from(other));
            }
        }
    }
    if check && write_api_baseline {
        bail!("--check cannot update a released API baseline");
    }
    let Some(wit) = wit else {
        println!("{USAGE}");
        bail!("no WIT path given");
    };
    let out = out.context("--out <dir> is required")?;
    for t in &targets {
        if !SDK_LANGS.contains(&t.as_str()) {
            bail!("unknown --target {t} (one of: {})", SDK_LANGS.join(", "));
        }
    }

    for core in &cores {
        if !CORE_LANGS.contains(&core.lang.as_str()) || (core.dir.is_some() && core.lang != "go") {
            bail!("invalid core configuration: {}", core.lang);
        }
    }

    // --- read the contract ---------------------------------------------------
    let dir = if wit.is_dir() { wit.clone() } else { wit.parent().map(Path::to_path_buf).unwrap_or_default() };
    let lock_path = lock_arg.unwrap_or_else(|| dir.join("abi-ids.lock"));
    let lock_text = std::fs::read_to_string(&lock_path).unwrap_or_default();
    let mut lock = lock::Lock::parse(&lock_text)?;
    let mut resolve = wit_parser::Resolve::default();
    let (pkg, _) = resolve.push_path(&wit).with_context(|| format!("reading WIT from {}", wit.display()))?;
    let model = ir::build(&resolve, pkg, &mut lock)?;

    let snapshot = api::Snapshot::of(&model);
    let mut outputs: Vec<(PathBuf, String)> = vec![(lock_path, lock.render())];
    if let Some(baseline_path) = &api_baseline {
        if write_api_baseline {
            outputs.push((baseline_path.clone(), snapshot.render()));
        } else {
            let text = std::fs::read_to_string(baseline_path)
                .with_context(|| format!("reading released API baseline {}; bootstrap explicitly with --write-api-baseline", baseline_path.display()))?;
            snapshot.ensure_compatible(&api::Snapshot::parse(&text)?)?;
        }
    } else if write_api_baseline {
        bail!("--write-api-baseline needs --api-baseline <file>");
    }

    // --- derive everything ---------------------------------------------------
    let summary = sdk_outputs_all(&out, &model, &targets, &cores, &mut outputs)?;

    let starters = if scaffold_examples && !check {
        let settings = config::Config { wit: Some(wit.clone()), out: Some(out.clone()), targets: targets.clone(), cores: cores.clone(), lock: Some(outputs[0].0.clone()), baseline: api_baseline.clone() };
        let mut files = scaffold::files(&out, &model, &targets, &cores);
        files.push((config_path.clone(), config::render(&settings, &config_path, &cwd)?));
        files
    } else { vec![] };

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
    if !check {
        if scaffold_examples {
            for (path, text) in starters {
                create_starter(&path, &text)?;
            }
            println!("Starter files: created only when absent; existing files preserved. Config: {}", config_path.display());
            println!("Core project setup is included in the starters. Implement their TODO methods, then follow {}/README.md to build and run the examples.", out.display());
        }
        print!("{summary}");
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

/// File names (relative, with `/`) of what `sdk_outputs` wrote below `dir` since `first`.
fn names_below(outputs: &[(PathBuf, String)], first: usize, dir: &Path) -> Vec<String> {
    outputs[first..].iter().filter_map(|(p, _)| p.strip_prefix(dir).ok().map(|r| r.to_string_lossy().replace('\\', "/"))).collect()
}

/// ABI headers, C header, one package per SDK language and the glue + runtime
/// of every chosen core, all below `out`. Returns the summary for the user.
fn sdk_outputs_all(out: &Path, model: &ir::Model, targets: &[String], cores: &[Core], outputs: &mut Vec<(PathBuf, String)>) -> Result<String> {
    use std::fmt::Write as _;
    let prefix = model.symbol_prefix();
    let mut targets: Vec<String> = targets.to_vec();
    let mut notes = String::new();
    // A C# core reuses the contract types, codec and MessagePack of the C# SDK.
    if cores.iter().any(|c| c.lang == "csharp") && !targets.iter().any(|t| t == "csharp") {
        targets.push("csharp".into());
        writeln!(notes, "  (--target csharp added: the C# core reuses the contract types and MessagePack of the C# SDK)").unwrap();
    }
    let abi = runtime::abi();
    for (name, text) in &abi {
        outputs.push((out.join("abi").join(name), text.clone()));
    }
    let c_header = abi_table::emit_c_header(model);
    outputs.push((out.join("c").join(format!("{prefix}.h")), c_header.clone()));
    let hook = gen_dart::HookDefaults { prebuilt_dir: Some("../native".into()), release_url: None };
    for lang in SDK_LANGS {
        if !targets.iter().any(|t| t == lang) {
            continue;
        }
        match lang {
            "dart" => {
                // A complete Dart package; its build hook takes the core from <out>/native.
                let pkg = out.join("dart");
                let gen = pkg.join("lib/src/generated");
                let first = outputs.len();
                sdk_outputs(lang, model, &hook, &gen, "../../abi", outputs);
                for (name, text) in packaging::dart_package(model, &names_below(outputs, first, &gen)) {
                    outputs.push((pkg.join(name), text));
                }
            }
            "swift" => {
                // A SwiftPM package: the SDK module and the C module it imports,
                // with its own copy of the headers so that the package can be moved.
                // In a directory named like the module: SwiftPM derives the package identity from it.
                let module = packaging::swift_module(model);
                let pkg = out.join("swift").join(&module);
                let c_module = runtime::swift_c_module(model);
                sdk_outputs(lang, model, &hook, &pkg.join("Sources").join(&module), "../../abi", outputs);
                let c_dir = pkg.join("Sources").join(&c_module);
                for (name, text) in abi.iter().filter(|(n, _)| n.ends_with(".h")) {
                    outputs.push((c_dir.join(name), text.clone()));
                }
                outputs.push((c_dir.join(format!("{prefix}.h")), c_header.clone()));
                for (name, text) in packaging::swift_package(model) {
                    outputs.push((pkg.join(name), text));
                }
            }
            "ts" => {
                let pkg = out.join("ts");
                let first = outputs.len();
                sdk_outputs(lang, model, &hook, &pkg, "../../abi", outputs);
                for (name, text) in packaging::ts_package(model, &names_below(outputs, first, &pkg)) {
                    outputs.push((pkg.join(name), text));
                }
            }
            _ => {
                sdk_outputs(lang, model, &hook, &out.join(lang), "../../abi", outputs);
                if lang == "csharp" {
                    let (name, text) = packaging::csharp_project(model);
                    outputs.push((out.join("csharp").join(name), text));
                } else {
                    outputs.push((out.join("java/build.sh"), packaging::java_build_script(model)));
                }
            }
        }
    }
    for core in cores {
        match core.lang.as_str() {
            "rust" => {
                outputs.push((out.join("core-rust/generated.rs"), gen_rust::emit(model)));
                outputs.push((out.join("core-rust/build-native.sh"), packaging::build_script("rust", model).unwrap()));
                for (name, text) in runtime::core("rust") {
                    outputs.push((out.join("core-rust/wabi-abi-rt").join(name), text));
                }
            }
            "go" => {
                // The header sits next to the glue: both belong into the core's own package.
                let glue = core.dir.clone().unwrap_or_else(|| out.join("core-go"));
                outputs.push((glue.join("wabi_generated.go"), gen_go::emit(model, ".")?));
                let header = abi.iter().find(|(n, _)| n == "wabi_abi.h").context("embedded wabi_abi.h")?;
                outputs.push((glue.join("wabi_abi.h"), header.1.clone()));
                outputs.push((out.join("core-go/build-native.sh"), packaging::build_script("go", model).unwrap()));
                for (name, text) in runtime::core("go") {
                    outputs.push((out.join("core-go/wabiabi").join(name), text));
                }
            }
            _ => {
                for (name, text) in gen_csharp_core::emit(model)? {
                    outputs.push((out.join("core-csharp/generated").join(name), text));
                }
                for (name, text) in runtime::core("csharp") {
                    outputs.push((out.join("core-csharp/runtime").join(name), text));
                }
            }
        }
    }

    outputs.push((out.join("README.md"), packaging::sdk_readme(model, &targets, cores)));

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
            "java" => format!(
                "Java SDK incl. runtime ({}) and JNI shim (lib{}); `bash {o}/java/build.sh` -> jars",
                runtime::java_package(model),
                runtime::jni_library(model)
            ),
            "swift" => format!(
                "SwiftPM package swift/{0} (C module {1}); `.package(path: \"{o}/swift/{0}\")`",
                packaging::swift_module(model),
                runtime::swift_c_module(model)
            ),
            "dart" => format!(
                "Dart/Flutter package {} incl. runtime; `flutter pub add {} --path {o}/dart` (core from native/ via native assets)",
                packaging::dart_package_name(model),
                packaging::dart_package_name(model)
            ),
            _ => format!("TypeScript package {} for Bun incl. runtime (bun:ffi)", packaging::ts_package_name(model)),
        };
        writeln!(s, "  {t:<15} {what}").unwrap();
    }
    s.push_str(&notes);
    for core in cores {
        match core.lang.as_str() {
            "rust" => writeln!(s, "  core-rust/      generated.rs + crate wabi-abi-rt\n\nRust core: add `wabi-abi-rt = {{ path = \"{o}/core-rust/wabi-abi-rt\" }}` to Cargo.toml,\n  `#[path = \"{o}/core-rust/generated.rs\"] pub mod generated;` in lib.rs, crate-type = [\"cdylib\", \"staticlib\"],\n  `pub struct Core;` at the crate root with `impl generated::Implementation for Core` (the exports refer to crate::Core)\n  and one type per resource implementing its generated `…Resource` trait.\n  Then `bash {o}/core-rust/build-native.sh <crate dir> [host linux-x64 linux-arm64 windows-x64 macos ios android all]`\n  puts the libraries into {o}/native/, where the Dart package and the NuGet package pick them up.").unwrap(),
            "go" => {
                let glue = core.dir.as_ref().map(|d| d.display().to_string()).unwrap_or_else(|| format!("{o}/core-go"));
                writeln!(s, "  core-go/        module wabi.example/wabiabi + build-native.sh; glue (wabi_generated.go, wabi_abi.h) in {glue}/\n\nGo core: the glue belongs into your main package (write it there with --core go=<dir>); in go.mod:\n  require wabi.example/wabiabi v0.0.0\n  replace wabi.example/wabiabi => {o}/core-go/wabiabi\n  then `bash {o}/core-go/build-native.sh <package dir> [host linux-x64 linux-arm64 windows-x64 macos ios android all]`\n  puts the libraries into {o}/native/, where the Dart package and the NuGet package pick them up.").unwrap()
            }
            _ => writeln!(s, "  core-csharp/    generated glue + C# core runtime (NativeAOT)\n\nC# core: compile core-csharp/generated/*.cs, core-csharp/runtime/*.cs, csharp/Types.g.cs, csharp/WireCodec.g.cs,\n  csharp/Runtime/MsgPack.cs and csharp/Runtime/Exceptions.cs, add `global using {};`,\n  publish with NativeAOT as {prefix}.", runtime::csharp_namespace(model)).unwrap(),
        }
    }
    writeln!(s, "\nSee {o}/README.md for package-specific build and usage instructions.").unwrap();
    writeln!(s, "\nKeep {} under version control: it pins every wire id (append-only).", "abi-ids.lock").unwrap();
    Ok(s)
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// Preserve existing files, including dangling symlinks (WASI may follow them
/// even with create_new). Exclusive creation also protects existing regular files.
fn create_starter(path: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    match std::fs::symlink_metadata(path) {
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e).with_context(|| format!("inspecting starter {}", path.display())),
    }
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => file.write_all(text.as_bytes()).with_context(|| format!("writing starter {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).with_context(|| format!("creating starter {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIT: &str = include_str!("../../example/wit/client.wit");

    fn build(src: &str, lock_text: &str) -> (ir::Model, String) {
        let mut lock = lock::Lock::parse(lock_text).unwrap();
        let mut resolve = wit_parser::Resolve::default();
        let pkg = resolve.push_str("client.wit", src).unwrap();
        let model = ir::build(&resolve, pkg, &mut lock).unwrap();
        (model, lock.render())
    }

    fn ids(m: &ir::Model) -> Vec<(String, u32)> {
        let client = m.resource("client");
        let mut v: Vec<_> = client.ctor.iter().chain(&client.methods).map(|f| (f.name.clone(), f.id)).collect();
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

    /// The embedded headers (abi/) match the table every binding is derived from.
    #[test]
    fn independent_packages_have_local_names_and_core_specific_guides() {
        for (namespace, package) in [("alpha", "archive-reader"), ("beta", "volume-tools")] {
            let src = format!("package {namespace}:{package}@1.0.0; interface api {{ resource reader {{ constructor(); ping: func() -> u32; }} }} world w {{ export api; }}");
            let (m, lock) = build(&src, "");
            let (_, again) = build(&src, &lock);
            assert_eq!(lock, again);
            for core in [None, Some("rust"), Some("go"), Some("csharp")] {
                let cores: Vec<Core> = core.map(|lang| Core { lang: lang.into(), dir: None }).into_iter().collect();
                let targets = SDK_LANGS.iter().map(|s| s.to_string()).collect::<Vec<_>>();
                let mut files = vec![];
                sdk_outputs_all(Path::new("sdk"), &m, &targets, &cores, &mut files).unwrap();
                let mut repeat = vec![];
                sdk_outputs_all(Path::new("sdk"), &m, &targets, &cores, &mut repeat).unwrap();
                assert_eq!(files, repeat);
                for (path, text) in &files {
                    for stale in ["wabi_client", "Wabi.Client", "com.wabi.client", "@LIBRARY@", "@JNI_CLASS@", "docs/COMPONENT_MODEL.md", "../example/"] {
                        assert!(!text.contains(stale), "{} contains {stale}", path.display());
                    }
                }
                let readme = &files.iter().find(|(p,_)| p == Path::new("sdk/README.md")).unwrap().1;
                for lang in ["rust", "go"] {
                    let script = format!("core-{lang}/build-native.sh");
                    assert_eq!(readme.contains(&script), core == Some(lang));
                    assert_eq!(files.iter().any(|(p,_)| p == &Path::new("sdk").join(&script)), core == Some(lang));
                }
                if core == Some("csharp") { assert!(readme.contains("dotnet publish")); }
                let java = runtime::java_package(&m).replace('.', "/");
                assert!(files.iter().any(|(p,t)| p == &Path::new("sdk/java").join(&java).join("AbiConstants.java") && t.contains(&format!("LIBRARY = \"{}\"", m.symbol_prefix()))));
                let jni_class = format!("Java_{}_JniAbi_", runtime::java_package(&m).replace('_', "_1").replace('.', "_"));
                let shim = &files.iter().find(|(p,_)| p == &Path::new("sdk/java/jni").join(format!("{}_jni.c", m.symbol_prefix()))).unwrap().1;
                assert!(shim.contains(&jni_class), "JNI symbol must escape package underscores");
                let header = &files.iter().find(|(p,_)| p == &Path::new("sdk/c").join(format!("{}.h", m.symbol_prefix()))).unwrap().1;
                assert!(header.contains(&format!("#define {}_SYMBOL_PREFIX \"{}_\"", m.symbol_prefix().to_ascii_uppercase(), m.symbol_prefix())));
            }
        }
    }

    #[test]
    fn starter_files_and_project_settings_are_user_owned() {
        let dir = std::env::temp_dir().join(format!("wabi-starter-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let path = dir.join("existing.txt");
        create_starter(&path, "initial").unwrap();
        std::fs::write(&path, [0xff, 0x00]).unwrap();
        create_starter(&path, "replacement").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), [0xff, 0x00]);
        let config_path = dir.join("wabi-gen.conf");
        let config = config::Config { wit: Some(dir.join("wit")), out: Some(dir.join("sdk")), targets: vec!["ts".into()], cores: vec![Core { lang: "go".into(), dir: Some(dir.join("core with spaces")) }], ..Default::default() };
        let text = config::render(&config, &config_path, &dir).unwrap();
        assert!(text.contains("wit = wit\nout = sdk"));
        std::fs::write(&config_path, text).unwrap();
        let saved = config::read(&config_path).unwrap();
        assert_eq!(saved.wit, config.wit);
        assert_eq!(saved.out, config.out);
        assert_eq!(saved.cores[0].dir, config.cores[0].dir);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn abi_headers_match_the_table() {
        let abi = runtime::abi();
        let text = |name: &str| abi.iter().find(|(n, _)| n == name).unwrap().1.clone();
        abi_table::check_header(&text("wabi_abi.h"), &text("wabi_abi_functions.h")).unwrap();
    }

    #[test]
    fn committed_api_baselines_are_canonical_and_compatible() {
        for (wit, lock_text, baseline) in [
            (WIT, include_str!("../../example/wit/abi-ids.lock"), include_str!("../../example/wit/api-baseline.snapshot")),
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
    fn resource_handles_are_values_only_where_their_ownership_is_clear() {
        let check = |body: &str| {
            let src = format!("package test:handles@1.0.0; interface api {{ {body} }} world w {{ export api; }}");
            let mut resolve = wit_parser::Resolve::default();
            let pkg = resolve.push_str("handles.wit", &src).unwrap();
            ir::build(&resolve, pkg, &mut lock::Lock::default()).map(|m| lock::Lock::default().render().len() + m.resources.len())
        };
        let pool = "resource pool { constructor(); }";
        // parameters (owned and borrowed) and results
        check(&format!("{pool} resource c {{ constructor(p: borrow<pool>); take: func(p: pool); make: func() -> pool; try-make: func() -> result<pool, string>; }}")).unwrap();
        for (body, why) in [
            (format!("{pool} resource c {{ constructor(); f: func(p: list<pool>); }}"), "whole parameter"),
            (format!("{pool} resource c {{ constructor(); f: func(p: option<borrow<pool>>); }}"), "whole parameter"),
            (format!("{pool} resource c {{ constructor(); f: func() -> list<pool>; }}"), "whole result"),
            (format!("{pool} record r {{ p: pool, }} resource c {{ constructor(); f: func(r: r); }}"), "inside records"),
        ] {
            let error = check(&body).unwrap_err().to_string();
            assert!(error.contains(why), "{body}: {error}");
        }
        // the shape names the kind of handle: changing it changes the id
        let mut l = lock::Lock::default();
        let mut resolve = wit_parser::Resolve::default();
        let pkg = resolve.push_str("h.wit", &format!("package test:handles@1.0.0; interface api {{ {pool} resource c {{ constructor(); f: func(p: borrow<pool>); g: func(p: pool) -> pool; }} }} world w {{ export api; }}")).unwrap();
        ir::build(&resolve, pkg, &mut l).unwrap();
        let text = l.render();
        assert!(text.contains("sync(borrow<pool>)->_") && text.contains("sync(own<pool>)->own<pool>"), "{text}");
    }

    #[test]
    fn committed_lock_is_up_to_date() {
        let committed = include_str!("../../example/wit/abi-ids.lock");
        let (_, rendered) = build(WIT, committed);
        assert_eq!(rendered, committed, "run wabi-wit-gen: abi-ids.lock is stale");
    }

    #[test]
    fn reordering_the_wit_never_changes_ids() {
        let committed = include_str!("../../example/wit/abi-ids.lock");
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
        let committed = include_str!("../../example/wit/abi-ids.lock");
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

    #[test]
    fn a_changed_signature_gets_a_new_id() {
        let committed = include_str!("../../example/wit/abi-ids.lock");
        let (original, _) = build(WIT, committed);
        let max = ids(&original).iter().map(|(_, id)| *id).max().unwrap();
        let id_of = |m: &ir::Model, name: &str| ids(m).into_iter().find(|(n, _)| n == name).unwrap().1;

        // get-customer takes a number instead of a string: old cores and SDKs
        // must not meet the new shape under the old id.
        let changed = WIT.replacen("get-customer: async func(id: string)", "get-customer: async func(id: u64)", 1);
        assert_ne!(changed, WIT);
        let (m2, lock2) = build(&changed, committed);
        assert_eq!(id_of(&m2, "get-customer"), max + 1);
        assert!(lock2.lines().any(|l| l.contains("client.get-customer") && l.contains("async(string)") && l.ends_with("retired")));
        // every other id is untouched
        assert!(ids(&original).iter().filter(|(n, _)| n != "get-customer").all(|e| ids(&m2).contains(e)));
        // ... and it is a breaking change of the released API
        let released = api::Snapshot::parse(include_str!("../../example/wit/api-baseline.snapshot")).unwrap();
        assert!(api::Snapshot::of(&m2).ensure_compatible(&released).is_err());

        // Renaming a parameter keeps the id (parameters travel by position).
        let renamed = WIT.replacen("get-customer: async func(id: string)", "get-customer: async func(customer-id: string)", 1);
        let (m3, lock3) = build(&renamed, committed);
        assert_eq!(id_of(&m3, "get-customer"), id_of(&original, "get-customer"));
        assert_eq!(lock3, committed);

        // Undoing the change brings the original id back.
        let (m4, _) = build(WIT, &lock2);
        assert_eq!(id_of(&m4, "get-customer"), id_of(&original, "get-customer"));
    }
}

//! acme-wit-gen — derives everything language-specific from `client.wit`.
//!
//!   acme-wit-gen --root <client-sdk dir> [--check]
//!
//! Reads   wit/*.wit, wit/abi-ids.lock
//! Writes  wit/abi-ids.lock                 (append-only id allocation)
//!         core-rust/src/generated/mod.rs   (Rust core: types, codec, dispatch, prefixed exports)
//!         generated/c/<prefix>.h           (C view of the package's ABI symbols)
//!         generated/csharp/*.g.cs          (C# SDK surface + P/Invoke table)
//!         generated/java/**/*.java         (Java SDK surface + Panama FFM table)
//!         generated/swift/*.swift          (Swift SDK surface)
//!         generated/dart/*.dart            (Dart/Flutter SDK surface + dart:ffi bindings)
//! With --check nothing is written; the exit code is non-zero if any output
//! (including the lock file) is out of date. Use it in CI.

macro_rules! w {
    ($o:expr, $($arg:tt)*) => { { use std::fmt::Write as _; writeln!($o, $($arg)*).unwrap(); } };
}

mod abi_table;
mod gen_csharp;
mod gen_dart;
mod gen_go;
mod gen_java;
mod gen_java_core;
mod gen_rust;
mod gen_swift;
mod ir;
mod lock;
mod names;

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// `acme-gen.conf` next to `wit/`: which targets to generate and where.
///
///   abi      = abi                              (dir with acme_abi.h, relative)
///   targets  = c csharp java swift dart rust go
///   rust_out = core-rust/src/generated/mod.rs   (Rust core glue)
///   go_out   = alt-cores/go/acme_generated.go   (Go core glue)
///   java_core_out = core-java/src/generated/java  (Java core glue, GraalVM native-image)
struct Config {
    abi: PathBuf,
    targets: Vec<String>,
    rust_out: Option<PathBuf>,
    go_out: Option<PathBuf>,
    java_core_out: Option<PathBuf>,
}

impl Config {
    fn load(root: &Path) -> Result<Config> {
        let path = root.join("acme-gen.conf");
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut c = Config { abi: "abi".into(), targets: vec![], rust_out: None, go_out: None, java_core_out: None };
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
                other => bail!("{}:{}: unknown key `{other}`", path.display(), n + 1),
            }
        }
        Ok(c)
    }

    fn has(&self, t: &str) -> bool {
        self.targets.iter().any(|x| x == t)
    }
}

fn main() -> Result<()> {
    let mut root = PathBuf::from(".");
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => root = PathBuf::from(args.next().context("--root needs a value")?),
            "--check" => check = true,
            "-h" | "--help" => {
                println!("usage: acme-wit-gen --root <package dir containing wit/ and acme-gen.conf> [--check]");
                return Ok(());
            }
            other => bail!("unknown argument {other}"),
        }
    }
    let cfg = Config::load(&root)?;

    let wit_dir = root.join("wit");
    let lock_path = wit_dir.join("abi-ids.lock");
    let lock_text = std::fs::read_to_string(&lock_path).unwrap_or_default();
    let mut lock = lock::Lock::parse(&lock_text)?;

    let mut resolve = wit_parser::Resolve::default();
    let (pkg, _) = resolve.push_dir(&wit_dir)?;
    let model = ir::build(&resolve, pkg, &mut lock)?;

    abi_table::check_header(&root.join(&cfg.abi))?;

    let mut outputs: Vec<(PathBuf, String)> = vec![(lock_path, lock.render())];
    if cfg.has("c") {
        outputs.push((root.join(format!("generated/c/{}.h", model.symbol_prefix())), abi_table::emit_c_header(&model)));
    }
    if cfg.has("rust") {
        let out = cfg.rust_out.as_ref().context("target rust needs rust_out")?;
        outputs.push((root.join(out), gen_rust::emit(&model)));
    }
    if cfg.has("go") {
        let out = cfg.go_out.as_ref().context("target go needs go_out")?;
        // cgo include path from the generated file's directory to the ABI headers
        let depth = out.parent().map(|p| p.components().count()).unwrap_or(0);
        let abi_rel = format!("{}{}", "../".repeat(depth), cfg.abi.display());
        outputs.push((root.join(out), gen_go::emit(&model, &abi_rel)?));
    }
    if cfg.has("java-core") {
        let out = cfg.java_core_out.as_ref().context("target java-core needs java_core_out")?;
        for (name, text) in gen_java::emit_core(&model)? {
            outputs.push((root.join(out).join(name), text));
        }
    }
    let langs: [(&str, fn(&ir::Model) -> Vec<(String, String)>); 4] =
        [("csharp", gen_csharp::emit), ("java", gen_java::emit), ("swift", gen_swift::emit), ("dart", gen_dart::emit)];
    for (lang, emit) in langs {
        if !cfg.has(lang) {
            continue;
        }
        for (name, text) in emit(&model) {
            outputs.push((root.join("generated").join(lang).join(name), text));
        }
    }

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
            println!("wrote {}", path.display());
        }
    }
    if !stale.is_empty() {
        bail!("generated files are out of date (run acme-wit-gen):\n  {}", stale.join("\n  "));
    }
    println!(
        "{}: {} types, {} resources, {} host interfaces — up to date",
        model.contract_id(),
        model.types.len(),
        model.resources.len(),
        model.imports.len()
    );
    Ok(())
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
    fn committed_lock_is_up_to_date() {
        let committed = include_str!("../../wit/abi-ids.lock");
        let (_, rendered) = build(WIT, committed);
        assert_eq!(rendered, committed, "run acme-wit-gen: abi-ids.lock is stale");
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

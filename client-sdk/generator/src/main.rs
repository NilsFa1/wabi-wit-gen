//! acme-wit-gen — derives everything language-specific from `client.wit`.
//!
//!   acme-wit-gen --root <client-sdk dir> [--check]
//!
//! Reads   wit/*.wit, wit/abi-ids.lock
//! Writes  wit/abi-ids.lock                 (append-only id allocation)
//!         core-rust/src/generated/mod.rs   (Rust core: types, codec, dispatch)
//!         generated/csharp/*.g.cs          (C# SDK surface + P/Invoke table)
//!         generated/java/**/*.java         (Java SDK surface + Panama FFM table)
//!         generated/swift/*.swift          (Swift SDK surface)
//! With --check nothing is written; the exit code is non-zero if any output
//! (including the lock file) is out of date. Use it in CI.

macro_rules! w {
    ($o:expr, $($arg:tt)*) => { { use std::fmt::Write as _; writeln!($o, $($arg)*).unwrap(); } };
}

mod abi_table;
mod gen_csharp;
mod gen_java;
mod gen_rust;
mod gen_swift;
mod ir;
mod lock;
mod names;

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    let mut root = PathBuf::from(".");
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--root" => root = PathBuf::from(args.next().context("--root needs a value")?),
            "--check" => check = true,
            "-h" | "--help" => {
                println!("usage: acme-wit-gen --root <client-sdk dir> [--check]");
                return Ok(());
            }
            other => bail!("unknown argument {other}"),
        }
    }

    let wit_dir = root.join("wit");
    let lock_path = wit_dir.join("abi-ids.lock");
    let lock_text = std::fs::read_to_string(&lock_path).unwrap_or_default();
    let mut lock = lock::Lock::parse(&lock_text)?;

    let mut resolve = wit_parser::Resolve::default();
    let (pkg, _) = resolve.push_dir(&wit_dir)?;
    let model = ir::build(&resolve, pkg, &mut lock)?;

    abi_table::check_header(&root.join("abi/acme_client.h"))?;

    let mut outputs: Vec<(PathBuf, String)> = vec![(lock_path, lock.render())];
    outputs.push((root.join("core-rust/src/generated/mod.rs"), gen_rust::emit(&model)));
    for (name, text) in gen_csharp::emit(&model) {
        outputs.push((root.join("generated/csharp").join(name), text));
    }
    for (name, text) in gen_java::emit(&model) {
        outputs.push((root.join("generated/java").join(name), text));
    }
    for (name, text) in gen_swift::emit(&model) {
        outputs.push((root.join("generated/swift").join(name), text));
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
    println!("{}: {} types, {} resources — up to date", model.contract_id(), model.types.len(), model.resources.len());
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

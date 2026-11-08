// Embeds everything a project needs besides the generated code, so that
// `wabi-wit-gen` (native or as WebAssembly in the npm package) is the only
// thing an SDK or core build needs:
//
//   app/<lang>/…   app-side runtimes (generator/runtime/<lang>), written next to the
//                  generated SDK code in the package's namespace (src/runtime.rs)
//   core/<lang>/…  core-side runtimes (../runtime/{rust,go,csharp-core}),
//                  written next to the generated core glue
//   abi/…          the ABI headers (../abi)
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

const APP: [&str; 5] = ["csharp", "java", "swift", "dart", "ts"];
const CORE: [(&str, &str); 3] = [("rust", "rust"), ("go", "go"), ("csharp", "csharp-core")];

fn skip(path: &Path) -> bool {
    let name = path.file_name().unwrap().to_string_lossy();
    name == "target" || name == "Cargo.lock" || name.ends_with("_test.go") || name.starts_with('.')
}

fn collect(dir: &Path, base: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if skip(&path) {
            continue;
        }
        if path.is_dir() {
            collect(&path, base, prefix, out);
        } else {
            let rel = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            println!("cargo:rerun-if-changed={}", path.display());
            out.push((format!("{prefix}{rel}"), path));
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let sdk = manifest.parent().unwrap().to_path_buf();
    let mut files = vec![];
    for lang in APP {
        let dir = manifest.join("runtime").join(lang);
        collect(&dir, &dir, &format!("app/{lang}/"), &mut files);
    }
    for (lang, dir) in CORE {
        let dir = sdk.join("runtime").join(dir);
        collect(&dir, &dir, &format!("core/{lang}/"), &mut files);
    }
    for name in ["wabi_abi.h", "wabi_abi_functions.h", "ABI.md"] {
        let path = sdk.join("abi").join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        files.push((format!("abi/{name}"), path));
    }
    let mut src = String::from("/// (embedded path, contents): app/<lang>/…, core/<lang>/…, abi/….\npub static FILES: &[(&str, &str)] = &[\n");
    for (rel, path) in &files {
        writeln!(src, "    ({rel:?}, include_str!({:?})),", path.display().to_string()).unwrap();
    }
    src.push_str("];\n");
    std::fs::write(PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("embedded_runtime.rs"), src).unwrap();
}

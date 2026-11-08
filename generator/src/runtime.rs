//! The hand-written app-side runtimes (generator/runtime/<lang>), embedded at
//! build time (build.rs) and written next to the generated code of every
//! package, in that package's own namespace:
//!
//! | target | written to                       | namespace / name                    |
//! |--------|----------------------------------|-------------------------------------|
//! | csharp | `generated/csharp/Runtime/`      | `<Ns>.Runtime` (e.g. `Demo.Greeter.Runtime`) |
//! | java   | `generated/java/<pkg>/runtime/`  | `<pkg>.runtime` (e.g. `com.demo.greeter.runtime`) |
//! |        | `generated/java/jni/`            | JNI shim `lib<prefix>_jni` (e.g. `libdemo_greeter_jni`) |
//! | swift  | `generated/swift/Runtime/`       | Swift module of the SDK; C module `C<Module>` |
//! | dart   | `generated/dart/runtime/`        | library of the SDK package          |
//! | ts     | `generated/ts/runtime/`          | module of the SDK package           |
//!
//! Two SDKs generated with this tool can therefore live in one app (one JVM,
//! one .NET process, one Swift app) without clashing classes or JNI symbols.
//!
//! The runtime sources are written for the reference package `wabi:client`
//! (namespace `Wabi.Client.Runtime`, package `com.wabi.client.runtime`, …);
//! emitting them for another package only replaces those names.

use crate::ir::Model;
use crate::names::pascal;

include!(concat!(env!("OUT_DIR"), "/embedded_runtime.rs"));

/// Java package of the runtime of `m`: `com.demo.greeter.runtime`.
pub fn java_package(m: &Model) -> String {
    format!("{}.runtime", crate::gen_java::pkg(m))
}

/// C# namespace of the runtime of `m`: `Demo.Greeter.Runtime`.
pub fn csharp_namespace(m: &Model) -> String {
    format!("{}.Runtime", crate::gen_csharp::cs_ns(m))
}

/// Name of the Swift system-library module that exposes the C ABI: `CDemoGreeter`.
pub fn swift_c_module(m: &Model) -> String {
    format!("C{}{}", pascal(&m.namespace), pascal(&m.package))
}

/// Name of the JNI shim library (without `lib` / extension): `demo_greeter_jni`.
pub fn jni_library(m: &Model) -> String {
    format!("{}_jni", m.symbol_prefix())
}

/// JNI name mangling of a fully qualified class name (JNI spec, "Resolving Native Method Names").
fn jni_mangle(class: &str) -> String {
    let mut out = String::new();
    for c in class.chars() {
        match c {
            '.' | '/' => out.push('_'),
            '_' => out.push_str("_1"),
            ';' => out.push_str("_2"),
            '[' => out.push_str("_3"),
            c if c.is_ascii_alphanumeric() => out.push(c),
            c => out.push_str(&format!("_0{:04x}", c as u32)),
        }
    }
    out
}

const REF_JAVA_PKG: &str = "com.wabi.client.runtime";
const REF_CS_NS: &str = "Wabi.Client.Runtime";

/// Files of the runtime for `lang`, as (path relative to the SDK output dir of `lang`, contents).
/// `abi_from_jni` is the ABI header directory relative to `<java out>/jni` (CMake build of the JNI shim).
pub fn emit(lang: &str, m: &Model, abi_from_jni: &str) -> Vec<(String, String)> {
    let prefix = format!("app/{lang}/");
    let mut out = vec![];
    for (rel, text) in FILES {
        let Some(rel) = rel.strip_prefix(&prefix) else { continue };
        let file = match lang {
            "csharp" => (format!("Runtime/{rel}"), csharp(m, text)),
            "java" => java(m, rel, text, abi_from_jni),
            "swift" => (format!("Runtime/{rel}"), text.replace("import CWabiClient", &format!("import {}", swift_c_module(m)))),
            // The runtime sits below the generated files: `../generated/x` -> `../x`.
            "dart" => (format!("runtime/{rel}"), text.replace("'../generated/", "'../")),
            "ts" => (format!("runtime/{rel}"), text.replace("\"../generated/", "\"../")),
            other => unreachable!("no embedded runtime for {other}"),
        };
        let text = banner(&file.0, &file.1.replace("@LIBRARY@", &m.symbol_prefix()));
        out.push((file.0, text));
    }
    out
}

/// Moves generated code of `lang` from the reference runtime namespace into the
/// package's own (`com.wabi.client.runtime` -> `com.demo.greeter.runtime`, …).
pub fn localize(lang: &str, m: &Model, path: String, text: String) -> (String, String) {
    match lang {
        "java" => {
            let pkg = java_package(m);
            (path.replace(&REF_JAVA_PKG.replace('.', "/"), &pkg.replace('.', "/")), text.replace(REF_JAVA_PKG, &pkg))
        }
        "csharp" => (path, text.replace(REF_CS_NS, &csharp_namespace(m))),
        "swift" => (path, text.replace("import CWabiClient", &format!("import {}", swift_c_module(m)))),
        _ => (path, text),
    }
}

/// The contract-agnostic runtime a core in `lang` (rust, go, csharp) builds
/// against, as (path relative to its directory, contents). Standalone mode only;
/// in this repository the cores use runtime/<lang> directly.
pub fn core(lang: &str) -> Vec<(String, String)> {
    let prefix = format!("core/{lang}/");
    FILES.iter().filter_map(|(rel, text)| rel.strip_prefix(&prefix).map(|r| (r.to_string(), text.to_string()))).collect()
}

/// The ABI headers (wabi_abi.h, wabi_abi_functions.h) and the normative ABI.md.
pub fn abi() -> Vec<(String, String)> {
    FILES.iter().filter_map(|(rel, text)| rel.strip_prefix("abi/").map(|r| (r.to_string(), text.to_string()))).collect()
}

/// SDK copies have package-specific documentation; the canonical ABI stays shared.
pub fn abi_for(m: &Model) -> Vec<(String, String)> {
    abi().into_iter().map(|(name, text)| {
        let text = crate::abi_docs::render(m, &name, &text);
        (name, text)
    }).collect()
}

/// Marks the copy as generated so nobody edits it in a package.
fn banner(path: &str, text: &str) -> String {
    let line = "Copied by wabi-wit-gen from generator/runtime — DO NOT EDIT; change the source there.";
    if path.ends_with(".txt") {
        format!("# {line}\n{text}")
    } else {
        format!("// {line}\n{text}")
    }
}

fn csharp(m: &Model, text: &str) -> String {
    let ns = crate::gen_csharp::cs_ns(m);
    text.replace(REF_CS_NS, &csharp_namespace(m)).replace("\"Wabi.Client.Tests\"", &format!("\"{ns}.Tests\""))
}

fn java(m: &Model, rel: &str, text: &str, abi_from_jni: &str) -> (String, String) {
    let pkg = java_package(m);
    let dir = pkg.replace('.', "/");
    let lib = jni_library(m);
    let common = |t: &str| {
        t.replace("@JNI_CLASS@", &jni_mangle(&format!("{pkg}.JniAbi")))
            .replace(REF_JAVA_PKG, &pkg)
            .replace(&REF_JAVA_PKG.replace('.', "/"), &dir)
            .replace("wabi_jni", &lib)
            .replace("\"wabi.jni.lib\"", &format!("\"{pkg}.jniLib\""))
    };
    if let Some(name) = rel.strip_prefix("com/wabi/client/runtime/") {
        return (format!("{dir}/{name}"), common(text));
    }
    if let Some(name) = rel.strip_prefix("jni/") {
        let name = name.replace("wabi_jni", &lib);
        let mut text = common(text);
        if name == "CMakeLists.txt" {
            text = text.replace("${CMAKE_CURRENT_LIST_DIR}/../../../abi", &format!("${{CMAKE_CURRENT_LIST_DIR}}/{abi_from_jni}"));
        }
        return (format!("jni/{name}"), text);
    }
    panic!("unexpected Java runtime file {rel}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jni_mangling() {
        assert_eq!(jni_mangle("com.wabi.client.runtime.JniAbi"), "com_wabi_client_runtime_JniAbi");
        assert_eq!(jni_mangle("com.my_org.pkg.runtime.JniAbi"), "com_my_1org_pkg_runtime_JniAbi");
    }

    #[test]
    fn embedded() {
        for lang in ["csharp", "java", "swift", "dart", "ts"] {
            assert!(FILES.iter().any(|(p, _)| p.starts_with(&format!("app/{lang}/"))), "no runtime files for {lang}");
        }
        assert!(FILES.iter().any(|(p, _)| *p == "app/java/jni/wabi_jni.c"));
        for lang in ["rust", "go", "csharp"] {
            assert!(!core(lang).is_empty(), "no core runtime for {lang}");
        }
        assert!(core("rust").iter().any(|(p, _)| p == "Cargo.toml"));
        assert!(core("go").iter().all(|(p, _)| !p.ends_with("_test.go")));
        assert!(abi().iter().any(|(p, _)| p == "wabi_abi.h"));
    }
}

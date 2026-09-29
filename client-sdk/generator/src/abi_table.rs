//! The generic native ABI as data. Language emitters derive their P/Invoke /
//! Panama FFM declarations from this table, and `check_header` verifies that
//! it matches `abi/acme_abi.h` + `abi/acme_abi_functions.h` (the normative C definition), so the three
//! can never drift apart. Note: this table does NOT grow with the WIT file.

use anyhow::{bail, Context, Result};
use std::path::Path;

/// C types used by the ABI. Everything is fixed width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CTy {
    Void,
    U32,
    U64,
    Status,
    Handle,
    Bytes,
    Buffer,
    BufferPtr,
    HandlePtr,
    Callback,
    VoidPtr,
    CStr,
    /// acme_host_handler (ABI 1.1)
    HostHandler,
}

impl CTy {
    fn c(&self) -> &'static str {
        match self {
            CTy::Void => "void",
            CTy::U32 => "uint32_t",
            CTy::U64 => "uint64_t",
            CTy::Status => "acme_status",
            CTy::Handle => "HANDLE",
            CTy::Bytes => "acme_bytes",
            CTy::Buffer => "acme_buffer",
            CTy::BufferPtr => "acme_buffer*",
            CTy::HandlePtr => "HANDLE*",
            CTy::Callback => "acme_future_callback",
            CTy::VoidPtr => "void*",
            CTy::CStr => "const char*",
            CTy::HostHandler => "acme_host_handler",
        }
    }
}

pub struct AbiFn {
    pub name: &'static str,
    pub ret: CTy,
    pub params: &'static [(CTy, &'static str)],
    /// ABI 1.1 host-call functions: only exported by cores with
    /// ACME_CAP_HOST_CALLS; bindings must treat them as optional.
    pub host_call: bool,
}

impl AbiFn {
    const fn core(name: &'static str, ret: CTy, params: &'static [(CTy, &'static str)]) -> AbiFn {
        AbiFn { name, ret, params, host_call: false }
    }
}

use CTy::*;

pub const FUNCTIONS: &[AbiFn] = &[
    AbiFn::core("acme_abi_version", U32, &[]),
    AbiFn::core("acme_abi_capabilities", U64, &[]),
    AbiFn::core("acme_contract_id", CStr, &[]),
    AbiFn::core("acme_core_info", CStr, &[]),
    AbiFn::core("acme_status_name", CStr, &[(Status, "status")]),
    AbiFn::core("acme_buffer_free", Void, &[(Buffer, "buffer")]),
    AbiFn::core("acme_call_sync", Status, &[(Handle, "object"), (U32, "methodId"), (Bytes, "request"), (BufferPtr, "outResponse")]),
    AbiFn::core("acme_call_async", Status, &[(Handle, "object"), (U32, "methodId"), (Bytes, "request"), (HandlePtr, "outFuture")]),
    AbiFn::core("acme_object_drop", Status, &[(Handle, "object")]),
    AbiFn::core("acme_future_subscribe", Status, &[(Handle, "future"), (Callback, "callback"), (VoidPtr, "userData")]),
    AbiFn::core("acme_future_poll", Status, &[(Handle, "future")]),
    AbiFn::core("acme_future_take", Status, &[(Handle, "future"), (BufferPtr, "outResult")]),
    AbiFn::core("acme_future_cancel", Status, &[(Handle, "future")]),
    AbiFn::core("acme_future_drop", Status, &[(Handle, "future")]),
    AbiFn::core("acme_stream_next", Status, &[(Handle, "stream"), (HandlePtr, "outFuture")]),
    AbiFn::core("acme_stream_cancel", Status, &[(Handle, "stream")]),
    AbiFn::core("acme_stream_drop", Status, &[(Handle, "stream")]),
    AbiFn { name: "acme_host_register", ret: Status, params: &[(HostHandler, "handler"), (VoidPtr, "userData")], host_call: true },
    AbiFn { name: "acme_host_reply", ret: Status, params: &[(U64, "call"), (Status, "status"), (Bytes, "response")], host_call: true },
];

/// Status codes (name without `ACME_` prefix, value).
pub const STATUSES: &[(&str, i32)] = &[
    ("OK", 0),
    ("PENDING", 1),
    ("READY", 2),
    ("STREAM_END", 3),
    ("ERR_INVALID_HANDLE", -1),
    ("ERR_INVALID_ARGUMENT", -2),
    ("ERR_UNKNOWN_METHOD", -3),
    ("ERR_CANCELLED", -4),
    ("ERR_BUSY", -5),
    ("ERR_ALREADY_TAKEN", -6),
    ("ERR_UNSUPPORTED", -7),
    ("ERR_INTERNAL", -8),
];

pub const CAPABILITIES: &[(&str, u32)] = &[
    ("SYNC_CALLS", 0),
    ("ASYNC_CALLS", 1),
    ("STREAMS", 2),
    ("CANCELLATION", 3),
    ("THREADED_CALLBACK", 4),
    ("PAYLOAD_MSGPACK_1", 5),
    ("HOST_CALLS", 6),
];

pub const ABI_MAJOR: u32 = 1;
pub const ABI_MINOR: u32 = 1;

/// `acme_call_sync` (generic table name) -> `<prefix>_call_sync` (exported symbol).
pub fn symbol(prefix: &str, generic: &str) -> String {
    format!("{prefix}_{}", generic.strip_prefix("acme_").expect("table names start with acme_"))
}

/// Checks abi/acme_abi.h (constants) and abi/acme_abi_functions.h (prototypes,
/// written as `ACME_FN(name)`) against this table.
pub fn check_header(abi_dir: &Path) -> Result<()> {
    let types = abi_dir.join("acme_abi.h");
    let funcs = abi_dir.join("acme_abi_functions.h");
    let consts = std::fs::read_to_string(&types).with_context(|| format!("reading {}", types.display()))?;
    let protos = std::fs::read_to_string(&funcs).with_context(|| format!("reading {}", funcs.display()))?;
    // ACME_FN(call_sync) -> acme_call_sync, so prototypes parse like plain C.
    let mut text = String::new();
    let mut rest = protos.as_str();
    while let Some(i) = rest.find("ACME_FN(") {
        text.push_str(&rest[..i]);
        let after = &rest[i + 8..];
        let close = after.find(')').context("unterminated ACME_FN(")?;
        text.push_str("acme_");
        text.push_str(&after[..close]);
        rest = &after[close + 1..];
    }
    text.push_str(rest);
    let path = funcs.as_path();
    let mut found = vec![];
    for stmt in text.split(';') {
        // Only real prototypes: `ACME_API` at the start of a line.
        let Some(pos) = stmt.find("\nACME_API ") else { continue };
        let proto: String = stmt[pos + 10..].split_whitespace().collect::<Vec<_>>().join(" ");
        let (head, rest) = proto.split_once('(').context("prototype without (")?;
        let params = rest.trim_end().trim_end_matches(')');
        let (ret, name) = split_decl(head);
        let ptys: Vec<String> = if params.trim() == "void" {
            vec![]
        } else {
            params.split(',').map(|p| split_decl(p.trim()).0).collect()
        };
        found.push((name, ret, ptys));
    }
    for f in FUNCTIONS {
        let Some((_, ret, ptys)) = found.iter().find(|(n, _, _)| n == f.name) else {
            bail!("abi table: {} missing from {}", f.name, path.display());
        };
        let want: Vec<String> = f.params.iter().map(|(t, _)| t.c().to_string()).collect();
        let got: Vec<String> = ptys.iter().map(|t| normalize_handle(t)).collect();
        if normalize_handle(ret) != f.ret.c() || got != want {
            bail!("abi table: {} does not match header: header ({ret}; {ptys:?}) vs table ({}; {want:?})", f.name, f.ret.c());
        }
    }
    if found.len() != FUNCTIONS.len() {
        bail!("abi table: header declares {} functions, table {}", found.len(), FUNCTIONS.len());
    }
    let text = consts;
    for (name, value) in STATUSES {
        let pat = format!("#define ACME_{name} ");
        let line = text.lines().find(|l| l.starts_with(&pat)).with_context(|| format!("ACME_{name} missing in header"))?;
        let v: i32 = line[pat.len()..].split_whitespace().next().unwrap().parse()?;
        if v != *value {
            bail!("ACME_{name}: header {v} != table {value}");
        }
    }
    for (name, bit) in CAPABILITIES {
        let needle = format!("ACME_CAP_{name}");
        let def = format!("#define {needle} ");
        let line = text.lines().find(|l| l.starts_with(&def)).with_context(|| format!("{needle} missing"))?;
        if !line.contains(&format!("(1ull << {bit})")) {
            bail!("{needle}: bit mismatch with table");
        }
    }
    let major = format!("#define ACME_ABI_VERSION_MAJOR {ABI_MAJOR}u");
    let minor = format!("#define ACME_ABI_VERSION_MINOR {ABI_MINOR}u");
    if !text.contains(&major) || !text.contains(&minor) {
        bail!("ABI version in header does not match generator ({ABI_MAJOR}.{ABI_MINOR})");
    }
    Ok(())
}

/// "const char *acme_core_info" -> ("const char*", "acme_core_info")
fn split_decl(s: &str) -> (String, String) {
    let s = s.replace(" *", "* ").replace("* ", "*");
    let s = s.replace('*', "* ");
    let s: Vec<&str> = s.split_whitespace().collect();
    let name = s.last().unwrap().to_string();
    let ty = s[..s.len() - 1].join(" ").replace(" *", "*");
    (ty, name)
}

fn normalize_handle(t: &str) -> String {
    t.replace("acme_object_handle", "HANDLE")
        .replace("acme_future_handle", "HANDLE")
        .replace("acme_stream_handle", "HANDLE")
}

fn c_ty(t: CTy) -> &'static str {
    match t {
        CTy::Handle => "uint64_t",
        CTy::HandlePtr => "uint64_t *",
        CTy::BufferPtr => "acme_buffer *",
        CTy::VoidPtr => "void *",
        CTy::CStr => "const char *",
        other => other.c(),
    }
}

/// `outResponse` -> `out_response`
fn c_param(n: &str) -> String {
    let mut out = String::new();
    for ch in n.chars() {
        if ch.is_ascii_uppercase() {
            out.push('_');
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// "acme_buffer *" + "out" -> "acme_buffer *out"; "uint32_t" + "x" -> "uint32_t x"
fn c_decl(ty: &str, name: &str) -> String {
    if ty.ends_with('*') { format!("{ty}{name}") } else { format!("{ty} {name}") }
}

/// generated/c/<prefix>.h — the package's view of the ABI: the 17 functions
/// under this package's symbol prefix (expanded from acme_abi_functions.h).
pub fn emit_c_header(m: &crate::ir::Model) -> String {
    let prefix = m.symbol_prefix();
    let upper = prefix.to_ascii_uppercase();
    let mut o = String::new();
    w!(o, "/* Generated by acme-wit-gen from the wit/ directory — DO NOT EDIT.");
    w!(o, " *");
    w!(o, " * Native ABI bindings of package {} (symbol prefix `{prefix}_`).", m.contract_id());
    w!(o, " * Normative definitions: abi/acme_abi.h, abi/acme_abi_functions.h, abi/ABI.md.");
    w!(o, " * Compile with -I<client-sdk>/abi (or put acme_abi.h next to this file). */");
    w!(o, "#ifndef {upper}_ACME_BINDINGS_H");
    w!(o, "#define {upper}_ACME_BINDINGS_H");
    w!(o, "");
    w!(o, "#ifndef ACME_ABI_H");
    w!(o, "#include \"acme_abi.h\"");
    w!(o, "#endif");
    w!(o, "");
    w!(o, "#define {upper}_CONTRACT_ID \"{}\"", m.contract_id());
    w!(o, "#define {upper}_LIBRARY \"{prefix}\"");
    w!(o, "#define {upper}_SYMBOL_PREFIX \"{prefix}_\"");
    w!(o, "");
    w!(o, "#ifdef __cplusplus");
    w!(o, "extern \"C\" {{");
    w!(o, "#endif");
    w!(o, "");
    for f in FUNCTIONS {
        let ps: Vec<String> = f.params.iter().map(|(t, n)| c_decl(c_ty(*t), &c_param(n))).collect();
        let ps = if ps.is_empty() { "void".to_string() } else { ps.join(", ") };
        w!(o, "ACME_API {}({ps});", c_decl(c_ty(f.ret), &symbol(&prefix, f.name)));
    }
    w!(o, "");
    w!(o, "#ifdef __cplusplus");
    w!(o, "}}");
    w!(o, "#endif");
    w!(o, "");
    w!(o, "#endif /* {upper}_ACME_BINDINGS_H */");
    o
}

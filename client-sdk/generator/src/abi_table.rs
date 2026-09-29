//! The generic native ABI as data. Language emitters derive their P/Invoke /
//! Panama FFM declarations from this table, and `check_header` verifies that
//! it matches `abi/acme_client.h` (the normative C definition), so the three
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
        }
    }
}

pub struct AbiFn {
    pub name: &'static str,
    pub ret: CTy,
    pub params: &'static [(CTy, &'static str)],
}

use CTy::*;

pub const FUNCTIONS: &[AbiFn] = &[
    AbiFn { name: "acme_abi_version", ret: U32, params: &[] },
    AbiFn { name: "acme_abi_capabilities", ret: U64, params: &[] },
    AbiFn { name: "acme_contract_id", ret: CStr, params: &[] },
    AbiFn { name: "acme_core_info", ret: CStr, params: &[] },
    AbiFn { name: "acme_status_name", ret: CStr, params: &[(Status, "status")] },
    AbiFn { name: "acme_buffer_free", ret: Void, params: &[(Buffer, "buffer")] },
    AbiFn {
        name: "acme_call_sync",
        ret: Status,
        params: &[(Handle, "object"), (U32, "methodId"), (Bytes, "request"), (BufferPtr, "outResponse")],
    },
    AbiFn {
        name: "acme_call_async",
        ret: Status,
        params: &[(Handle, "object"), (U32, "methodId"), (Bytes, "request"), (HandlePtr, "outFuture")],
    },
    AbiFn { name: "acme_object_drop", ret: Status, params: &[(Handle, "object")] },
    AbiFn {
        name: "acme_future_subscribe",
        ret: Status,
        params: &[(Handle, "future"), (Callback, "callback"), (VoidPtr, "userData")],
    },
    AbiFn { name: "acme_future_poll", ret: Status, params: &[(Handle, "future")] },
    AbiFn { name: "acme_future_take", ret: Status, params: &[(Handle, "future"), (BufferPtr, "outResult")] },
    AbiFn { name: "acme_future_cancel", ret: Status, params: &[(Handle, "future")] },
    AbiFn { name: "acme_future_drop", ret: Status, params: &[(Handle, "future")] },
    AbiFn { name: "acme_stream_next", ret: Status, params: &[(Handle, "stream"), (HandlePtr, "outFuture")] },
    AbiFn { name: "acme_stream_cancel", ret: Status, params: &[(Handle, "stream")] },
    AbiFn { name: "acme_stream_drop", ret: Status, params: &[(Handle, "stream")] },
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
];

pub const ABI_MAJOR: u32 = 1;
pub const ABI_MINOR: u32 = 0;

/// Parses the prototypes (and #defines) of acme_client.h and compares them with
/// this table.
pub fn check_header(path: &Path) -> Result<()> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
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
        let line = text.lines().find(|l| l.contains(&needle)).with_context(|| format!("{needle} missing"))?;
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

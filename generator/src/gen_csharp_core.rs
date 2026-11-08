//! Emits the *core-side* glue for a C# core (NativeAOT shared library, runtime
//! in `runtime/csharp-core`): the package part of the exports (symbol prefix,
//! contract id, host-call flag), one interface per resource that the core
//! implements, the `ICore` factory, the method-id dispatcher and typed clients
//! for the WIT imports (host calls). The contract types and their codec are the
//! ones generated for the C# SDK, so SDK and core share one type definition.
//!
//! The core provides `sealed class Core : ICore` (public no-arg constructor).
//! Domain errors are thrown as the generated exception (`ClientErrorException`,
//! ...), in sync methods and from the returned `Task` alike.

use crate::gen_csharp::{cs, cs_ns, dec, docs, enc, header, ident};
use crate::ir::*;
use crate::names::*;
use anyhow::{bail, Result};

pub fn emit(m: &Model) -> Result<Vec<(String, String)>> {
    for r in &m.resources {
        if r.ctor.is_none() {
            bail!("target csharp-core: resource `{}` needs a constructor", r.name);
        }
        if r.methods.iter().any(|f| f.is_async && matches!(f.ret, Ret::Value(Ty::Stream(_)))) {
            bail!("target csharp-core: `{}`: stream methods must not be async", r.name);
        }
    }
    Ok(vec![("CoreContract.g.cs".into(), contract(m)), ("CoreDispatch.g.cs".into(), dispatch(m))])
}

fn usings(o: &mut String, m: &Model) {
    w!(o, "using System;");
    w!(o, "using System.Collections.Generic;");
    w!(o, "using System.Threading;");
    w!(o, "using System.Threading.Tasks;");
    // The SDK side's MessagePack runtime and contract types, in the package's namespace.
    w!(o, "using {};", crate::runtime::csharp_namespace(m));
    w!(o, "using {};", cs_ns(m));
    w!(o, "");
    w!(o, "namespace Wabi.Core;");
}

/// Core-side interface name of a resource: `key-store` -> `IKeyStore`.
fn iface(r: &Resource) -> String {
    format!("I{}", pascal(&r.name))
}

/// C# type of a parameter or result in the core: resources are the core's own
/// interfaces (`ITransport`), everything else is shared with the SDK.
fn core_ty(t: &Ty) -> String {
    match t {
        Ty::Handle { resource, .. } => format!("I{}", pascal(resource)),
        t => cs(t),
    }
}

/// Decodes a parameter: a borrowed object stays, an owned one moves into the core.
fn core_dec(t: &Ty) -> String {
    match t {
        Ty::Handle { resource, owned } => format!("Handles.Arg<I{}>(r.ReadUInt64(), {owned})", pascal(resource)),
        t => dec(t, "r", 1),
    }
}

/// Encodes the result in `e`: a resource the core hands to the host gets its own handle.
fn core_enc(t: &Ty, e: &str) -> String {
    match t {
        Ty::Handle { .. } => format!("w.WriteUInt(Handles.Add(Handles.Object, {e}));"),
        t => enc(t, e, 1),
    }
}

/// Name of the method-id constant: resource/interface + function.
fn id_const(owner: &str, f: &Func) -> String {
    format!("{}{}", pascal(owner), pascal(&f.name))
}

fn is_stream(f: &Func) -> Option<&Ty> {
    match &f.ret {
        Ret::Value(Ty::Stream(t)) => Some(t),
        _ => None,
    }
}

fn ok_ty(f: &Func) -> Option<&Ty> {
    match &f.ret {
        Ret::Unit | Ret::Result { ok: None, .. } => None,
        Ret::Value(t) | Ret::Result { ok: Some(t), .. } => Some(t),
    }
}

/// Return type of a core method: `T`, `void`, `Task<T>`, `Task` or `CoreStream<T>`.
fn ret_ty(f: &Func) -> String {
    if let Some(item) = is_stream(f) {
        return format!("CoreStream<{}>", cs(item));
    }
    match (f.is_async, ok_ty(f)) {
        (true, Some(t)) => format!("Task<{}>", core_ty(t)),
        (true, None) => "Task".into(),
        (false, Some(t)) => core_ty(t),
        (false, None) => "void".into(),
    }
}

fn method_name(f: &Func) -> String {
    if f.is_async { format!("{}Async", pascal(&f.name)) } else { pascal(&f.name) }
}

fn params(f: &Func, cancellation: bool) -> String {
    let mut ps: Vec<String> = f.params.iter().map(|(n, t)| format!("{} {}", core_ty(t), ident(n))).collect();
    if cancellation {
        ps.push("CancellationToken cancellationToken".into());
    }
    ps.join(", ")
}

/// `<see cref>` of the exception carrying the function's domain error, if any.
fn error_doc(f: &Func) -> Option<String> {
    match &f.ret {
        Ret::Result { err: Some(Ty::Named(n)), .. } => Some(exception_name(n)),
        Ret::Result { .. } => Some("WabiCallFailedException".into()),
        _ => None,
    }
}

fn contract(m: &Model) -> String {
    let mut o = header();
    usings(&mut o, m);
    w!(o, "");
    w!(o, "/// <summary>Package part of the generic exports (runtime/csharp-core/CoreAbi.cs).</summary>");
    w!(o, "static partial class Exports");
    w!(o, "{{");
    w!(o, "    /// <summary>Symbol prefix of {}: {}_call_sync, ...</summary>", m.contract_id(), m.symbol_prefix());
    w!(o, "    private const string Prefix = \"{}_\";", m.symbol_prefix());
    w!(o, "    private const string ContractIdText = \"{}\";", m.contract_id());
    if m.needs_host_calls() {
        w!(o, "    private const bool HostCallsEnabled = true; // WIT imports: compile runtime/csharp-core/CoreHostCalls.cs");
    } else {
        w!(o, "    private const bool HostCallsEnabled = false; // no WIT imports");
    }
    w!(o, "}}");
    w!(o, "");
    w!(o, "/// <summary>");
    w!(o, "/// What a C# core for {} provides: <c>sealed class Core : ICore</c>", m.contract_id());
    w!(o, "/// with a public no-arg constructor.");
    w!(o, "/// </summary>");
    w!(o, "interface ICore");
    w!(o, "{{");
    w!(o, "    /// <summary>Free text identifying the core implementation ({}_core_info).</summary>", m.symbol_prefix());
    w!(o, "    string CoreInfo {{ get; }}");
    for r in &m.resources {
        let c = r.ctor.as_ref().unwrap();
        w!(o, "");
        docs(&mut o, &c.docs, "    ");
        if let Some(e) = error_doc(c) {
            w!(o, "    /// <exception cref=\"{e}\">domain error</exception>");
        }
        w!(o, "    {} Create{}({});", iface(r), pascal(&r.name), params(c, false));
    }
    w!(o, "}}");
    for r in &m.resources {
        w!(o, "");
        docs(&mut o, &r.docs, "");
        w!(o, "/// <remarks>");
        w!(o, "/// WIT resource `{}`, implemented by the core. Async methods get the host's", r.name);
        w!(o, "/// cancellation (<c>future_cancel</c>) as token; domain errors are thrown as the");
        w!(o, "/// generated exception. <see cref=\"ICoreObject.Close\"/> runs when the host drops");
        w!(o, "/// the handle; in-flight calls may still use the object.");
        w!(o, "/// </remarks>");
        w!(o, "interface {} : ICoreObject", iface(r));
        w!(o, "{{");
        for (i, f) in r.methods.iter().enumerate() {
            if i > 0 {
                w!(o, "");
            }
            docs(&mut o, &f.docs, "    ");
            if let Some(e) = error_doc(f) {
                w!(o, "    /// <exception cref=\"{e}\">domain error</exception>");
            }
            w!(o, "    {} {}({});", ret_ty(f), method_name(f), params(f, f.is_async));
        }
        w!(o, "}}");
    }
    for h in &m.imports {
        w!(o, "");
        import_client(&mut o, h);
    }
    o
}

/// Typed client of one WIT import: every function is a host call into the app.
fn import_client(o: &mut String, h: &HostInterface) {
    docs(o, &h.docs, "");
    w!(o, "/// <remarks>");
    w!(o, "/// WIT import `{}`: implemented by the APP, reached through host calls (ABI 1.1).", h.name);
    w!(o, "/// Only call from async work. Domain errors are thrown as the generated exception;");
    w!(o, "/// <see cref=\"HostCallException\"/> reports a call without answer (no handler,");
    w!(o, "/// cancelled, error status, malformed answer).");
    w!(o, "/// </remarks>");
    w!(o, "static class {}", pascal(&h.name));
    w!(o, "{{");
    let ids: Vec<String> = h.funcs.iter().map(|f| format!("{} = 0x{:04x}", pascal(&f.name), f.id)).collect();
    w!(o, "    private const uint {};", ids.join(", "));
    for f in &h.funcs {
        w!(o, "");
        docs(o, &f.docs, "    ");
        let ret = match ok_ty(f) {
            Some(t) => format!("Task<{}>", cs(t)),
            None => "Task".into(),
        };
        let mut ps: Vec<String> = f.params.iter().map(|(n, t)| format!("{} {}", cs(t), ident(n))).collect();
        ps.push("CancellationToken cancellationToken = default".into());
        w!(o, "    public static async {ret} {}Async({})", pascal(&f.name), ps.join(", "));
        w!(o, "    {{");
        w!(o, "        var request = Wire.Encode(w =>");
        w!(o, "        {{");
        w!(o, "            w.WriteArrayHeader({});", f.params.len());
        for (i, (n, t)) in f.params.iter().enumerate() {
            w!(o, "            {}", enc(t, &ident(n), 1 + 10 * i));
        }
        w!(o, "        }});");
        w!(o, "        var r = new MsgPackReader(await HostCalls.CallAsync({}, request, cancellationToken).ConfigureAwait(false));", pascal(&f.name));
        let domain = match &f.ret {
            Ret::Result { err: Some(Ty::Named(n)), .. } => exception_name(n),
            _ => "WabiCallFailedException".into(),
        };
        w!(o, "        try");
        w!(o, "        {{");
        match &f.ret {
            Ret::Unit => {
                w!(o, "            r.ReadNil();");
                w!(o, "            Done(r);");
            }
            Ret::Value(t) => {
                w!(o, "            var ret = {};", dec(t, "r", 1));
                w!(o, "            Done(r);");
                w!(o, "            return ret;");
            }
            Ret::Result { ok, err } => {
                w!(o, "            if (r.ReadResultTag() != 0)");
                w!(o, "            {{");
                match err {
                    Some(Ty::Named(n)) => w!(o, "                var error = WireCodec.Read{}(r);", pascal(n)),
                    Some(t) => w!(o, "                var error = {};", dec(t, "r", 1)),
                    None => w!(o, "                object? error = null;\n                r.ReadNil();"),
                }
                w!(o, "                Done(r);");
                w!(o, "                throw new {domain}(error);");
                w!(o, "            }}");
                match ok {
                    Some(t) => {
                        w!(o, "            var ret = {};", dec(t, "r", 1));
                        w!(o, "            Done(r);");
                        w!(o, "            return ret;");
                    }
                    None => {
                        w!(o, "            r.ReadNil();");
                        w!(o, "            Done(r);");
                    }
                }
            }
        }
        w!(o, "        }}");
        w!(o, "        catch (Exception e) when (e is not {domain})");
        w!(o, "        {{");
        w!(o, "            throw new HostCallException(Status.InvalidArgument, $\"{}.{}: malformed answer: {{e.Message}}\");", h.name, f.name);
        w!(o, "        }}");
        w!(o, "    }}");
    }
    w!(o, "");
    w!(o, "    private static void Done(MsgPackReader r)");
    w!(o, "    {{");
    w!(o, "        if (r.Remaining != 0) throw new WabiProtocolException($\"{{r.Remaining}} trailing bytes\");");
    w!(o, "    }}");
    w!(o, "}}");
}

/// Locals for decoded parameters: prefixed so WIT names cannot collide with the dispatcher's.
fn local(n: &str) -> String {
    format!("p{}", pascal(n))
}

fn decode_params(o: &mut String, f: &Func, ind: &str) {
    w!(o, "{ind}var r = Params(request, {});", f.params.len());
    for (n, t) in &f.params {
        w!(o, "{ind}var {} = {};", local(n), core_dec(t));
    }
    w!(o, "{ind}Done(r);");
}

fn args(f: &Func) -> Vec<String> {
    f.params.iter().map(|(n, _)| local(n)).collect()
}

/// Expression (a byte[]) encoding the complete ok payload; the value is in `ret`.
fn encode_ok(f: &Func) -> String {
    match &f.ret {
        Ret::Unit => "Wire.Encode(w => w.WriteNil())".into(),
        Ret::Value(t) => format!("Wire.Encode(w => {{ {} }})", core_enc(t, "ret")),
        Ret::Result { ok: Some(t), .. } => format!("Wire.Encode(w => {{ w.WriteArrayHeader(2); w.WriteUInt(0); {} }})", core_enc(t, "ret")),
        Ret::Result { ok: None, .. } => "Wire.Encode(w => { w.WriteArrayHeader(2); w.WriteUInt(0); w.WriteNil(); })".into(),
    }
}

/// `catch` clause turning the function's domain error into the `[1, error]` payload, if it has one.
fn catch_err(o: &mut String, f: &Func, ind: &str, ret: &str) {
    let Ret::Result { err, .. } = &f.ret else { return };
    match err {
        Some(Ty::Named(n)) => {
            w!(o, "{ind}catch ({} e)", exception_name(n));
            w!(o, "{ind}{{");
            w!(o, "{ind}    {ret}Wire.Encode(w => {{ w.WriteArrayHeader(2); w.WriteUInt(1); WireCodec.Write{}(w, e.Error); }}){};", pascal(n), ret_close(ret));
        }
        Some(t) => {
            w!(o, "{ind}catch (WabiCallFailedException e) when (e.Error is {} error)", cs(t));
            w!(o, "{ind}{{");
            w!(o, "{ind}    {ret}Wire.Encode(w => {{ w.WriteArrayHeader(2); w.WriteUInt(1); {} }}){};", enc(t, "error", 1), ret_close(ret));
        }
        None => {
            w!(o, "{ind}catch (WabiCallFailedException)");
            w!(o, "{ind}{{");
            w!(o, "{ind}    {ret}Wire.Encode(w => {{ w.WriteArrayHeader(2); w.WriteUInt(1); w.WriteNil(); }}){};", ret_close(ret));
        }
    }
    w!(o, "{ind}}}");
}

fn ret_close(ret: &str) -> &'static str {
    if ret.contains('(') { ")" } else { "" }
}

fn dispatch(m: &Model) -> String {
    let mut o = header();
    usings(&mut o, m);
    w!(o, "");
    w!(o, "/// <summary>Method-id dispatcher of {} for the generic C# core runtime.</summary>", m.contract_id());
    w!(o, "static class Dispatch");
    w!(o, "{{");
    w!(o, "    // Method ids: wit/abi-ids.lock");
    let mut ids = vec![];
    for r in &m.resources {
        for f in r.ctor.iter().chain(&r.methods) {
            ids.push(format!("{} = 0x{:04x}", id_const(&r.name, f), f.id));
        }
    }
    w!(o, "    private const uint {};", ids.join(",\n        "));
    w!(o, "");
    w!(o, "    private static readonly ICore Impl = new Core();");
    w!(o, "");
    w!(o, "    public static string CoreInfo => Impl.CoreInfo;");
    w!(o, "");
    w!(o, "    private static MsgPackReader Params(byte[] request, int n)");
    w!(o, "    {{");
    w!(o, "        var r = new MsgPackReader(request);");
    w!(o, "        var got = r.ReadArrayHeader();");
    w!(o, "        if (got != n) throw new WabiProtocolException($\"expected {{n}} parameters, found {{got}}\");");
    w!(o, "        return r;");
    w!(o, "    }}");
    w!(o, "");
    w!(o, "    private static void Done(MsgPackReader r)");
    w!(o, "    {{");
    w!(o, "        if (r.Remaining != 0) throw new WabiProtocolException($\"{{r.Remaining}} trailing bytes after payload\");");
    w!(o, "    }}");
    w!(o, "");

    // ------------------------------------------------------------- sync
    w!(o, "    public static (int, byte[]?) Sync(ulong obj, uint method, byte[] request)");
    w!(o, "    {{");
    w!(o, "        switch (method)");
    w!(o, "        {{");
    for r in &m.resources {
        let c = r.ctor.as_ref().unwrap();
        w!(o, "            case {}:", id_const(&r.name, c));
        w!(o, "            {{");
        w!(o, "                if (obj != 0) return (Status.InvalidHandle, null);");
        decode_params(&mut o, c, "                ");
        let create = format!("Impl.Create{}({})", pascal(&r.name), args(c).join(", "));
        if matches!(c.ret, Ret::Result { .. }) {
            w!(o, "                {} created;", iface(r));
            w!(o, "                try");
            w!(o, "                {{");
            w!(o, "                    created = {create};");
            w!(o, "                }}");
            catch_err(&mut o, c, "                ", "return (Status.Ok, ");
            w!(o, "                var handle = Handles.Add(Handles.Object, created);");
        } else {
            w!(o, "                var handle = Handles.Add(Handles.Object, {create});");
        }
        w!(o, "                return (Status.Ok, Wire.Encode(w => w.WriteUInt(handle)));");
        w!(o, "            }}");
        for f in r.methods.iter().filter(|f| !f.is_async) {
            w!(o, "            case {}:", id_const(&r.name, f));
            w!(o, "            {{");
            w!(o, "                if (Handles.Get<{}>(obj) is not {{ }} self) return (Status.InvalidHandle, null);", iface(r));
            decode_params(&mut o, f, "                ");
            let call = format!("self.{}({})", method_name(f), args(f).join(", "));
            if let Some(item) = is_stream(f) {
                w!(o, "                var s = {call};");
                w!(o, "                var stream = Handles.Add(Handles.Stream, new NativeStream<{}>(s.Items,", cs(item));
                w!(o, "                    item => Wire.Encode(w => {{ {} }}), s.Release ?? (() => {{ }})));", enc(item, "item", 1));
                w!(o, "                return (Status.Ok, Wire.Encode(w => w.WriteUInt(stream)));");
            } else {
                let has_err = matches!(f.ret, Ret::Result { .. });
                let ind = if has_err { "                    " } else { "                " };
                if has_err {
                    w!(o, "                try");
                    w!(o, "                {{");
                }
                match ok_ty(f) {
                    Some(_) => w!(o, "{ind}var ret = {call};"),
                    None => w!(o, "{ind}{call};"),
                }
                w!(o, "{ind}return (Status.Ok, {});", encode_ok(f));
                if has_err {
                    w!(o, "                }}");
                    catch_err(&mut o, f, "                ", "return (Status.Ok, ");
                }
            }
            w!(o, "            }}");
        }
        let asyncs: Vec<String> = r.methods.iter().filter(|f| f.is_async).map(|f| id_const(&r.name, f)).collect();
        if !asyncs.is_empty() {
            w!(o, "            case {}:", asyncs.join(" or "));
            w!(o, "                return (Handles.Get<{}>(obj) is null ? Status.InvalidHandle : Status.Unsupported, null);", iface(r));
        }
    }
    w!(o, "            default:");
    w!(o, "                return (Status.UnknownMethod, null);");
    w!(o, "        }}");
    w!(o, "    }}");
    w!(o, "");

    // ------------------------------------------------------------ async
    w!(o, "    public static (int, Func<CancellationToken, Task<byte[]>>?) Async(ulong obj, uint method, byte[] request)");
    w!(o, "    {{");
    w!(o, "        switch (method)");
    w!(o, "        {{");
    let syncs: Vec<String> = m
        .resources
        .iter()
        .flat_map(|r| r.ctor.iter().chain(r.methods.iter().filter(|f| !f.is_async)).map(move |f| id_const(&r.name, f)))
        .collect();
    w!(o, "            case {}:", syncs.join(" or "));
    w!(o, "                return (Status.Unsupported, null);");
    for r in &m.resources {
        for f in r.methods.iter().filter(|f| f.is_async) {
            w!(o, "            case {}:", id_const(&r.name, f));
            w!(o, "            {{");
            w!(o, "                if (Handles.Get<{}>(obj) is not {{ }} self) return (Status.InvalidHandle, null);", iface(r));
            decode_params(&mut o, f, "                ");
            let mut a = args(f);
            a.push("cancellationToken".into());
            let call = format!("await self.{}({}).ConfigureAwait(false)", method_name(f), a.join(", "));
            w!(o, "                return (Status.Ok, async cancellationToken =>");
            w!(o, "                {{");
            let has_err = matches!(f.ret, Ret::Result { .. });
            let ind = "                    ";
            if has_err {
                w!(o, "                    try");
                w!(o, "                    {{");
            }
            let ind2 = if has_err { format!("{ind}    ") } else { ind.to_string() };
            match ok_ty(f) {
                Some(_) => w!(o, "{ind2}var ret = {call};"),
                None => w!(o, "{ind2}{call};"),
            }
            w!(o, "{ind2}return {};", encode_ok(f));
            if has_err {
                w!(o, "                    }}");
                catch_err(&mut o, f, "                    ", "return ");
            }
            w!(o, "                }});");
            w!(o, "            }}");
        }
    }
    w!(o, "            default:");
    w!(o, "                return (Status.UnknownMethod, null);");
    w!(o, "        }}");
    w!(o, "    }}");
    w!(o, "}}");
    o
}

#[cfg(test)]
mod tests {
    use crate::{ir, lock};

    fn model(src: &str) -> ir::Model {
        let mut lock = lock::Lock::parse("").unwrap();
        let mut resolve = wit_parser::Resolve::default();
        let pkg = resolve.push_str("t.wit", src).unwrap();
        ir::build(&resolve, pkg, &mut lock).unwrap()
    }

    #[test]
    fn every_method_is_dispatched_and_every_import_has_a_client() {
        let m = model(
            "package demo:vault@0.1.0;
            interface types { variant vault-error { key-not-found(string), crypto(string), } }
            interface host-keys {
                use types.{vault-error};
                sign: func(key-id: string, digest: list<u8>) -> result<list<u8>, vault-error>;
            }
            interface api {
                use types.{vault-error};
                resource signer {
                    constructor(key-id: string);
                    thumbprint: async func() -> result<string, vault-error>;
                }
            }
            world sdk { import host-keys; export api; }",
        );
        let files = super::emit(&m).unwrap();
        let dispatch = &files.iter().find(|(n, _)| n == "CoreDispatch.g.cs").unwrap().1;
        let contract = &files.iter().find(|(n, _)| n == "CoreContract.g.cs").unwrap().1;
        for f in m.resources.iter().flat_map(|r| r.ctor.iter().chain(&r.methods)) {
            assert!(dispatch.contains(&format!("0x{:04x}", f.id)), "{} has no method id constant", f.name);
        }
        assert!(contract.contains("private const string Prefix = \"demo_vault_\";"));
        assert!(contract.contains("HostCallsEnabled = true"));
        assert!(contract.contains("static class HostKeys"));
        assert!(contract.contains("interface ISigner : ICoreObject"));
    }

    #[test]
    fn resources_without_constructor_are_rejected() {
        let m = model("package a:b@1.0.0;\ninterface i { resource r { f: func(); } }\nworld w { export i; }\n");
        assert!(super::emit(&m).is_err());
    }
}

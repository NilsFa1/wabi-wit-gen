//! Emits the *core-side* glue for a Java core (GraalVM native-image shared
//! library, runtime in `runtime/java-core`): one interface per resource that
//! the core implements, the `Implementation` factory, and the method-id
//! dispatcher. The contract types and their codec are the ones generated for
//! the Java SDK (same package), so SDK and core share one type definition.
//!
//! The core provides `<pkg>.Core implements Implementation` (public no-arg ctor).

use crate::gen_java::{boxed, dec, docs, enc, header, ident, jt, pkg};
use crate::ir::*;
use crate::names::*;
use anyhow::{bail, Result};

pub fn emit_core(m: &Model) -> Result<Vec<(String, String)>> {
    if m.needs_host_calls() {
        bail!("target java-core: host calls (WIT imports) are not implemented for Java cores yet");
    }
    for r in &m.resources {
        if r.ctor.is_none() {
            bail!("target java-core: resource `{}` needs a constructor", r.name);
        }
        if r.methods.iter().any(|f| matches!(f.ret, Ret::Value(Ty::Stream(_)))) {
            bail!("target java-core: streams are not supported by the Java core emitter yet (`{}`)", r.name);
        }
    }
    let dir = pkg(m).replace('.', "/");
    let mut files = vec![("wabi/core/runtime/Binding.java".to_string(), binding(m))];
    for r in &m.resources {
        files.push((format!("{dir}/{}Resource.java", pascal(&r.name)), resource(m, r)));
    }
    files.push((format!("{dir}/Implementation.java"), implementation(m)));
    files.push((format!("{dir}/CoreGlue.java"), glue(m)));
    Ok(files)
}

fn binding(m: &Model) -> String {
    let mut o = header("wabi.core.runtime");
    w!(o, "/** Binds the generic Java core runtime to package {}. */", m.contract_id());
    w!(o, "final class Binding {{");
    w!(o, "    private Binding() {{}}");
    w!(o, "");
    w!(o, "    static final CorePackage PACKAGE = new {}.CoreGlue();", pkg(m));
    w!(o, "}}");
    o
}

fn ok_ty(f: &Func) -> Option<&Ty> {
    match &f.ret {
        Ret::Unit | Ret::Result { ok: None, .. } => None,
        Ret::Value(t) | Ret::Result { ok: Some(t), .. } => Some(t),
    }
}

fn sig_ret(f: &Func) -> String {
    match (f.is_async, ok_ty(f)) {
        (true, Some(t)) => format!("java.util.concurrent.CompletionStage<{}>", boxed(t)),
        (true, None) => "java.util.concurrent.CompletionStage<Void>".into(),
        (false, Some(t)) => jt(t),
        (false, None) => "void".into(),
    }
}

fn params(f: &Func) -> String {
    f.params.iter().map(|(n, t)| format!("{} {}", jt(t), ident(n))).collect::<Vec<_>>().join(", ")
}

fn args(f: &Func) -> String {
    f.params.iter().map(|(n, _)| ident(n)).collect::<Vec<_>>().join(", ")
}

fn error_note(f: &Func) -> Option<String> {
    match &f.ret {
        Ret::Result { err: Some(Ty::Named(n)), .. } => Some(exception_name(n)),
        _ => None,
    }
}

fn resource(m: &Model, r: &Resource) -> String {
    let mut o = header(&pkg(m));
    docs(&mut o, &r.docs, "");
    w!(o, "// Implemented by the Java core. Async methods return a CompletionStage");
    w!(o, "// (cancelled when the host cancels); domain errors are thrown / completed");
    w!(o, "// exceptionally with the generated exception. close() runs on handle drop.");
    w!(o, "public interface {}Resource extends wabi.core.runtime.CoreObject {{", pascal(&r.name));
    for f in &r.methods {
        docs(&mut o, &f.docs, "    ");
        if let Some(ex) = error_note(f) {
            w!(o, "    /** Domain errors: {{@link {ex}}}. */");
        }
        w!(o, "    {} {}({});", sig_ret(f), ident(&f.name), params(f));
    }
    w!(o, "}}");
    o
}

fn implementation(m: &Model) -> String {
    let mut o = header(&pkg(m));
    w!(o, "/**");
    w!(o, " * What a Java core for {} provides: the class {{@code {}.Core}}", m.contract_id(), pkg(m));
    w!(o, " * (public no-arg constructor) implementing this interface.");
    w!(o, " */");
    w!(o, "public interface Implementation {{");
    w!(o, "    /** Free text identifying the core implementation ({}_core_info). */", m.symbol_prefix());
    w!(o, "    String coreInfo();");
    for r in &m.resources {
        let c = r.ctor.as_ref().unwrap();
        w!(o, "");
        docs(&mut o, &c.docs, "    ");
        w!(o, "    {}Resource new{}({});", pascal(&r.name), pascal(&r.name), params(c));
    }
    w!(o, "}}");
    o
}

fn decode_params(o: &mut String, f: &Func, ind: &str) {
    w!(o, "{ind}var r = new MsgPackReader(request);");
    w!(o, "{ind}if (r.readArrayHeader() != {}) throw new AbiException(Status.INVALID_ARGUMENT, \"{}: expected {} parameters\");", f.params.len(), f.name, f.params.len());
    for (n, t) in &f.params {
        w!(o, "{ind}final {} {} = {};", jt(t), ident(n), dec(t, "r", 1));
    }
    w!(o, "{ind}if (r.remaining() != 0) throw new AbiException(Status.INVALID_ARGUMENT, \"{}: trailing bytes\");", f.name);
}

/// Statements encoding `ret` (the method's ok value) as the complete result payload into `w`.
fn encode_ok(o: &mut String, f: &Func, ind: &str) {
    match &f.ret {
        Ret::Unit => w!(o, "{ind}w.writeNil();"),
        Ret::Value(t) => w!(o, "{ind}{}", enc(t, "ret", 1)),
        Ret::Result { ok, .. } => {
            w!(o, "{ind}w.writeArrayHeader(2);");
            w!(o, "{ind}w.writeUInt(0);");
            match ok {
                Some(t) => w!(o, "{ind}{}", enc(t, "ret", 1)),
                None => w!(o, "{ind}w.writeNil();"),
            }
        }
    }
}

/// Encodes a domain error `cause` into `w` or rethrows it (unexpected failures -> WABI_ERR_INTERNAL).
fn encode_err(o: &mut String, f: &Func, ind: &str, rethrow: &str) {
    match &f.ret {
        Ret::Result { err: Some(Ty::Named(n)), .. } => {
            w!(o, "{ind}if (!(cause instanceof {} e)) {rethrow}", exception_name(n));
            w!(o, "{ind}w.writeArrayHeader(2);");
            w!(o, "{ind}w.writeUInt(1);");
            w!(o, "{ind}WireCodec.write{}(w, e.error());", pascal(n));
        }
        _ => w!(o, "{ind}{rethrow}"),
    }
}

fn glue(m: &Model) -> String {
    let mut o = header(&pkg(m));
    w!(o, "import wabi.core.runtime.AbiException;");
    w!(o, "import wabi.core.runtime.CorePackage;");
    w!(o, "import wabi.core.runtime.Handles;");
    w!(o, "import wabi.core.runtime.Status;");
    // MessagePack of the package's SDK runtime (generated/java/<pkg>/runtime).
    let rt = crate::runtime::java_package(m);
    w!(o, "import {rt}.MsgPackReader;");
    w!(o, "import {rt}.MsgPackWriter;");
    w!(o, "import {rt}.WabiProtocolException;");
    w!(o, "import java.util.concurrent.CompletableFuture;");
    w!(o, "import java.util.concurrent.CompletionException;");
    w!(o, "import java.util.concurrent.CompletionStage;");
    w!(o, "import java.util.function.Supplier;");
    w!(o, "");
    w!(o, "/** Method-id dispatcher of {} for the generic Java core runtime. */", m.contract_id());
    w!(o, "public final class CoreGlue implements CorePackage {{");
    w!(o, "    private final Implementation impl = new Core();");
    w!(o, "");
    w!(o, "    @Override");
    w!(o, "    public String contractId() {{");
    w!(o, "        return \"{}\";", m.contract_id());
    w!(o, "    }}");
    w!(o, "");
    w!(o, "    @Override");
    w!(o, "    public String coreInfo() {{");
    w!(o, "        return impl.coreInfo();");
    w!(o, "    }}");
    w!(o, "");
    w!(o, "    private static Throwable unwrap(Throwable t) {{");
    w!(o, "        while (t instanceof CompletionException && t.getCause() != null) t = t.getCause();");
    w!(o, "        return t;");
    w!(o, "    }}");
    w!(o, "");
    // An undecodable request is the caller's fault (INVALID_ARGUMENT). Translated
    // here because only the generated glue knows the package's runtime namespace.
    w!(o, "    @Override");
    w!(o, "    public byte[] dispatchSync(long object, int methodId, byte[] request) throws AbiException {{");
    w!(o, "        try {{");
    w!(o, "            return sync(object, methodId, request);");
    w!(o, "        }} catch (WabiProtocolException e) {{");
    w!(o, "            throw new AbiException(Status.INVALID_ARGUMENT, e.getMessage());");
    w!(o, "        }}");
    w!(o, "    }}");
    w!(o, "");
    w!(o, "    @Override");
    w!(o, "    public Supplier<CompletionStage<byte[]>> dispatchAsync(long object, int methodId, byte[] request) throws AbiException {{");
    w!(o, "        try {{");
    w!(o, "            return async(object, methodId, request);");
    w!(o, "        }} catch (WabiProtocolException e) {{");
    w!(o, "            throw new AbiException(Status.INVALID_ARGUMENT, e.getMessage());");
    w!(o, "        }}");
    w!(o, "    }}");
    w!(o, "");
    w!(o, "    private byte[] sync(long object, int methodId, byte[] request) throws AbiException {{");
    w!(o, "        var w = new MsgPackWriter();");
    w!(o, "        switch (methodId) {{");
    for r in &m.resources {
        let rn = pascal(&r.name);
        let c = r.ctor.as_ref().unwrap();
        w!(o, "            case MethodIds.{}_{} -> {{", screaming(&r.name), screaming(&c.name));
        w!(o, "                if (object != 0) throw new AbiException(Status.INVALID_HANDLE, \"constructor called on an object\");");
        decode_params(&mut o, c, "                ");
        w!(o, "                w.writeUInt64(Handles.add(Handles.OBJECT, impl.new{rn}({})));", args(c));
        w!(o, "            }}");
        for f in r.methods.iter().filter(|f| !f.is_async) {
            w!(o, "            case MethodIds.{}_{} -> {{", screaming(&r.name), screaming(&f.name));
            w!(o, "                final var self = Handles.object(object, {rn}Resource.class);");
            decode_params(&mut o, f, "                ");
            let call = format!("self.{}({})", ident(&f.name), args(f));
            w!(o, "                try {{");
            match ok_ty(f) {
                Some(t) => w!(o, "                    final {} ret = {call};", jt(t)),
                None => w!(o, "                    {call};"),
            }
            encode_ok(&mut o, f, "                    ");
            w!(o, "                }} catch (RuntimeException failure) {{");
            w!(o, "                    Throwable cause = failure;");
            encode_err(&mut o, f, "                    ", "throw failure;");
            w!(o, "                }}");
            w!(o, "            }}");
        }
        let asyncs: Vec<String> = r.methods.iter().filter(|f| f.is_async).map(|f| format!("MethodIds.{}_{}", screaming(&r.name), screaming(&f.name))).collect();
        if !asyncs.is_empty() {
            w!(o, "            case {} -> throw new AbiException(Status.UNSUPPORTED, \"async method called synchronously\");", asyncs.join(", "));
        }
    }
    w!(o, "            default -> throw new AbiException(Status.UNKNOWN_METHOD, \"unknown method id\");");
    w!(o, "        }}");
    w!(o, "        return w.toByteArray();");
    w!(o, "    }}");
    w!(o, "");
    w!(o, "    private Supplier<CompletionStage<byte[]>> async(long object, int methodId, byte[] request) throws AbiException {{");
    w!(o, "        switch (methodId) {{");
    for r in &m.resources {
        let rn = pascal(&r.name);
        for f in r.methods.iter().filter(|f| f.is_async) {
            w!(o, "            case MethodIds.{}_{} -> {{", screaming(&r.name), screaming(&f.name));
            w!(o, "                final var self = Handles.object(object, {rn}Resource.class);");
            decode_params(&mut o, f, "                ");
            let bt = ok_ty(f).map(boxed).unwrap_or_else(|| "Void".into());
            w!(o, "                return () -> {{");
            w!(o, "                    CompletionStage<{bt}> stage;");
            w!(o, "                    try {{");
            w!(o, "                        stage = self.{}({});", ident(&f.name), args(f));
            w!(o, "                    }} catch (RuntimeException e) {{");
            w!(o, "                        stage = CompletableFuture.failedFuture(e);");
            w!(o, "                    }}");
            w!(o, "                    return stage.handle((ret, failure) -> {{");
            w!(o, "                        var w = new MsgPackWriter();");
            w!(o, "                        if (failure == null) {{");
            encode_ok(&mut o, f, "                            ");
            w!(o, "                            return w.toByteArray();");
            w!(o, "                        }}");
            w!(o, "                        Throwable cause = unwrap(failure);");
            encode_err(&mut o, f, "                        ", "throw new CompletionException(cause);");
            w!(o, "                        return w.toByteArray();");
            w!(o, "                    }});");
            w!(o, "                }};");
            w!(o, "            }}");
        }
        let syncs: Vec<String> = r.ctor.iter().chain(r.methods.iter().filter(|f| !f.is_async)).map(|f| format!("MethodIds.{}_{}", screaming(&r.name), screaming(&f.name))).collect();
        w!(o, "            case {} -> throw new AbiException(Status.UNSUPPORTED, \"sync method called asynchronously\");", syncs.join(", "));
    }
    w!(o, "            default -> throw new AbiException(Status.UNKNOWN_METHOD, \"unknown method id\");");
    w!(o, "        }}");
    w!(o, "    }}");
    w!(o, "}}");
    o
}

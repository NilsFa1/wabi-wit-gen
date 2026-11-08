//! Language-neutral model of the WIT contract, with all wire identifiers
//! resolved through the lock file. Every language emitter works only on this.

use crate::lock::Lock;
use anyhow::{bail, Result};
use wit_parser::{
    FunctionKind, Handle, InterfaceId, Resolve, Type, TypeDefKind, TypeId, TypeOwner,
};

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Char,
    String,
    /// `list<u8>` — transported as MessagePack `bin`.
    Bytes,
    List(Box<Ty>),
    Option(Box<Ty>),
    Tuple(Vec<Ty>),
    /// A record, variant or enum declared in the contract.
    Named(String),
    /// `stream<T>` — transported as a stream handle (u64).
    Stream(Box<Ty>),
    /// A resource of this package as a value: `own<T>` (also a bare `T`) moves
    /// the object, `borrow<T>` lends it for the call. Transported as its
    /// object handle (u64). Only as a parameter, a result or the ok arm of a
    /// result (see `check_handles`).
    Handle { resource: String, owned: bool },
}

impl Ty {
    /// The type as it travels: what `abi-ids.lock` and the API baseline record.
    pub fn shape(&self) -> String {
        match self {
            Ty::Bool => "bool".into(), Ty::U8 => "u8".into(), Ty::U16 => "u16".into(),
            Ty::U32 => "u32".into(), Ty::U64 => "u64".into(), Ty::S8 => "s8".into(),
            Ty::S16 => "s16".into(), Ty::S32 => "s32".into(), Ty::S64 => "s64".into(),
            Ty::F32 => "f32".into(), Ty::F64 => "f64".into(), Ty::Char => "char".into(),
            Ty::String => "string".into(), Ty::Bytes => "bytes".into(),
            Ty::Named(n) => format!("named<{n}>"),
            Ty::List(t) => format!("list<{}>", t.shape()),
            Ty::Option(t) => format!("option<{}>", t.shape()),
            Ty::Stream(t) => format!("stream<{}>", t.shape()),
            Ty::Tuple(ts) => format!("tuple<{}>", ts.iter().map(Ty::shape).collect::<Vec<_>>().join(",")),
            Ty::Handle { resource, owned } => format!("{}<{resource}>", if *owned { "own" } else { "borrow" }),
        }
    }

    fn contains_handle(&self) -> bool {
        match self {
            Ty::Handle { .. } => true,
            Ty::List(t) | Ty::Option(t) | Ty::Stream(t) => t.contains_handle(),
            Ty::Tuple(ts) => ts.iter().any(Ty::contains_handle),
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub tag: u32,
    pub ty: Ty,
    pub docs: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Case {
    pub name: String,
    pub tag: u32,
    pub ty: Option<Ty>,
    pub docs: Option<String>,
}

#[derive(Debug, Clone)]
pub enum TypeKind {
    Record(Vec<Field>),
    Variant(Vec<Case>),
    /// Enum cases never carry a payload (`Case::ty` is always `None`).
    Enum(Vec<Case>),
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: String,
    pub kind: TypeKind,
    pub docs: Option<String>,
    /// Used as the error arm of some `result<_, E>`: gets an exception wrapper.
    pub is_error: bool,
}

#[derive(Debug, Clone)]
pub enum Ret {
    Unit,
    Value(Ty),
    Result { ok: Option<Ty>, err: Option<Ty> },
}

impl Ret {
    pub fn shape(&self) -> String {
        let arm = |t: &Option<Ty>| t.as_ref().map(Ty::shape).unwrap_or_else(|| "_".into());
        match self {
            Ret::Unit => "_".into(),
            Ret::Value(t) => t.shape(),
            Ret::Result { ok, err } => format!("result<{},{}>", arm(ok), arm(err)),
        }
    }
}

/// Wire shape of a function: an id in `abi-ids.lock` stands for exactly this.
/// Parameter names are not part of it (parameters travel by position).
fn func_shape(is_async: bool, params: &[(String, Ty)], ret: &Ret) -> String {
    format!(
        "{}({})->{}",
        if is_async { "async" } else { "sync" },
        params.iter().map(|(_, t)| t.shape()).collect::<Vec<_>>().join(","),
        ret.shape()
    )
}

#[derive(Debug, Clone)]
pub struct Func {
    pub name: String,
    pub id: u32,
    pub is_async: bool,
    pub params: Vec<(String, Ty)>,
    pub ret: Ret,
    pub docs: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Resource {
    pub name: String,
    pub group: u32,
    pub ctor: Option<Func>,
    pub methods: Vec<Func>,
    pub docs: Option<String>,
}

impl Resource {
    /// Resources that functions of this one take or return as values.
    pub fn handle_resources(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        let mut add = |t: &Ty| {
            if let Ty::Handle { resource, .. } = t {
                if !out.contains(resource) {
                    out.push(resource.clone());
                }
            }
        };
        for f in self.ctor.iter().chain(&self.methods) {
            f.params.iter().for_each(|(_, t)| add(t));
            match &f.ret {
                Ret::Value(t) | Ret::Result { ok: Some(t), .. } => add(t),
                _ => {}
            }
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct Model {
    pub namespace: String,
    pub package: String,
    pub version: String,
    pub types: Vec<TypeDecl>,
    pub resources: Vec<Resource>,
    /// WIT interfaces the world IMPORTS: implemented by the host (app), called
    /// by the core through the ABI 1.1 host calls.
    pub imports: Vec<HostInterface>,
}

/// An imported interface: freestanding functions the APP implements.
#[derive(Debug, Clone)]
pub struct HostInterface {
    pub name: String,
    pub group: u32,
    pub funcs: Vec<Func>,
    pub docs: Option<String>,
}

thread_local! {
    /// (namespace, package) of the model built last on this thread: the type
    /// mapping functions of the emitters have no model at hand.
    static CURRENT_PACKAGE: std::cell::RefCell<(String, String)> = const { std::cell::RefCell::new((String::new(), String::new())) };
}

/// SDK class of resource `name` in the model built last (`Model::class_of`).
pub fn sdk_class(name: &str) -> String {
    use crate::names::pascal;
    CURRENT_PACKAGE.with(|p| {
        let (namespace, package) = &*p.borrow();
        if name == package { format!("{}{}", pascal(namespace), pascal(name)) } else { pascal(name) }
    })
}

impl Model {
    /// SDK class of a resource: `wabi:client` + resource `client` -> `WabiClient`
    /// (qualified only when the resource is named like its package), otherwise
    /// just the resource name (`key-store` -> `KeyStore`).
    pub fn class_of(&self, resource: &str) -> String {
        use crate::names::pascal;
        if resource == self.package { format!("{}{}", pascal(&self.namespace), pascal(resource)) } else { pascal(resource) }
    }

    #[cfg(test)]
    pub fn resource(&self, name: &str) -> &Resource {
        self.resources.iter().find(|r| r.name == name).expect("resource handles were checked")
    }

    /// Host calls need ABI 1.2 + WABI_CAP_HOST_CALLS_2 (the SDKs register with
    /// host_register2); everything else runs on 1.0.
    pub fn needs_host_calls(&self) -> bool {
        !self.imports.is_empty()
    }

    /// Minimum ABI minor version an SDK for this package requires from a core.
    pub fn required_abi_minor(&self) -> u32 {
        if self.needs_host_calls() { 2 } else { 0 }
    }

    /// `wabi:client@1.0.0`
    pub fn contract_id(&self) -> String {
        format!("{}:{}@{}", self.namespace, self.package, self.version)
    }

    /// Symbol prefix of this package's ABI exports and library name:
    /// `wabi:client` -> `wabi_client` (`wabi_client_call_sync`, `libwabi_client.so`).
    /// Unique per WIT package, so several packages can share one process/binary.
    pub fn symbol_prefix(&self) -> String {
        format!("{}_{}", crate::names::snake(&self.namespace), crate::names::snake(&self.package))
    }

    pub fn version_triple(&self) -> (u32, u32, u32) {
        let mut it = self.version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
        (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
    }

}

pub fn build(resolve: &Resolve, pkg: wit_parser::PackageId, lock: &mut Lock) -> Result<Model> {
    let package = &resolve.packages[pkg];
    let pname = &package.name;
    let prefix = format!("{}:{}", pname.namespace, pname.name);
    let mut model = Model {
        namespace: pname.namespace.clone(),
        package: pname.name.clone(),
        version: pname.version.as_ref().map(|v| v.to_string()).unwrap_or_else(|| "0.0.0".into()),
        types: vec![],
        resources: vec![],
        imports: vec![],
    };

    // Interfaces of this package that a world imports (and that have functions)
    // are implemented by the host, not by the core.
    let mut imported = std::collections::HashSet::new();
    for wid in package.worlds.values() {
        for item in resolve.worlds[*wid].imports.values() {
            if let wit_parser::WorldItem::Interface { id, .. } = item {
                if resolve.interfaces[*id].package == Some(pkg) && !resolve.interfaces[*id].functions.is_empty() {
                    imported.insert(*id);
                }
            }
        }
    }

    let mut error_types = vec![];
    let mut resource_indices = std::collections::HashMap::new();

    for (iface_name, &iid) in &package.interfaces {
        let iface = &resolve.interfaces[iid];
        let q = |rest: &str| format!("{prefix}/{iface_name}.{rest}");

        for (tname, &tid) in &iface.types {
            let td = &resolve.types[tid];
            if td.owner != TypeOwner::Interface(iid) {
                continue;
            }
            let docs = td.docs.contents.clone();
            let kind = match &td.kind {
                TypeDefKind::Record(r) => TypeKind::Record(
                    r.fields
                        .iter()
                        .map(|f| {
                            let ty = ty(resolve, &f.ty)?;
                            Ok(Field {
                                name: f.name.clone(),
                                tag: lock.member("field", &q(&format!("{tname}.{}", f.name)), &ty.shape()),
                                ty,
                                docs: f.docs.contents.clone(),
                            })
                        })
                        .collect::<Result<_>>()?,
                ),
                TypeDefKind::Variant(v) => TypeKind::Variant(
                    v.cases
                        .iter()
                        .map(|c| {
                            let ty = c.ty.as_ref().map(|t| ty(resolve, t)).transpose()?;
                            Ok(Case {
                                name: c.name.clone(),
                                tag: lock.member("case", &q(&format!("{tname}.{}", c.name)), &ty.as_ref().map(Ty::shape).unwrap_or_else(|| "_".into())),
                                ty,
                                docs: c.docs.contents.clone(),
                            })
                        })
                        .collect::<Result<_>>()?,
                ),
                TypeDefKind::Enum(e) => TypeKind::Enum(
                    e.cases
                        .iter()
                        .map(|c| Case {
                            name: c.name.clone(),
                            tag: lock.member("case", &q(&format!("{tname}.{}", c.name)), "_"),
                            ty: None,
                            docs: c.docs.contents.clone(),
                        })
                        .collect(),
                ),
                TypeDefKind::Resource => {
                    let group = lock.group(&q(tname));
                    resource_indices.insert(tid, model.resources.len());
                    model.resources.push(Resource {
                        name: tname.clone(),
                        group,
                        ctor: None,
                        methods: vec![],
                        docs,
                    });
                    continue;
                }
                TypeDefKind::Type(_) => continue, // alias created by `use`
                other => bail!("type `{tname}`: {} is not supported by this generator", other.as_str()),
            };
            model.types.push(TypeDecl { name: tname.clone(), kind, docs, is_error: false });
        }

        if imported.contains(&iid) {
            let group = lock.group(&format!("{prefix}/{iface_name}"));
            let mut funcs = vec![];
            for func in iface.functions.values() {
                if func.kind.resource().is_some() {
                    bail!("imported interface `{iface_name}`: resources are not supported for host calls yet");
                }
                let item = func.item_name().to_string();
                let params = func
                    .params
                    .iter()
                    .map(|p| Ok((p.name.clone(), ty(resolve, &p.ty)?)))
                    .collect::<Result<Vec<_>>>()?;
                let ret = ret_of(resolve, func, &mut error_types)?;
                let id = lock.method(group, &q(&item), false, &func_shape(func.kind.is_async(), &params, &ret))?;
                if matches!(ret, Ret::Value(Ty::Stream(_))) {
                    bail!("`{}`: streams are not supported for host calls yet", func.name);
                }
                funcs.push(Func { name: item, id, is_async: func.kind.is_async(), params, ret, docs: func.docs.contents.clone() });
            }
            model.imports.push(HostInterface { name: iface_name.clone(), group, funcs, docs: iface.docs.contents.clone() });
            continue;
        }

        for func in iface.functions.values() {
            let rid = match func.kind.resource() {
                Some(rid) => rid,
                None => bail!("freestanding function `{}`: only resource members are supported", func.name),
            };
            let rname = resource_name(resolve, rid, iid)?;
            let item = func.item_name().to_string();
            let qname = q(&format!("{rname}.{item}"));
            let index = *resource_indices.get(&rid)
                .ok_or_else(|| anyhow::anyhow!("resource {rname} has no declaration in this interface"))?;
            let res = &mut model.resources[index];
            let is_ctor = matches!(func.kind, FunctionKind::Constructor(_));
            let is_method = matches!(func.kind, FunctionKind::Method(_) | FunctionKind::AsyncMethod(_));
            let group = res.group;
            let params = func
                .params
                .iter()
                .skip(if is_method { 1 } else { 0 }) // implicit `self: borrow<r>`
                .map(|p| Ok((p.name.clone(), ty(resolve, &p.ty)?)))
                .collect::<Result<Vec<_>>>()?;
            if matches!(func.kind, FunctionKind::Static(_) | FunctionKind::AsyncStatic(_)) {
                bail!("static function `{}` is not supported by this generator yet", func.name);
            }
            let ret = if is_ctor { Ret::Unit } else { ret_of(resolve, func, &mut error_types)? };
            let id = lock.method(group, &qname, is_ctor, &func_shape(func.kind.is_async(), &params, &ret))?;
            if let Ret::Value(Ty::Stream(_)) = ret {
                if func.kind.is_async() {
                    bail!("`{}`: return a stream from a sync func (the stream itself is async)", func.name);
                }
            }
            let f = Func {
                name: item,
                id,
                is_async: func.kind.is_async(),
                params,
                ret,
                docs: func.docs.contents.clone(),
            };
            if is_ctor {
                res.ctor = Some(f);
            } else {
                res.methods.push(f);
            }
        }
    }

    for t in &mut model.types {
        t.is_error = error_types.contains(&t.name);
    }
    validate_names(&model)?;
    check_handles(&model)?;
    CURRENT_PACKAGE.with(|p| *p.borrow_mut() = (model.namespace.clone(), model.package.clone()));
    lock.retire_untouched();
    Ok(model)
}

fn ret_of(resolve: &Resolve, func: &wit_parser::Function, error_types: &mut Vec<String>) -> Result<Ret> {
    Ok(match &func.result {
        None => Ret::Unit,
        Some(Type::Id(t)) if matches!(resolve.types[*t].kind, TypeDefKind::Result(_)) => {
            let TypeDefKind::Result(r) = &resolve.types[*t].kind else { unreachable!() };
            let err = r.err.as_ref().map(|t| ty(resolve, t)).transpose()?;
            if let Some(Ty::Named(n)) = &err {
                error_types.push(n.clone());
            }
            Ret::Result { ok: r.ok.as_ref().map(|t| ty(resolve, t)).transpose()?, err }
        }
        Some(t) => Ret::Value(ty(resolve, t)?),
    })
}

fn resource_name(resolve: &Resolve, mut id: TypeId, iid: InterfaceId) -> Result<String> {
    loop {
        let td = &resolve.types[id];
        match &td.kind {
            TypeDefKind::Type(Type::Id(inner)) => id = *inner,
            TypeDefKind::Resource => {
                if td.owner != TypeOwner::Interface(iid) {
                    bail!("resources must be defined in the interface that declares their functions");
                }
                return Ok(td.name.clone().unwrap());
            }
            _ => bail!("expected resource"),
        }
    }
}

fn ty(resolve: &Resolve, t: &Type) -> Result<Ty> {
    Ok(match t {
        Type::Bool => Ty::Bool,
        Type::U8 => Ty::U8,
        Type::U16 => Ty::U16,
        Type::U32 => Ty::U32,
        Type::U64 => Ty::U64,
        Type::S8 => Ty::S8,
        Type::S16 => Ty::S16,
        Type::S32 => Ty::S32,
        Type::S64 => Ty::S64,
        Type::F32 => Ty::F32,
        Type::F64 => Ty::F64,
        Type::Char => Ty::Char,
        Type::String => Ty::String,
        Type::ErrorContext => bail!("error-context is not supported"),
        Type::Id(id) => {
            let td = &resolve.types[*id];
            match &td.kind {
                TypeDefKind::Type(inner) => ty(resolve, inner)?,
                TypeDefKind::Record(_) | TypeDefKind::Variant(_) | TypeDefKind::Enum(_) => {
                    Ty::Named(td.name.clone().expect("named type"))
                }
                TypeDefKind::List(Type::U8) => Ty::Bytes,
                TypeDefKind::List(inner) => Ty::List(Box::new(ty(resolve, inner)?)),
                TypeDefKind::Option(inner) => {
                    let inner = ty(resolve, inner)?;
                    if matches!(inner, Ty::Option(_)) {
                        bail!("option<option<T>> is not supported (ambiguous on the wire)");
                    }
                    Ty::Option(Box::new(inner))
                }
                TypeDefKind::Tuple(tp) => {
                    Ty::Tuple(tp.types.iter().map(|t| ty(resolve, t)).collect::<Result<_>>()?)
                }
                TypeDefKind::Stream(Some(inner)) => Ty::Stream(Box::new(ty(resolve, inner)?)),
                TypeDefKind::Stream(None) => bail!("stream without element type is not supported"),
                TypeDefKind::Future(_) => bail!(
                    "future<T> values are not supported yet; use `async func` (same future-handle mechanism)"
                ),
                TypeDefKind::Handle(Handle::Own(r)) => Ty::Handle { resource: handle_resource(resolve, *r)?, owned: true },
                TypeDefKind::Handle(Handle::Borrow(r)) => Ty::Handle { resource: handle_resource(resolve, *r)?, owned: false },
                TypeDefKind::Resource => Ty::Handle { resource: td.name.clone().expect("named resource"), owned: true },
                other => bail!("{} is not supported by this generator", other.as_str()),
            }
        }
    })
}

/// Name of the resource a handle type refers to.
fn handle_resource(resolve: &Resolve, mut id: TypeId) -> Result<String> {
    loop {
        let td = &resolve.types[id];
        match &td.kind {
            TypeDefKind::Type(Type::Id(inner)) => id = *inner,
            TypeDefKind::Resource => return Ok(td.name.clone().expect("named resource")),
            _ => bail!("expected a resource"),
        }
    }
}

/// Resource handles travel as plain object handles. They are supported where
/// their ownership is unambiguous: as a parameter of an exported function, as
/// its result, or as the ok arm of its result.
fn check_handles(m: &Model) -> Result<()> {
    for t in &m.types {
        let inside = match &t.kind {
            TypeKind::Record(fields) => fields.iter().any(|f| f.ty.contains_handle()),
            TypeKind::Variant(cases) | TypeKind::Enum(cases) => cases.iter().any(|c| c.ty.as_ref().is_some_and(Ty::contains_handle)),
        };
        if inside {
            bail!("type `{}`: resource handles inside records and variants are not supported", t.name);
        }
    }
    for h in &m.imports {
        for f in &h.funcs {
            let ret = match &f.ret {
                Ret::Unit => false,
                Ret::Value(t) => t.contains_handle(),
                Ret::Result { ok, err } => ok.iter().chain(err).any(Ty::contains_handle),
            };
            if ret || f.params.iter().any(|(_, t)| t.contains_handle()) {
                bail!("`{}.{}`: resource handles are not supported for host calls yet", h.name, f.name);
            }
        }
    }
    let direct = |t: &Ty| matches!(t, Ty::Handle { .. }) || !t.contains_handle();
    for r in &m.resources {
        for f in r.ctor.iter().chain(&r.methods) {
            let at = format!("`{}.{}`", r.name, f.name);
            for (name, t) in &f.params {
                if !direct(t) {
                    bail!("{at}, parameter `{name}`: a resource handle must be the whole parameter (not inside list, option or tuple)");
                }
                if let Ty::Handle { resource, .. } = t {
                    if !m.resources.iter().any(|r| &r.name == resource) {
                        bail!("{at}: resource `{resource}` is not exported by this package");
                    }
                }
            }
            let (ok, err) = match &f.ret {
                Ret::Unit => (None, None),
                Ret::Value(t) => (Some(t), None),
                Ret::Result { ok, err } => (ok.as_ref(), err.as_ref()),
            };
            if err.is_some_and(Ty::contains_handle) || ok.is_some_and(|t| !direct(t)) {
                bail!("{at}: a resource handle must be the whole result or the whole ok arm of the result");
            }
            if let Some(Ty::Handle { resource, owned }) = ok {
                if !owned {
                    bail!("{at}: a result cannot be borrowed (use `{resource}`, not `borrow<{resource}>`)");
                }
                if !m.resources.iter().any(|r| &r.name == resource) {
                    bail!("{at}: resource `{resource}` is not exported by this package");
                }
            }
        }
    }
    Ok(())
}

fn validate_names(m: &Model) -> Result<()> {
    use crate::names::{camel, exception_name, pascal, snake};
    use std::collections::HashMap;

    fn unique<'a>(scope: &str, names: impl IntoIterator<Item = &'a str>) -> Result<()> {
        let names: Vec<_> = names.into_iter().collect();
        for convert in [pascal as fn(&str) -> String, camel, snake] {
            let mut seen = HashMap::new();
            for &name in &names {
                let target = convert(name);
                if let Some(previous) = seen.insert(target.clone(), name) {
                    bail!("name collision in {scope}: {previous} and {name} both generate {target}");
                }
            }
        }
        Ok(())
    }

    // Emitters flatten WIT interfaces into one package. Reject collisions
    // before any output is written instead of binding by an unqualified name.
    let mut seen = HashMap::new();
    let mut insert = |name: String, source: String| -> Result<()> {
        if let Some(previous) = seen.insert(name.clone(), source.clone()) {
            bail!("generated type name collision: {previous} and {source} both generate {name}");
        }
        Ok(())
    };
    for t in &m.types { insert(pascal(&t.name), format!("type {}", t.name))?; }
    for r in &m.resources {
        insert(pascal(&r.name), format!("resource {}", r.name))?;
        let sdk = crate::gen_csharp::class_name(m, r);
        if sdk != pascal(&r.name) { insert(sdk, format!("SDK resource {}", r.name))?; }
    }
    for h in &m.imports { insert(pascal(&h.name), format!("host interface {}", h.name))?; }
    for t in &m.types {
        if t.is_error { insert(exception_name(&t.name), format!("error wrapper {}", t.name))?; }
    }
    for name in ["WireCodec", "MethodIds", "Contract", "NativeAbi", "AbiConstants"] {
        insert(name.to_owned(), "generated support type".to_owned())?;
    }
    if !m.imports.is_empty() { insert(format!("{}Host", pascal(&m.package)), "host dispatcher".to_owned())?; }

    for t in &m.types {
        match &t.kind {
            TypeKind::Record(fields) => unique(&t.name, fields.iter().map(|f| f.name.as_str()))?,
            TypeKind::Variant(cases) | TypeKind::Enum(cases) => unique(&t.name, cases.iter().map(|c| c.name.as_str()))?,
        }
    }
    for r in &m.resources {
        let methods: Vec<_> = r.methods.iter().map(|f| f.name.as_str()).collect();
        unique(&r.name, methods.iter().copied())?;
        // C#/Java add Async to asynchronous methods.
        let mut seen = HashMap::new();
        for f in &r.methods {
            let target = format!("{}{}", pascal(&f.name), if f.is_async { "Async" } else { "" });
            if let Some(old) = seen.insert(target.clone(), &f.name) {
                bail!("method name collision in {}: {old} and {} generate {target}", r.name, f.name);
            }
        }
    }
    for h in &m.imports { unique(&h.name, h.funcs.iter().map(|f| f.name.as_str()))?; }
    for f in m.resources.iter().flat_map(|r| r.ctor.iter().chain(&r.methods))
        .chain(m.imports.iter().flat_map(|h| &h.funcs)) {
        unique(&f.name, f.params.iter().map(|(name, _)| name.as_str()))?;
    }
    Ok(())
}

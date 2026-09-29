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

#[derive(Debug, Clone)]
pub struct Model {
    pub namespace: String,
    pub package: String,
    pub version: String,
    pub types: Vec<TypeDecl>,
    pub resources: Vec<Resource>,
}

impl Model {
    /// `acme:client@1.0.0`
    pub fn contract_id(&self) -> String {
        format!("{}:{}@{}", self.namespace, self.package, self.version)
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
    };

    let mut error_types = vec![];

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
                            Ok(Field {
                                name: f.name.clone(),
                                tag: lock.member("field", &q(&format!("{tname}.{}", f.name))),
                                ty: ty(resolve, &f.ty)?,
                                docs: f.docs.contents.clone(),
                            })
                        })
                        .collect::<Result<_>>()?,
                ),
                TypeDefKind::Variant(v) => TypeKind::Variant(
                    v.cases
                        .iter()
                        .map(|c| {
                            Ok(Case {
                                name: c.name.clone(),
                                tag: lock.member("case", &q(&format!("{tname}.{}", c.name))),
                                ty: c.ty.as_ref().map(|t| ty(resolve, t)).transpose()?,
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
                            tag: lock.member("case", &q(&format!("{tname}.{}", c.name))),
                            ty: None,
                            docs: c.docs.contents.clone(),
                        })
                        .collect(),
                ),
                TypeDefKind::Resource => {
                    let group = lock.group(&q(tname));
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

        for func in iface.functions.values() {
            let rid = match func.kind.resource() {
                Some(rid) => rid,
                None => bail!("freestanding function `{}`: only resource members are supported", func.name),
            };
            let rname = resource_name(resolve, rid, iid)?;
            let item = func.item_name().to_string();
            let qname = q(&format!("{rname}.{item}"));
            let res = model
                .resources
                .iter_mut()
                .find(|r| r.name == rname)
                .expect("resource declared before its functions");
            let is_ctor = matches!(func.kind, FunctionKind::Constructor(_));
            let is_method = matches!(func.kind, FunctionKind::Method(_) | FunctionKind::AsyncMethod(_));
            let id = lock.method(res.group, &qname, is_ctor)?;
            let params = func
                .params
                .iter()
                .skip(if is_method { 1 } else { 0 }) // implicit `self: borrow<r>`
                .map(|p| Ok((p.name.clone(), ty(resolve, &p.ty)?)))
                .collect::<Result<Vec<_>>>()?;
            if matches!(func.kind, FunctionKind::Static(_) | FunctionKind::AsyncStatic(_)) {
                bail!("static function `{}` is not supported by this generator yet", func.name);
            }
            let ret = if is_ctor {
                Ret::Unit
            } else {
                match &func.result {
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
                }
            };
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
    lock.retire_untouched();
    Ok(model)
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
                TypeDefKind::Handle(Handle::Own(_) | Handle::Borrow(_)) => {
                    bail!("resource handles as values are not supported yet")
                }
                other => bail!("{} is not supported by this generator", other.as_str()),
            }
        }
    })
}

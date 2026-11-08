//! Released API baseline, separate from the append-only numeric ID lock.
//! Defaults to wit/api-baseline.snapshot; --api-baseline can point at the
//! snapshot from an actual release. Updating it is an explicit release step.
use crate::ir::{Func, Model, Ty, TypeKind};
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;

pub struct Snapshot {
    contract: String,
    entries: BTreeMap<String, String>,
}

fn ty(t: &Ty) -> String {
    t.shape()
}

fn signature(f: &Func) -> String {
    format!("{}:{}({})->{}", f.id, if f.is_async { "async" } else { "sync" },
        f.params.iter().map(|(n, t)| format!("{n}:{}", ty(t))).collect::<Vec<_>>().join(","),
        f.ret.shape())
}

impl Snapshot {
    pub fn of(m: &Model) -> Self {
        let mut entries = BTreeMap::new();
        for t in &m.types {
            let kind = match &t.kind { TypeKind::Record(_) => "record", TypeKind::Variant(_) => "variant", TypeKind::Enum(_) => "enum" };
            entries.insert(format!("type/{}", t.name), kind.into());
            match &t.kind {
                TypeKind::Record(fields) => for f in fields {
                    entries.insert(format!("field/{}.{}", t.name, f.name), format!("{}:{}", f.tag, ty(&f.ty)));
                },
                TypeKind::Variant(cases) | TypeKind::Enum(cases) => for c in cases {
                    entries.insert(format!("case/{}.{}", t.name, c.name), format!("{}:{}", c.tag, c.ty.as_ref().map(ty).unwrap_or_else(|| "_".into())));
                },
            }
        }
        for r in &m.resources {
            entries.insert(format!("resource/{}", r.name), r.group.to_string());
            for f in r.ctor.iter().chain(&r.methods) {
                entries.insert(format!("method/{}.{}", r.name, f.name), signature(f));
            }
        }
        for h in &m.imports {
            entries.insert(format!("import/{}", h.name), h.group.to_string());
            for f in &h.funcs {
                entries.insert(format!("host/{}.{}", h.name, f.name), signature(f));
            }
        }
        Self { contract: m.contract_id(), entries }
    }

    pub fn render(&self) -> String {
        let mut text = format!("# Released API baseline v1; update only as an explicit release step.\ncontract\t{}\n", self.contract);
        for (key, value) in &self.entries { text.push_str(&format!("{key}\t{value}\n")); }
        text
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut contract = None;
        let mut entries = BTreeMap::new();
        for line in text.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let (key, value) = line.split_once('\t').context("API baseline: expected tab-separated key and value")?;
            if key == "contract" {
                if contract.replace(value.to_owned()).is_some() { bail!("duplicate contract in API baseline"); }
            } else if entries.insert(key.to_owned(), value.to_owned()).is_some() {
                bail!("duplicate API baseline entry {key}");
            }
        }
        Ok(Self { contract: contract.context("API baseline has no contract")?, entries })
    }

    pub fn ensure_compatible(&self, baseline: &Self) -> Result<()> {
        fn version(id: &str) -> Result<(&str, (u32, u32, u32))> {
            let (package, v) = id.rsplit_once('@').context("API contract needs a version")?;
            let parts = v.split('.').map(str::parse::<u32>).collect::<std::result::Result<Vec<_>, _>>()?;
            if parts.len() != 3 { bail!("API baseline needs a three-part release version"); }
            Ok((package, (parts[0], parts[1], parts[2])))
        }
        let (pkg, new) = version(&self.contract)?;
        let (old_pkg, old) = version(&baseline.contract)?;
        if pkg != old_pkg { bail!("API baseline is for {old_pkg}, current package is {pkg}"); }
        if new < old { bail!("contract version cannot go backwards: {} -> {}", baseline.contract, self.contract); }
        let mut breaking = vec![];
        for (key, value) in &baseline.entries {
            match self.entries.get(key) {
                None => breaking.push(format!("removed {key}")),
                Some(v) if v != value => breaking.push(format!("changed {key}: {value} -> {v}")),
                _ => {}
            }
        }
        let mut added = false;
        for (key, value) in &self.entries {
            if baseline.entries.contains_key(key) { continue; }
            added = true;
            // A new required host function breaks existing app implementations.
            if key.starts_with("host/") || key.starts_with("import/") {
                breaking.push(format!("new required {key}"));
            }
            if let Some(field) = key.strip_prefix("field/") {
                let (parent, _) = field.rsplit_once('.').context("invalid field baseline key")?;
                if baseline.entries.get(&format!("type/{parent}")).map(String::as_str) == Some("record")
                    && !value.split_once(':').map(|(_, t)| t.starts_with("option<")).unwrap_or(false) {
                    breaking.push(format!("new required {key}"));
                }
            }
        }
        if !breaking.is_empty() && new.0 == old.0 {
            bail!("breaking API changes require a major version increase:\n  {}", breaking.join("\n  "));
        }
        if added && breaking.is_empty() && new.0 == old.0 && new.1 == old.1 {
            bail!("API additions require a minor version increase");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot(version: &str, entries: &str) -> Snapshot {
        Snapshot::parse(&format!("contract\ttest:api@{version}\n{entries}")).unwrap()
    }
    #[test]
    fn signature_changes_and_removals_need_a_major() {
        let old = snapshot("1.0.0", "method/client.get\t257:async(id:string)->string\n");
        for text in ["method/client.get\t257:async(id:u32)->string\n", "method/client.get\t257:sync(id:string)->string\n", "method/client.get\t257:async(id:string,limit:u32)->string\n", ""] {
            assert!(snapshot("1.1.0", text).ensure_compatible(&old).is_err());
            assert!(snapshot("2.0.0", text).ensure_compatible(&old).is_ok());
        }
    }
    #[test]
    fn only_optional_new_fields_are_minor_compatible() {
        let old = snapshot("1.0.0", "type/customer\trecord\nfield/customer.id\t1:string\n");
        let optional = "type/customer\trecord\nfield/customer.id\t1:string\nfield/customer.email\t2:option<string>\n";
        assert!(snapshot("1.1.0", optional).ensure_compatible(&old).is_ok());
        assert!(snapshot("1.0.1", optional).ensure_compatible(&old).is_err());
        assert!(snapshot("1.1.0", &optional.replace("option<string>", "string")).ensure_compatible(&old).is_err());
        assert!(snapshot("1.1.0", "type/customer\trecord\nfield/customer.id\t1:u32\n").ensure_compatible(&old).is_err());
    }
    #[test]
    fn new_types_cases_and_methods_are_additive_but_host_methods_are_not() {
        let old = snapshot("1.0.0", "type/event\tenum\ncase/event.ready\t1:_\n");
        let new = "type/event\tenum\ncase/event.ready\t1:_\ncase/event.closed\t2:_\ntype/options\trecord\nfield/options.name\t1:string\nmethod/client.ping\t258:sync()->_\n";
        assert!(snapshot("1.1.0", new).ensure_compatible(&old).is_ok());
        assert!(snapshot("1.1.0", &(new.to_owned() + "host/keys.sign\t513:sync()->bytes\n")).ensure_compatible(&old).is_err());
    }
    #[test]
    fn ordering_and_roundtrip_do_not_change_the_contract() {
        let a = snapshot("1.0.0", "type/z\tenum\ntype/a\trecord\n");
        let b = snapshot("1.0.0", "type/a\trecord\ntype/z\tenum\n");
        assert_eq!(a.render(), b.render());
        assert!(Snapshot::parse(&a.render()).unwrap().ensure_compatible(&b).is_ok());
    }
}

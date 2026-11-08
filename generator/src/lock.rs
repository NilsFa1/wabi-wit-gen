//! `abi-ids.lock`: the append-only table of every numeric wire identifier.
//!
//! IDs are *never* derived from declaration order. The first time the generator
//! sees an item it allocates the next free number in the item's scope and
//! records it here; from then on the lock file is authoritative. Items that
//! disappear from the WIT are kept as `retired` so their number is never
//! reused for something else.
//!
//! An id stands for one wire shape. If the types of a method, field or case
//! change, the item gets a new id and the old one is retired: a peer built
//! from the old contract then fails cleanly (unknown method, missing field,
//! unknown case) instead of decoding bytes of another shape.

use anyhow::{bail, Context, Result};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub kind: String,
    pub name: String,
    pub id: u32,
    /// Wire shape the id stands for (methods: sync/async, parameter and result
    /// types; fields and cases: their type). `None` for groups and for entries
    /// written before shapes were recorded.
    pub shape: Option<String>,
    pub retired: bool,
}

#[derive(Debug, Default)]
pub struct Lock {
    entries: Vec<Entry>,
    touched: HashSet<usize>,
}

pub const MAX_METHODS_PER_GROUP: u32 = 0xFF;

impl Lock {
    pub fn parse(text: &str) -> Result<Lock> {
        let mut lock = Lock::default();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap().trim();
            if line.is_empty() {
                continue;
            }
            let cols: Vec<&str> = line.split_whitespace().collect();
            let (shape, retired) = match cols[..] {
                [_, _, _] => (None, false),
                [_, _, _, "retired"] => (None, true),
                [_, _, _, shape] => (Some(shape), false),
                [_, _, _, shape, "retired"] => (Some(shape), true),
                _ => bail!("abi-ids.lock:{}: expected `<kind> <name> <id> [<shape>] [retired]`", n + 1),
            };
            let id = parse_id(cols[2]).with_context(|| format!("abi-ids.lock:{}", n + 1))?;
            let e = Entry { kind: cols[0].to_string(), name: cols[1].to_string(), id, shape: shape.map(String::from), retired };
            if !e.retired && lock.entries.iter().any(|o| !o.retired && o.kind == e.kind && o.name == e.name) {
                bail!("abi-ids.lock:{}: duplicate entry {} {}", n + 1, e.kind, e.name);
            }
            lock.entries.push(e);
        }
        lock.validate()?;
        Ok(lock)
    }

    fn validate(&self) -> Result<()> {
        let mut seen = HashSet::new();
        for e in &self.entries {
            let scope = match e.kind.as_str() {
                "group" | "method" => e.kind.clone(),
                "field" | "case" => format!("member:{}", parent(&e.name)),
                k => bail!("abi-ids.lock: unknown kind `{k}`"),
            };
            if !seen.insert((scope, e.id)) {
                bail!("abi-ids.lock: id {:#x} used twice ({} {})", e.id, e.kind, e.name);
            }
        }
        Ok(())
    }

    /// The entry for `name` with this wire shape. An id stands for one shape
    /// only: if the shape changed, the old entry stays behind (it is retired at
    /// the end of the run) and a new id is allocated, so that a peer built from
    /// the old contract can never decode the new shape by accident. Going back
    /// to an earlier shape brings its id back.
    fn get_or_insert(&mut self, kind: &str, name: &str, shape: Option<&str>, alloc: impl FnOnce(&Lock) -> u32) -> u32 {
        let named = |e: &Entry| e.kind == kind && e.name == name;
        let found = self
            .entries
            .iter()
            .position(|e| named(e) && e.shape.as_deref() == shape)
            // written before shapes were recorded: adopt the current shape
            .or_else(|| self.entries.iter().position(|e| named(e) && e.shape.is_none() && !e.retired));
        if let Some(i) = found {
            self.entries[i].retired = false;
            self.entries[i].shape = shape.map(String::from);
            self.touched.insert(i);
            return self.entries[i].id;
        }
        let id = alloc(self);
        self.touched.insert(self.entries.len());
        self.entries.push(Entry { kind: kind.into(), name: name.into(), id, shape: shape.map(String::from), retired: false });
        id
    }

    /// Resource group (high byte of method ids).
    pub fn group(&mut self, qname: &str) -> u32 {
        self.get_or_insert("group", qname, None, |l| {
            l.entries.iter().filter(|e| e.kind == "group").map(|e| e.id).max().unwrap_or(0) + 1
        })
    }

    /// Method id = (group << 8) | index. Constructors prefer index 0.
    pub fn method(&mut self, group: u32, qname: &str, is_ctor: bool, shape: &str) -> Result<u32> {
        let base = group << 8;
        let id = self.get_or_insert("method", qname, Some(shape), |l| {
            let used: HashSet<u32> = l
                .entries
                .iter()
                .filter(|e| e.kind == "method" && e.id >> 8 == group)
                .map(|e| e.id)
                .collect();
            if is_ctor && !used.contains(&base) {
                return base;
            }
            let max = used.iter().copied().max().unwrap_or(base);
            max.max(base) + 1
        });
        if id & 0xFF > MAX_METHODS_PER_GROUP || id >> 8 != group {
            bail!("method id space of group {group:#x} exhausted at {qname}");
        }
        Ok(id)
    }

    /// Record field / variant or enum case tag, unique within its parent type.
    pub fn member(&mut self, kind: &str, qname: &str, shape: &str) -> u32 {
        let p = parent(qname).to_string();
        self.get_or_insert(kind, qname, Some(shape), |l| {
            l.entries
                .iter()
                .filter(|e| (e.kind == "field" || e.kind == "case") && parent(&e.name) == p)
                .map(|e| e.id)
                .max()
                .unwrap_or(0)
                + 1
        })
    }

    /// Marks every entry that was not referenced in this run as retired.
    pub fn retire_untouched(&mut self) {
        for (i, e) in self.entries.iter_mut().enumerate() {
            if !self.touched.contains(&i) {
                e.retired = true;
            }
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::from(HEADER);
        let w = self.entries.iter().map(|e| e.name.len()).max().unwrap_or(0).max(48);
        let ws = self.entries.iter().filter_map(|e| e.shape.as_ref()).map(String::len).max().unwrap_or(0);
        for e in &self.entries {
            let id = match e.kind.as_str() {
                "group" => format!("{:#04x}", e.id),
                "method" => format!("{:#06x}", e.id),
                _ => e.id.to_string(),
            };
            let shape = e.shape.as_deref().unwrap_or("");
            let line = format!("{:<10} {:<w$}  {:<6}  {:<ws$}{}", e.kind, e.name, id, shape, if e.retired { "  retired" } else { "" });
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    }
}

fn parent(qname: &str) -> &str {
    qname.rsplit_once('.').map(|(p, _)| p).unwrap_or(qname)
}

fn parse_id(s: &str) -> Result<u32> {
    Ok(match s.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16)?,
        None => s.parse()?,
    })
}

const HEADER: &str = "\
# Wabi wire-identifier lock file — maintained by wabi-wit-gen, commit it.
#
# Pins every numeric identifier that appears on the native ABI or in a payload
# (method ids, record field tags, variant/enum case tags). It is APPEND-ONLY:
#   * new WIT items get the next free number in their scope,
#   * existing entries are never renumbered, so reordering the WIT is safe,
#   * an id stands for one wire shape (last column): a method whose parameter
#     or result types change, or a field/case whose type changes, gets a NEW
#     id and its old one is retired,
#   * removed items stay here as `retired`; their number is never reused,
#   * `wabi-wit-gen --check` (CI) fails if this file would change.
#
# kind     qualified-name                                     id      shape
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_append_only() {
        let mut l = Lock::default();
        let g = l.group("p/i.r");
        assert_eq!(g, 1);
        assert_eq!(l.method(g, "p/i.r.constructor", true, "sync()->_").unwrap(), 0x100);
        assert_eq!(l.method(g, "p/i.r.b", false, "sync()->_").unwrap(), 0x101);
        assert_eq!(l.method(g, "p/i.r.a", false, "sync()->_").unwrap(), 0x102);
        let text = l.render();

        // Re-parse and ask in a different order: same ids.
        let mut l2 = Lock::parse(&text).unwrap();
        let g2 = l2.group("p/i.r");
        assert_eq!(l2.method(g2, "p/i.r.a", false, "sync()->_").unwrap(), 0x102);
        assert_eq!(l2.method(g2, "p/i.r.b", false, "sync()->_").unwrap(), 0x101);
        // `constructor` not touched -> retired, its id is not reused.
        assert_eq!(l2.method(g2, "p/i.r.c", false, "sync()->_").unwrap(), 0x103);
        l2.retire_untouched();
        assert!(l2.render().contains("retired"));
        let mut l3 = Lock::parse(&l2.render()).unwrap();
        assert_eq!(l3.method(1, "p/i.r.d", false, "sync()->_").unwrap(), 0x104);
    }

    #[test]
    fn a_changed_shape_gets_a_new_id_and_the_old_shape_keeps_its_own() {
        let mut l = Lock::default();
        let g = l.group("p/i.r");
        assert_eq!(l.method(g, "p/i.r.get", false, "async(string)->string").unwrap(), 0x101);
        assert_eq!(l.member("field", "p/t.a.x", "u32"), 1);
        l.retire_untouched();

        // The parameter type and the field type change: new ids, old ones retired.
        let mut l = Lock::parse(&l.render()).unwrap();
        let g = l.group("p/i.r");
        assert_eq!(l.method(g, "p/i.r.get", false, "async(u32)->string").unwrap(), 0x102);
        assert_eq!(l.member("field", "p/t.a.x", "u64"), 2);
        assert_eq!(l.method(g, "p/i.r.other", false, "sync()->_").unwrap(), 0x103);
        l.retire_untouched();
        let text = l.render();
        assert_eq!(text.lines().filter(|l| l.contains("p/i.r.get") && l.ends_with("retired")).count(), 1);
        assert_eq!(text.lines().filter(|l| l.contains("p/t.a.x") && l.ends_with("retired")).count(), 1);

        // Going back to the first shape brings its id back; the second one is retired.
        let mut l = Lock::parse(&text).unwrap();
        let g = l.group("p/i.r");
        assert_eq!(l.method(g, "p/i.r.get", false, "async(string)->string").unwrap(), 0x101);
        assert_eq!(l.member("field", "p/t.a.x", "u32"), 1);
        l.retire_untouched();
        assert!(l.render().lines().any(|l| l.contains("0x0102") && l.ends_with("retired")));
        Lock::parse(&l.render()).unwrap();
    }

    #[test]
    fn locks_without_shapes_keep_their_ids() {
        let mut l = Lock::parse("group p/i.r 0x01\nmethod p/i.r.get 0x0105\nfield p/t.a.x 3\n").unwrap();
        let g = l.group("p/i.r");
        assert_eq!(l.method(g, "p/i.r.get", false, "async(string)->string").unwrap(), 0x105);
        assert_eq!(l.member("field", "p/t.a.x", "u32"), 3);
        assert!(l.render().contains("async(string)->string"));
    }

    #[test]
    fn member_tags_are_scoped_per_type() {
        let mut l = Lock::default();
        assert_eq!(l.member("field", "p/t.a.x", "u32"), 1);
        assert_eq!(l.member("field", "p/t.a.y", "u32"), 2);
        assert_eq!(l.member("field", "p/t.b.x", "u32"), 1);
    }
}

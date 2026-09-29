//! `abi-ids.lock`: the append-only table of every numeric wire identifier.
//!
//! IDs are *never* derived from declaration order. The first time the generator
//! sees an item it allocates the next free number in the item's scope and
//! records it here; from then on the lock file is authoritative. Items that
//! disappear from the WIT are kept as `retired` so their number is never
//! reused for something else.

use anyhow::{bail, Context, Result};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub kind: String,
    pub name: String,
    pub id: u32,
    pub retired: bool,
}

#[derive(Debug, Default)]
pub struct Lock {
    entries: Vec<Entry>,
    touched: HashSet<(String, String)>,
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
            if cols.len() < 3 || cols.len() > 4 || (cols.len() == 4 && cols[3] != "retired") {
                bail!("abi-ids.lock:{}: expected `<kind> <name> <id> [retired]`", n + 1);
            }
            let id = parse_id(cols[2]).with_context(|| format!("abi-ids.lock:{}", n + 1))?;
            let e = Entry {
                kind: cols[0].to_string(),
                name: cols[1].to_string(),
                id,
                retired: cols.len() == 4,
            };
            if lock.find(&e.kind, &e.name).is_some() {
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

    fn find(&self, kind: &str, name: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.kind == kind && e.name == name)
    }

    fn touch(&mut self, kind: &str, name: &str) {
        self.touched.insert((kind.to_string(), name.to_string()));
    }

    fn get_or_insert(&mut self, kind: &str, name: &str, alloc: impl FnOnce(&Lock) -> u32) -> u32 {
        self.touch(kind, name);
        if let Some(i) = self.find(kind, name) {
            self.entries[i].retired = false;
            return self.entries[i].id;
        }
        let id = alloc(self);
        self.entries.push(Entry { kind: kind.into(), name: name.into(), id, retired: false });
        id
    }

    /// Resource group (high byte of method ids).
    pub fn group(&mut self, qname: &str) -> u32 {
        self.get_or_insert("group", qname, |l| {
            l.entries.iter().filter(|e| e.kind == "group").map(|e| e.id).max().unwrap_or(0) + 1
        })
    }

    /// Method id = (group << 8) | index. Constructors prefer index 0.
    pub fn method(&mut self, group: u32, qname: &str, is_ctor: bool) -> Result<u32> {
        let base = group << 8;
        let id = self.get_or_insert("method", qname, |l| {
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
    pub fn member(&mut self, kind: &str, qname: &str) -> u32 {
        let p = parent(qname).to_string();
        self.get_or_insert(kind, qname, |l| {
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
        for e in &mut self.entries {
            if !self.touched.contains(&(e.kind.clone(), e.name.clone())) {
                e.retired = true;
            }
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::from(HEADER);
        let w = self.entries.iter().map(|e| e.name.len()).max().unwrap_or(0).max(48);
        for e in &self.entries {
            let id = match e.kind.as_str() {
                "group" => format!("{:#04x}", e.id),
                "method" => format!("{:#06x}", e.id),
                _ => e.id.to_string(),
            };
            let line = format!("{:<10} {:<w$}  {}{}", e.kind, e.name, id, if e.retired { " retired" } else { "" });
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
# Acme wire-identifier lock file — maintained by acme-wit-gen, commit it.
#
# Pins every numeric identifier that appears on the native ABI or in a payload
# (method ids, record field tags, variant/enum case tags). It is APPEND-ONLY:
#   * new WIT items get the next free number in their scope,
#   * existing entries are never renumbered, so reordering client.wit is safe,
#   * removed items stay here as `retired`; their number is never reused,
#   * `acme-wit-gen --check` (CI) fails if this file would change.
#
# kind     qualified-name                                     id
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_append_only() {
        let mut l = Lock::default();
        let g = l.group("p/i.r");
        assert_eq!(g, 1);
        assert_eq!(l.method(g, "p/i.r.constructor", true).unwrap(), 0x100);
        assert_eq!(l.method(g, "p/i.r.b", false).unwrap(), 0x101);
        assert_eq!(l.method(g, "p/i.r.a", false).unwrap(), 0x102);
        let text = l.render();

        // Re-parse and ask in a different order: same ids.
        let mut l2 = Lock::parse(&text).unwrap();
        let g2 = l2.group("p/i.r");
        assert_eq!(l2.method(g2, "p/i.r.a", false).unwrap(), 0x102);
        assert_eq!(l2.method(g2, "p/i.r.b", false).unwrap(), 0x101);
        // `constructor` not touched -> retired, its id is not reused.
        assert_eq!(l2.method(g2, "p/i.r.c", false).unwrap(), 0x103);
        l2.retire_untouched();
        assert!(l2.render().contains("retired"));
        let mut l3 = Lock::parse(&l2.render()).unwrap();
        assert_eq!(l3.method(1, "p/i.r.d", false).unwrap(), 0x104);
    }

    #[test]
    fn member_tags_are_scoped_per_type() {
        let mut l = Lock::default();
        assert_eq!(l.member("field", "p/t.a.x"), 1);
        assert_eq!(l.member("field", "p/t.a.y"), 2);
        assert_eq!(l.member("field", "p/t.b.x"), 1);
    }
}

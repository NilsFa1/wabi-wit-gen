//! Process-wide handle table.
//!
//! Handles are `kind << 56 | serial`. The serial comes from a monotonically
//! increasing counter and is never reused, so a stale or double-dropped handle
//! is always *detected* (ACME_ERR_INVALID_HANDLE) instead of aliasing a newer
//! object. The kind byte only exists to make handles readable in logs and to
//! reject e.g. a future handle passed to acme_stream_next; hosts must treat
//! handles as opaque.

use crate::future::FutureSlot;
use crate::host::ObjectRef;
use crate::stream::StreamSlot;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

pub const KIND_OBJECT: u64 = 0x01;
pub const KIND_FUTURE: u64 = 0x02;
pub const KIND_STREAM: u64 = 0x03;

pub struct ObjectEntry {
    pub group: u32,
    pub obj: ObjectRef,
    pub on_drop: Option<Box<dyn FnOnce() + Send>>,
}

pub enum Entry {
    Object(ObjectEntry),
    Future(Arc<FutureSlot>),
    Stream(Arc<StreamSlot>),
}

static TABLE: LazyLock<Mutex<HashMap<u64, Entry>>> = LazyLock::new(Default::default);
static NEXT: AtomicU64 = AtomicU64::new(1);

fn table() -> std::sync::MutexGuard<'static, HashMap<u64, Entry>> {
    // A panic while holding the lock cannot leave the map inconsistent
    // (all mutations are single insert/remove calls), so poisoning is ignored.
    TABLE.lock().unwrap_or_else(|p| p.into_inner())
}

pub fn alloc(kind: u64) -> u64 {
    (kind << 56) | NEXT.fetch_add(1, Ordering::Relaxed)
}

pub fn insert(handle: u64, e: Entry) {
    table().insert(handle, e);
}

pub fn object(h: u64) -> Option<ObjectRef> {
    match table().get(&h) {
        Some(Entry::Object(o)) => Some(o.obj.clone()),
        _ => None,
    }
}

pub fn future(h: u64) -> Option<Arc<FutureSlot>> {
    match table().get(&h) {
        Some(Entry::Future(f)) => Some(f.clone()),
        _ => None,
    }
}

pub fn stream(h: u64) -> Option<Arc<StreamSlot>> {
    match table().get(&h) {
        Some(Entry::Stream(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Removes the entry iff it has the expected kind. The caller runs any drop
/// logic *after* the table lock is released.
pub fn remove(h: u64, kind: u64) -> Option<Entry> {
    let mut t = table();
    let matches = matches!(
        (t.get(&h), kind),
        (Some(Entry::Object(_)), KIND_OBJECT) | (Some(Entry::Future(_)), KIND_FUTURE) | (Some(Entry::Stream(_)), KIND_STREAM)
    );
    if matches {
        t.remove(&h)
    } else {
        None
    }
}

/// Number of live handles (diagnostics / leak tests).
pub fn live_count() -> usize {
    table().len()
}

use crate::generated::ClientError;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

pub type Reply = Result<Value, ClientError>;

/// Maps in-flight request ids to the task awaiting the response.
#[derive(Default)]
pub struct Correlator {
    next_id: AtomicU64,
    inner: Mutex<State>,
}

#[derive(Default)]
struct State {
    pending: HashMap<u64, oneshot::Sender<Reply>>,
    closed: Option<ClientError>,
}

/// Removes the pending entry if the awaiting future is dropped (cancelled),
/// so abandoned requests never leak.
pub struct Pending {
    pub id: u64,
    pub rx: oneshot::Receiver<Reply>,
    owner: Arc<Correlator>,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.owner.lock().pending.remove(&self.id);
    }
}

impl Correlator {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn register(self: &Arc<Self>) -> Result<Pending, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        let mut s = self.lock();
        if let Some(e) = &s.closed {
            return Err(e.clone());
        }
        s.pending.insert(id, tx);
        Ok(Pending { id, rx, owner: self.clone() })
    }

    /// Delivers a response; unknown ids (late replies to cancelled requests)
    /// are ignored.
    pub fn resolve(&self, id: u64, reply: Reply) {
        if let Some(tx) = self.lock().pending.remove(&id) {
            let _ = tx.send(reply);
        }
    }

    /// Fails every in-flight request and rejects new ones.
    pub fn close(&self, err: ClientError) {
        let drained: Vec<_> = {
            let mut s = self.lock();
            s.closed.get_or_insert(err.clone());
            s.pending.drain().collect()
        };
        for (_, tx) in drained {
            let _ = tx.send(Err(err.clone()));
        }
    }

    pub fn in_flight(&self) -> usize {
        self.lock().pending.len()
    }
}

use crate::ffi::host::AbiStream;
use crate::generated::ClientEvent;
use futures::StreamExt;
use std::sync::Mutex;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

/// Fan-out of client events to any number of independent subscriptions.
/// A subscriber that falls more than `CAPACITY` events behind skips the
/// oldest ones instead of stalling the connection.
pub struct EventHub {
    tx: Mutex<Option<broadcast::Sender<ClientEvent>>>,
}

const CAPACITY: usize = 1024;

impl Default for EventHub {
    fn default() -> Self {
        EventHub { tx: Mutex::new(Some(broadcast::channel(CAPACITY).0)) }
    }
}

impl EventHub {
    pub fn publish(&self, ev: ClientEvent) {
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(ev); // no subscribers is fine
        }
    }

    pub fn subscribe(&self) -> AbiStream<ClientEvent> {
        match self.tx.lock().unwrap().as_ref() {
            Some(tx) => Box::pin(BroadcastStream::new(tx.subscribe()).filter_map(|r| async move { r.ok() })),
            None => Box::pin(futures::stream::empty()),
        }
    }

    /// Ends every subscription (their streams return end-of-stream).
    pub fn close(&self) {
        self.tx.lock().unwrap().take();
    }
}

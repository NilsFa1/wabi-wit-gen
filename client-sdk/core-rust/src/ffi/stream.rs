//! Generic stream handle: a pull-based byte stream whose `next()` returns an
//! ordinary future handle, so hosts need exactly one await mechanism.

use super::abi_types::*;
use super::future::{FutureSlot, Outcome};
use super::handles::{self, Entry, KIND_FUTURE};
use super::host::AbiStream;
use super::runtime;
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct StreamSlot {
    source: tokio::sync::Mutex<Option<AbiStream<Vec<u8>>>>,
    busy: AtomicBool,
    cancel: CancellationToken,
}

/// Clears the busy flag even if the next-task is aborted (next-future cancel).
struct BusyGuard(Arc<StreamSlot>);
impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

impl StreamSlot {
    pub fn new(source: AbiStream<Vec<u8>>) -> Self {
        StreamSlot { source: tokio::sync::Mutex::new(Some(source)), busy: AtomicBool::new(false), cancel: CancellationToken::new() }
    }

    pub fn next(self: &Arc<Self>) -> Result<u64, acme_status> {
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err(ACME_ERR_BUSY);
        }
        let guard = BusyGuard(self.clone());
        let fh = handles::alloc(KIND_FUTURE);
        let slot = Arc::new(FutureSlot::new(fh));
        handles::insert(fh, Entry::Future(slot.clone()));

        if self.cancel.is_cancelled() {
            drop(guard);
            slot.complete(Outcome::End); // not subscribed yet: no callback
            return Ok(fh);
        }

        let this = self.clone();
        let task_slot = slot.clone();
        let jh = runtime::get().spawn(async move {
            let guard = guard;
            let outcome = tokio::select! {
                biased;
                _ = this.cancel.cancelled() => Outcome::Cancelled,
                item = async {
                    let mut src = this.source.lock().await;
                    match src.as_mut() {
                        Some(s) => s.next().await,
                        None => None,
                    }
                } => match item {
                    Some(bytes) => Outcome::Value(bytes),
                    None => Outcome::End,
                },
            };
            // Release `busy` before the host is notified, so it may call
            // next() again straight from its callback continuation.
            drop(guard);
            task_slot.complete(outcome);
        });
        slot.set_abort(jh.abort_handle());
        Ok(fh)
    }

    pub fn cancel(self: &Arc<Self>) {
        if self.cancel.is_cancelled() {
            return;
        }
        self.cancel.cancel();
        // Release the producer (e.g. the broadcast subscription) promptly.
        let this = self.clone();
        runtime::get().spawn(async move {
            this.source.lock().await.take();
        });
    }
}

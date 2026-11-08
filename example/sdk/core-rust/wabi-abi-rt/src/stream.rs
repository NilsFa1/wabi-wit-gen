//! Generic stream handle: a pull-based byte stream whose `next()` returns an
//! ordinary future handle, so hosts need exactly one await mechanism.

use crate::abi_types::*;
use crate::future::{FutureSlot, Outcome};
use crate::handles::{self, Entry, KIND_FUTURE};
use crate::host::AbiStream;
use crate::runtime;
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::collections::VecDeque;
use tokio_util::sync::CancellationToken;

pub struct StreamSlot {
    source: tokio::sync::Mutex<Option<AbiStream<Vec<u8>>>>,
    busy: AtomicBool,
    leftover: Mutex<VecDeque<Vec<u8>>>,
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
        StreamSlot { source: tokio::sync::Mutex::new(Some(source)), busy: AtomicBool::new(false), leftover: Mutex::new(VecDeque::new()), cancel: CancellationToken::new() }
    }

    pub fn next(self: &Arc<Self>) -> Result<u64, wabi_status> {
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err(WABI_ERR_BUSY);
        }
        let guard = BusyGuard(self.clone());
        let fh = handles::alloc(KIND_FUTURE);
        let pull_cancel = CancellationToken::new();
        let slot = Arc::new(FutureSlot::new_pull(fh, pull_cancel.clone()));
        handles::insert(fh, Entry::Future(slot.clone()));

        if self.cancel.is_cancelled() {
            drop(guard);
            slot.complete(Outcome::End); // not subscribed yet: no callback
            return Ok(fh);
        }

        let this = self.clone();
        let task_slot = slot.clone();
        runtime::get().spawn(async move {
            let guard = guard;
            let outcome = tokio::select! {
                biased;
                _ = this.cancel.cancelled() => Outcome::Cancelled,
                _ = pull_cancel.cancelled() => Outcome::Cancelled,
                item = async {
                    let previous = this.leftover.lock().unwrap().pop_front();
                    if previous.is_some() { return previous; }
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
            this.finish_pull(&task_slot, outcome, guard);
        });
        Ok(fh)
    }

    fn finish_pull(&self, slot: &FutureSlot, outcome: Outcome, guard: BusyGuard) {
        slot.complete_with(outcome, |rejected| {
            if let Some(Outcome::Value(bytes)) = rejected {
                self.leftover.lock().unwrap().push_front(bytes);
            }
            // Poll/take and callbacks only see completion after this release.
            drop(guard);
        });
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::c_void;
    use std::sync::atomic::AtomicU64;

    unsafe extern "C" fn on_done(user: *mut c_void, _: u64) {
        let data = &*(user as *const (Arc<StreamSlot>, AtomicU64));
        // No future or stream lock may be held during this reentrant call.
        data.1.store(data.0.next().unwrap_or(u64::MAX), Ordering::SeqCst);
    }

    async fn take_pull(h: u64) -> Vec<u8> {
        let f = handles::future(h).expect("missing future");
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while f.poll() == WABI_PENDING { tokio::task::yield_now().await; }
        }).await.unwrap();
        let (status, bytes) = f.take();
        assert_eq!(status, WABI_OK);
        handles::remove(h, KIND_FUTURE);
        bytes
    }

    #[test]
    fn cancelled_dequeue_preserves_order_and_allows_reentrant_next() {
        runtime::get().block_on(async {
            let source = Box::pin(futures::stream::iter(vec![b"second".to_vec()]));
            let stream = Arc::new(StreamSlot::new(source));
            stream.busy.store(true, Ordering::Release);
            let slot = FutureSlot::new_pull(123, CancellationToken::new());
            let data = (stream.clone(), AtomicU64::new(0));
            assert_eq!(slot.subscribe(Some(on_done), &data as *const _ as *mut c_void), WABI_OK);
            slot.cancel(); // precisely after dequeue, before finish_pull
            assert_eq!(slot.poll(), WABI_PENDING);
            stream.finish_pull(&slot, Outcome::Value(b"first".to_vec()), BusyGuard(stream.clone()));
            assert_eq!(slot.take().0, WABI_ERR_CANCELLED);
            let reentrant = data.1.load(Ordering::SeqCst);
            assert_ne!(reentrant, 0);
            assert_ne!(reentrant, u64::MAX, "callback saw a busy stream");
            assert_eq!(take_pull(reentrant).await, b"first");
            assert_eq!(take_pull(stream.next().unwrap()).await, b"second");
        });
    }

    #[test]
    fn completion_wins_before_cancel_and_releases_busy() {
        let stream = Arc::new(StreamSlot::new(Box::pin(futures::stream::empty())));
        stream.busy.store(true, Ordering::Release);
        let slot = FutureSlot::new_pull(124, CancellationToken::new());
        stream.finish_pull(&slot, Outcome::Value(b"item".to_vec()), BusyGuard(stream.clone()));
        slot.cancel();
        let (status, value) = slot.take();
        assert_eq!(status, WABI_OK);
        assert_eq!(value, b"item");
        assert!(!stream.busy.load(Ordering::Acquire));
        assert!(stream.leftover.lock().unwrap().is_empty());
    }
}

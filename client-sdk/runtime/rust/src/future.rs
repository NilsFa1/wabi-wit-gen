//! One generic future type backs every async WIT function and every
//! `stream.next()`. State machine (ABI.md §4):
//!
//!   Pending --complete()--> Ready(outcome) --take()--> Taken
//!      \--cancel()/drop()--> Ready(Cancelled)
//!
//! The first transition out of Pending wins; later ones are no-ops. That single
//! rule resolves every "completes exactly while being cancelled" race.

use crate::abi_types::*;
use crate::runtime;
use std::ffi::c_void;
use std::sync::Mutex;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

pub enum Outcome {
    Value(Vec<u8>),
    End,
    Cancelled,
    Failed(wabi_status, String),
}

enum State {
    Pending,
    Ready(Outcome),
    Taken,
}

#[derive(Clone, Copy)]
struct Callback {
    f: wabi_future_callback,
    user_data: *mut c_void,
}
// The host promises that user_data may be used from any thread (ABI.md §6).
unsafe impl Send for Callback {}

impl Callback {
    fn invoke(self, handle: u64) {
        if let Some(f) = self.f {
            unsafe { f(self.user_data, handle) }
        }
    }
}

struct Inner {
    state: State,
    subscribed: bool,
    callback: Option<Callback>,
    abort: Option<AbortHandle>,
    pull_cancel: Option<CancellationToken>,
    cancel_requested: bool,
}

pub struct FutureSlot {
    pub handle: u64,
    inner: Mutex<Inner>,
}

impl FutureSlot {
    pub fn new(handle: u64) -> Self {
        FutureSlot {
            handle,
            inner: Mutex::new(Inner { state: State::Pending, subscribed: false, callback: None, abort: None, pull_cancel: None, cancel_requested: false }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Pull cancellation is finalized by the worker after restoring any item.
    /// It must not abort that worker or expose READY before stream cleanup.
    pub(crate) fn new_pull(handle: u64, cancel: CancellationToken) -> Self {
        let slot = Self::new(handle);
        slot.lock().pull_cancel = Some(cancel);
        slot
    }

    /// Called on a core thread. No host callback runs while a lock is held.
    pub fn complete(&self, outcome: Outcome) {
        self.complete_with(outcome, |_| {});
    }

    /// Return rejected outcomes to the producer and clean up before readiness
    /// is observable. The closure must not access this future or call host code.
    pub(crate) fn complete_with(&self, outcome: Outcome, before_publish: impl FnOnce(Option<Outcome>)) {
        let cb = {
            let mut g = self.lock();
            if !matches!(g.state, State::Pending) {
                before_publish(Some(outcome));
                return;
            }
            let rejected = if g.cancel_requested {
                g.state = State::Ready(Outcome::Cancelled);
                Some(outcome)
            } else {
                g.state = State::Ready(outcome);
                None
            };
            g.abort = None;
            before_publish(rejected);
            g.callback.take()
        };
        if let Some(cb) = cb {
            cb.invoke(self.handle);
        }
    }

    /// Host-initiated cancellation (wabi_future_cancel / _drop). The callback
    /// is never invoked on the calling (host) thread; it is scheduled on the
    /// core runtime instead.
    pub fn cancel(&self) {
        let (cb, abort) = {
            let mut g = self.lock();
            if !matches!(g.state, State::Pending) {
                return; // completion won the race
            }
            if let Some(token) = g.pull_cancel.clone() {
                g.cancel_requested = true;
                token.cancel();
                return; // worker restores stream state, then publishes cancellation
            }
            g.state = State::Ready(Outcome::Cancelled);
            (g.callback.take(), g.abort.take())
        };
        if let Some(a) = abort {
            a.abort();
        }
        if let Some(cb) = cb {
            let h = self.handle;
            runtime::get().spawn(async move { cb.invoke(h) });
        }
    }

    /// Associates the task computing this future, so cancel can abort it.
    pub fn set_abort(&self, abort: AbortHandle) {
        let mut g = self.lock();
        match g.state {
            State::Pending => g.abort = Some(abort),
            // Cancelled before the task was registered.
            State::Ready(Outcome::Cancelled) => abort.abort(),
            _ => {}
        }
    }

    pub fn subscribe(&self, f: wabi_future_callback, user_data: *mut c_void) -> wabi_status {
        let mut g = self.lock();
        if g.subscribed {
            return WABI_ERR_BUSY;
        }
        g.subscribed = true;
        match g.state {
            State::Pending => {
                g.callback = Some(Callback { f, user_data });
                WABI_OK
            }
            _ => WABI_READY,
        }
    }

    pub fn poll(&self) -> wabi_status {
        match self.lock().state {
            State::Pending => WABI_PENDING,
            _ => WABI_READY,
        }
    }

    pub fn take(&self) -> (wabi_status, Vec<u8>) {
        let mut g = self.lock();
        match std::mem::replace(&mut g.state, State::Taken) {
            State::Pending => {
                g.state = State::Pending;
                (WABI_PENDING, vec![])
            }
            State::Taken => (WABI_ERR_ALREADY_TAKEN, b"result already taken".to_vec()),
            State::Ready(Outcome::Value(v)) => (WABI_OK, v),
            State::Ready(Outcome::End) => (WABI_STREAM_END, vec![]),
            State::Ready(Outcome::Cancelled) => (WABI_ERR_CANCELLED, vec![]),
            State::Ready(Outcome::Failed(s, msg)) => (s, msg.into_bytes()),
        }
    }
}

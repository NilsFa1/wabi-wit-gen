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

pub enum Outcome {
    Value(Vec<u8>),
    End,
    Cancelled,
    Failed(acme_status, String),
}

enum State {
    Pending,
    Ready(Outcome),
    Taken,
}

#[derive(Clone, Copy)]
struct Callback {
    f: acme_future_callback,
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
}

pub struct FutureSlot {
    pub handle: u64,
    inner: Mutex<Inner>,
}

impl FutureSlot {
    pub fn new(handle: u64) -> Self {
        FutureSlot {
            handle,
            inner: Mutex::new(Inner { state: State::Pending, subscribed: false, callback: None, abort: None }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Called by the core when the operation finished. Must run on a core
    /// thread: the callback is invoked inline, after the lock is released.
    pub fn complete(&self, outcome: Outcome) {
        let cb = {
            let mut g = self.lock();
            if !matches!(g.state, State::Pending) {
                return; // cancelled first: completion is discarded
            }
            g.state = State::Ready(outcome);
            g.abort = None;
            g.callback.take()
        };
        if let Some(cb) = cb {
            cb.invoke(self.handle);
        }
    }

    /// Host-initiated cancellation (acme_future_cancel / _drop). The callback
    /// is never invoked on the calling (host) thread; it is scheduled on the
    /// core runtime instead.
    pub fn cancel(&self) {
        let (cb, abort) = {
            let mut g = self.lock();
            if !matches!(g.state, State::Pending) {
                return; // completion won the race
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

    pub fn subscribe(&self, f: acme_future_callback, user_data: *mut c_void) -> acme_status {
        let mut g = self.lock();
        if g.subscribed {
            return ACME_ERR_BUSY;
        }
        g.subscribed = true;
        match g.state {
            State::Pending => {
                g.callback = Some(Callback { f, user_data });
                ACME_OK
            }
            _ => ACME_READY,
        }
    }

    pub fn poll(&self) -> acme_status {
        match self.lock().state {
            State::Pending => ACME_PENDING,
            _ => ACME_READY,
        }
    }

    pub fn take(&self) -> (acme_status, Vec<u8>) {
        let mut g = self.lock();
        match std::mem::replace(&mut g.state, State::Taken) {
            State::Pending => {
                g.state = State::Pending;
                (ACME_PENDING, vec![])
            }
            State::Taken => (ACME_ERR_ALREADY_TAKEN, b"result already taken".to_vec()),
            State::Ready(Outcome::Value(v)) => (ACME_OK, v),
            State::Ready(Outcome::End) => (ACME_STREAM_END, vec![]),
            State::Ready(Outcome::Cancelled) => (ACME_ERR_CANCELLED, vec![]),
            State::Ready(Outcome::Failed(s, msg)) => (s, msg.into_bytes()),
        }
    }
}

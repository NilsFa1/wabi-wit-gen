//! Host calls (ABI 1.1 and 1.2, ABI.md §4a): the core calls functions of WIT
//! imports that the APP implements. The core invokes the registered handler
//! with a call id; the app answers later, from any thread, with exactly one
//! `<prefix>_host_reply(call, status, response)`.
//!
//! ABI 1.2 (`<prefix>_host_register2`): the handler also gets the object on
//! whose behalf the core calls, and the app is told when the core gives up on
//! a call (the surrounding operation was cancelled).

use crate::abi_types::*;
use crate::{handles, runtime};
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{LazyLock, Mutex, RwLock};
use tokio::sync::oneshot;

/// A host call that produced no return value: no handler registered,
/// cancelled, the app answered with an error status, or a malformed answer.
#[derive(Debug, Clone, PartialEq)]
pub struct HostError {
    pub status: wabi_status,
    pub message: String,
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "host call failed ({}): {}", self.status, self.message)
    }
}

impl std::error::Error for HostError {}

struct Pending {
    /// Kept alive (and at a stable address) until the reply: the app may read
    /// it asynchronously (ABI contract).
    _request: Box<[u8]>,
    reply: Option<oneshot::Sender<(wabi_status, Vec<u8>)>>,
}

#[derive(Clone, Copy)]
enum Invoke {
    V1(unsafe extern "C" fn(*mut c_void, u32, *const u8, usize, u64)),
    V2(unsafe extern "C" fn(*mut c_void, u64, u32, *const u8, usize, u64)),
}

#[derive(Clone, Copy)]
struct Handler {
    invoke: Invoke,
    cancel: wabi_host_cancel,
    user_data: usize, // *mut c_void, stored as an integer to stay Send
}

/// Readers hold the lock while they run the handler, so that registering
/// (the writer) returns only once no invocation of the old handler is running.
static HANDLER: RwLock<Option<Handler>> = RwLock::new(None);
static PENDING: LazyLock<Mutex<HashMap<u64, Pending>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Kind byte of call ids (they share the never-reused counter of handles).
const KIND_HOST_CALL: u64 = 4;

tokio::task_local! {
    /// The object whose method the current task runs (0: none).
    static CURRENT_OBJECT: u64;
}

/// Runs `work` as an operation of `object`: host calls made inside name that
/// object to the app. The generic `call_async` does this for every method; a
/// core needs it only for tasks it spawns itself (`tokio::spawn` does not
/// inherit it).
pub fn with_object<F: Future>(object: u64, work: F) -> impl Future<Output = F::Output> {
    CURRENT_OBJECT.scope(object, work)
}

/// Handle of the object whose method is running in this task (0: none).
pub fn current_object() -> u64 {
    CURRENT_OBJECT.try_with(|o| *o).unwrap_or(0)
}

fn install(handler: Option<Handler>) -> wabi_status {
    *HANDLER.write().unwrap_or_else(|p| p.into_inner()) = handler;
    WABI_OK
}

/// `<prefix>_host_register`: installs (or with NULL removes) the process-wide handler.
pub extern "C" fn wabi_host_register(handler: wabi_host_handler, user_data: *mut c_void) -> wabi_status {
    install(handler.map(|f| Handler { invoke: Invoke::V1(f), cancel: None, user_data: user_data as usize }))
}

/// `<prefix>_host_register2` (ABI 1.2): the handler learns the object, `on_cancel` abandoned calls.
pub extern "C" fn wabi_host_register2(handler: wabi_host_handler2, on_cancel: wabi_host_cancel, user_data: *mut c_void) -> wabi_status {
    install(handler.map(|f| Handler { invoke: Invoke::V2(f), cancel: on_cancel, user_data: user_data as usize }))
}

/// `<prefix>_host_reply`: completes a host call; `response` is copied.
///
/// # Safety
/// `response` must be readable for `len` bytes.
pub unsafe extern "C" fn wabi_host_reply(call: u64, status: wabi_status, response: wabi_bytes) -> wabi_status {
    catch_unwind(AssertUnwindSafe(|| {
        if response.len > 0 && response.ptr.is_null() {
            return WABI_ERR_INVALID_ARGUMENT;
        }
        let Some(mut pending) = PENDING.lock().unwrap().remove(&call) else {
            return WABI_ERR_INVALID_HANDLE;
        };
        let bytes = if response.len == 0 { Vec::new() } else { std::slice::from_raw_parts(response.ptr, response.len).to_vec() };
        if let Some(tx) = pending.reply.take() {
            let _ = tx.send((status, bytes)); // receiver gone: the operation was cancelled
        }
        WABI_OK // `pending` (and the request) is freed here
    }))
    .unwrap_or(WABI_ERR_INTERNAL)
}

/// Tells the app that nobody waits for `call` any more, unless it has answered already.
fn notify_abandoned(call: u64) {
    if !PENDING.lock().unwrap().contains_key(&call) {
        return;
    }
    let handler = HANDLER.read().unwrap_or_else(|p| p.into_inner());
    if let Some(Handler { cancel: Some(cancel), user_data, .. }) = *handler {
        unsafe { cancel(user_data as *mut c_void, call) };
    }
}

/// Armed while a host call waits for its answer: dropping it then means the
/// surrounding operation was cancelled.
struct Abandon(Option<u64>);

impl Drop for Abandon {
    fn drop(&mut self) {
        if let Some(call) = self.0 {
            // Never from inside an ABI call of the host: always on a core thread.
            runtime::get().spawn(async move { notify_abandoned(call) });
        }
    }
}

/// Runs import function `method_id` in the app and waits for its answer.
/// Call it only from async work (never from a sync dispatch, which runs inside
/// an ABI call of the host). Dropping the future (cancellation) abandons the
/// call and tells the app; the request stays valid until the app replies anyway.
pub async fn call(method_id: u32, request: Vec<u8>) -> Result<Vec<u8>, HostError> {
    let object = current_object();
    let request = request.into_boxed_slice();
    let (ptr, len) = (request.as_ptr(), request.len());
    let (tx, rx) = oneshot::channel();
    let id = handles::alloc(KIND_HOST_CALL);
    {
        let handler = HANDLER.read().unwrap_or_else(|p| p.into_inner());
        let Some(handler) = *handler else {
            return Err(HostError { status: WABI_ERR_UNSUPPORTED, message: "no host handler registered (host_register)".into() });
        };
        PENDING.lock().unwrap().insert(id, Pending { _request: request, reply: Some(tx) });
        // The box does not move when the Pending is moved into the map: ptr stays valid.
        let user_data = handler.user_data as *mut c_void;
        match handler.invoke {
            Invoke::V1(f) => unsafe { f(user_data, method_id, ptr, len, id) },
            Invoke::V2(f) => unsafe { f(user_data, object, method_id, ptr, len, id) },
        }
    }
    let mut abandon = Abandon(Some(id));
    let answer = rx.await;
    abandon.0 = None;
    match answer {
        Ok((WABI_OK, bytes)) => Ok(bytes),
        Ok((status, bytes)) => Err(HostError { status, message: String::from_utf8_lossy(&bytes).into_owned() }),
        Err(_) => Err(HostError { status: WABI_ERR_INTERNAL, message: "host call abandoned".into() }),
    }
}

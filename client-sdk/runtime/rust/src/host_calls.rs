//! Host calls (ABI 1.1, ABI.md §4a): the core calls functions of WIT imports
//! that the APP implements. The core invokes the registered handler with a
//! call id; the app answers later, from any thread, with exactly one
//! `<prefix>_host_reply(call, status, response)`.

use crate::abi_types::*;
use crate::handles;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{LazyLock, Mutex};
use tokio::sync::oneshot;

/// A host call that produced no return value: no handler registered,
/// cancelled, the app answered with an error status, or a malformed answer.
#[derive(Debug, Clone, PartialEq)]
pub struct HostError {
    pub status: acme_status,
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
    reply: Option<oneshot::Sender<(acme_status, Vec<u8>)>>,
}

#[derive(Clone, Copy)]
struct Handler {
    f: unsafe extern "C" fn(*mut c_void, u32, *const u8, usize, u64),
    user_data: usize, // *mut c_void, stored as an integer to stay Send
}

static HANDLER: Mutex<Option<Handler>> = Mutex::new(None);
static PENDING: LazyLock<Mutex<HashMap<u64, Pending>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Kind byte of call ids (they share the never-reused counter of handles).
const KIND_HOST_CALL: u64 = 4;

/// `<prefix>_host_register`: installs (or with NULL removes) the process-wide handler.
pub extern "C" fn acme_host_register(handler: acme_host_handler, user_data: *mut c_void) -> acme_status {
    let h = handler.map(|f| Handler { f, user_data: user_data as usize });
    *HANDLER.lock().unwrap() = h;
    ACME_OK
}

/// `<prefix>_host_reply`: completes a host call; `response` is copied.
///
/// # Safety
/// `response` must be readable for `len` bytes.
pub unsafe extern "C" fn acme_host_reply(call: u64, status: acme_status, response: acme_bytes) -> acme_status {
    catch_unwind(AssertUnwindSafe(|| {
        if response.len > 0 && response.ptr.is_null() {
            return ACME_ERR_INVALID_ARGUMENT;
        }
        let Some(mut pending) = PENDING.lock().unwrap().remove(&call) else {
            return ACME_ERR_INVALID_HANDLE;
        };
        let bytes = if response.len == 0 { Vec::new() } else { std::slice::from_raw_parts(response.ptr, response.len).to_vec() };
        if let Some(tx) = pending.reply.take() {
            let _ = tx.send((status, bytes)); // receiver gone: the operation was cancelled
        }
        ACME_OK // `pending` (and the request) is freed here
    }))
    .unwrap_or(ACME_ERR_INTERNAL)
}

/// Runs import function `method_id` in the app and waits for its answer.
/// Call it only from async work (never from a sync dispatch, which runs inside
/// an ABI call of the host). Dropping the future (cancellation) abandons the
/// call; the request stays valid until the app replies anyway.
pub async fn call(method_id: u32, request: Vec<u8>) -> Result<Vec<u8>, HostError> {
    let Some(handler) = *HANDLER.lock().unwrap() else {
        return Err(HostError { status: ACME_ERR_UNSUPPORTED, message: "no host handler registered (host_register)".into() });
    };
    let request = request.into_boxed_slice();
    let (ptr, len) = (request.as_ptr(), request.len());
    let (tx, rx) = oneshot::channel();
    let id = handles::alloc(KIND_HOST_CALL);
    PENDING.lock().unwrap().insert(id, Pending { _request: request, reply: Some(tx) });
    // The box does not move when the Pending is moved into the map: ptr stays valid.
    unsafe { (handler.f)(handler.user_data as *mut c_void, method_id, ptr, len, id) };
    match rx.await {
        Ok((ACME_OK, bytes)) => Ok(bytes),
        Ok((status, bytes)) => Err(HostError { status, message: String::from_utf8_lossy(&bytes).into_owned() }),
        Err(_) => Err(HostError { status: ACME_ERR_INTERNAL, message: "host call abandoned".into() }),
    }
}

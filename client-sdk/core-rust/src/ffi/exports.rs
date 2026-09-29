//! The `extern "C"` surface of abi/acme_client.h. This is the ONLY file in the
//! Rust core that exports symbols. Every entry point:
//!   * validates pointers and handles (never trusts the host),
//!   * catches panics (unwinding across `extern "C"` would be UB),
//!   * writes out-parameters on every path.

use super::abi_types::*;
use super::future::{FutureSlot, Outcome};
use super::handles::{self, Entry, KIND_FUTURE, KIND_OBJECT, KIND_STREAM};
use super::host::{AbiError, ObjectRef};
use super::{runtime, Core};
use crate::generated;
use futures::FutureExt;
use std::ffi::{c_char, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into())
}

/// Runs `f`, converting panics into ACME_ERR_INTERNAL.
fn guard(f: impl FnOnce() -> acme_status) -> acme_status {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(ACME_ERR_INTERNAL)
}

unsafe fn request_slice<'a>(b: acme_bytes) -> Result<&'a [u8], AbiError> {
    if b.len == 0 {
        Ok(&[])
    } else if b.ptr.is_null() {
        Err(AbiError::InvalidArgument("request.ptr is NULL but len > 0".into()))
    } else {
        Ok(std::slice::from_raw_parts(b.ptr, b.len))
    }
}

fn resolve_object(h: u64) -> Result<Option<ObjectRef>, AbiError> {
    if h == 0 {
        Ok(None)
    } else {
        handles::object(h).map(Some).ok_or(AbiError::InvalidHandle)
    }
}

// ---------------------------------------------------------------- metadata

#[no_mangle]
pub extern "C" fn acme_abi_version() -> u32 {
    ACME_ABI_VERSION
}

#[no_mangle]
pub extern "C" fn acme_abi_capabilities() -> u64 {
    ACME_CAP_SYNC_CALLS
        | ACME_CAP_ASYNC_CALLS
        | ACME_CAP_STREAMS
        | ACME_CAP_CANCELLATION
        | ACME_CAP_THREADED_CALLBACK
        | ACME_CAP_PAYLOAD_MSGPACK_1
}

#[no_mangle]
pub extern "C" fn acme_contract_id() -> *const c_char {
    generated::CONTRACT_ID_C.as_ptr()
}

#[no_mangle]
pub extern "C" fn acme_core_info() -> *const c_char {
    c"acme-rust-core/0.1.0 (tokio)".as_ptr()
}

#[no_mangle]
pub extern "C" fn acme_status_name(status: acme_status) -> *const c_char {
    let s: &'static CStr = match status {
        ACME_OK => c"ACME_OK",
        ACME_PENDING => c"ACME_PENDING",
        ACME_READY => c"ACME_READY",
        ACME_STREAM_END => c"ACME_STREAM_END",
        ACME_ERR_INVALID_HANDLE => c"ACME_ERR_INVALID_HANDLE",
        ACME_ERR_INVALID_ARGUMENT => c"ACME_ERR_INVALID_ARGUMENT",
        ACME_ERR_UNKNOWN_METHOD => c"ACME_ERR_UNKNOWN_METHOD",
        ACME_ERR_CANCELLED => c"ACME_ERR_CANCELLED",
        ACME_ERR_BUSY => c"ACME_ERR_BUSY",
        ACME_ERR_ALREADY_TAKEN => c"ACME_ERR_ALREADY_TAKEN",
        ACME_ERR_UNSUPPORTED => c"ACME_ERR_UNSUPPORTED",
        ACME_ERR_INTERNAL => c"ACME_ERR_INTERNAL",
        _ => c"ACME_UNKNOWN_STATUS",
    };
    s.as_ptr()
}

// ------------------------------------------------------------------ memory

/// # Safety
/// `buffer` must have been returned by this library and not freed before.
#[no_mangle]
pub unsafe extern "C" fn acme_buffer_free(buffer: acme_buffer) {
    let _ = catch_unwind(AssertUnwindSafe(|| buffer.free()));
}

// ------------------------------------------------------------------- calls

/// # Safety
/// `request` must be readable for `len` bytes; `out_response` writable.
#[no_mangle]
pub unsafe extern "C" fn acme_call_sync(
    object: u64,
    method_id: u32,
    request: acme_bytes,
    out_response: *mut acme_buffer,
) -> acme_status {
    if out_response.is_null() {
        return ACME_ERR_INVALID_ARGUMENT;
    }
    *out_response = acme_buffer::EMPTY;
    let r = catch_unwind(AssertUnwindSafe(|| -> Result<Vec<u8>, AbiError> {
        let req = request_slice(request)?;
        let obj = resolve_object(object)?;
        generated::dispatch_sync::<Core>(obj, method_id, req)
    }));
    let (status, bytes) = match r {
        Ok(Ok(v)) => (ACME_OK, v),
        Ok(Err(e)) => (e.status(), e.message().into_bytes()),
        Err(p) => (ACME_ERR_INTERNAL, panic_message(p).into_bytes()),
    };
    *out_response = acme_buffer::from_vec(bytes);
    status
}

/// # Safety
/// `request` must be readable for `len` bytes; `out_future` writable.
#[no_mangle]
pub unsafe extern "C" fn acme_call_async(
    object: u64,
    method_id: u32,
    request: acme_bytes,
    out_future: *mut u64,
) -> acme_status {
    if out_future.is_null() {
        return ACME_ERR_INVALID_ARGUMENT;
    }
    *out_future = 0;
    guard(|| {
        let fut = match request_slice(request)
            .and_then(|req| Ok((req, resolve_object(object)?)))
            .and_then(|(req, obj)| generated::dispatch_async::<Core>(obj, method_id, req))
        {
            Ok(f) => f,
            Err(e) => return e.status(),
        };
        let h = handles::alloc(KIND_FUTURE);
        let slot = Arc::new(FutureSlot::new(h));
        handles::insert(h, Entry::Future(slot.clone()));
        let task_slot = slot.clone();
        let jh = runtime::get().spawn(async move {
            let outcome = match AssertUnwindSafe(fut).catch_unwind().await {
                Ok(Ok(v)) => Outcome::Value(v),
                Ok(Err(e)) => Outcome::Failed(e.status(), e.message()),
                Err(p) => Outcome::Failed(ACME_ERR_INTERNAL, panic_message(p)),
            };
            task_slot.complete(outcome);
        });
        slot.set_abort(jh.abort_handle());
        *out_future = h;
        ACME_OK
    })
}

#[no_mangle]
pub extern "C" fn acme_object_drop(object: u64) -> acme_status {
    guard(|| match handles::remove(object, KIND_OBJECT) {
        Some(Entry::Object(mut o)) => {
            if let Some(hook) = o.on_drop.take() {
                hook();
            }
            ACME_OK
        }
        _ => ACME_ERR_INVALID_HANDLE,
    })
}

// ----------------------------------------------------------------- futures

/// # Safety
/// `callback`/`user_data` must stay valid until the callback has been invoked.
#[no_mangle]
pub unsafe extern "C" fn acme_future_subscribe(
    future: u64,
    callback: acme_future_callback,
    user_data: *mut c_void,
) -> acme_status {
    if callback.is_none() {
        return ACME_ERR_INVALID_ARGUMENT;
    }
    guard(|| match handles::future(future) {
        Some(f) => f.subscribe(callback, user_data),
        None => ACME_ERR_INVALID_HANDLE,
    })
}

#[no_mangle]
pub extern "C" fn acme_future_poll(future: u64) -> acme_status {
    guard(|| handles::future(future).map(|f| f.poll()).unwrap_or(ACME_ERR_INVALID_HANDLE))
}

/// # Safety
/// `out_result` must be writable.
#[no_mangle]
pub unsafe extern "C" fn acme_future_take(future: u64, out_result: *mut acme_buffer) -> acme_status {
    if out_result.is_null() {
        return ACME_ERR_INVALID_ARGUMENT;
    }
    *out_result = acme_buffer::EMPTY;
    guard(|| match handles::future(future) {
        Some(f) => {
            let (status, bytes) = f.take();
            *out_result = acme_buffer::from_vec(bytes);
            status
        }
        None => ACME_ERR_INVALID_HANDLE,
    })
}

#[no_mangle]
pub extern "C" fn acme_future_cancel(future: u64) -> acme_status {
    guard(|| match handles::future(future) {
        Some(f) => {
            f.cancel();
            ACME_OK
        }
        None => ACME_ERR_INVALID_HANDLE,
    })
}

#[no_mangle]
pub extern "C" fn acme_future_drop(future: u64) -> acme_status {
    guard(|| match handles::remove(future, KIND_FUTURE) {
        Some(Entry::Future(f)) => {
            f.cancel(); // no-op if already complete
            ACME_OK
        }
        _ => ACME_ERR_INVALID_HANDLE,
    })
}

// ----------------------------------------------------------------- streams

/// # Safety
/// `out_future` must be writable.
#[no_mangle]
pub unsafe extern "C" fn acme_stream_next(stream: u64, out_future: *mut u64) -> acme_status {
    if out_future.is_null() {
        return ACME_ERR_INVALID_ARGUMENT;
    }
    *out_future = 0;
    guard(|| match handles::stream(stream) {
        Some(s) => match s.next() {
            Ok(h) => {
                *out_future = h;
                ACME_OK
            }
            Err(status) => status,
        },
        None => ACME_ERR_INVALID_HANDLE,
    })
}

#[no_mangle]
pub extern "C" fn acme_stream_cancel(stream: u64) -> acme_status {
    guard(|| match handles::stream(stream) {
        Some(s) => {
            s.cancel();
            ACME_OK
        }
        None => ACME_ERR_INVALID_HANDLE,
    })
}

#[no_mangle]
pub extern "C" fn acme_stream_drop(stream: u64) -> acme_status {
    guard(|| match handles::remove(stream, KIND_STREAM) {
        Some(Entry::Stream(s)) => {
            s.cancel();
            ACME_OK
        }
        _ => ACME_ERR_INVALID_HANDLE,
    })
}

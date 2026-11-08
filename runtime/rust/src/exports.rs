//! The generic implementation of the functions of abi/wabi_abi_functions.h,
//! parameterised over the WIT [`Package`]. Nothing here is exported directly:
//! the generated `exports` module exports them under the package's symbol
//! prefix (`wabi_client_call_sync`, ...) and forwards here. Every entry point:
//!   * validates pointers and handles (never trusts the host),
//!   * catches panics (unwinding across `extern "C"` would be UB),
//!   * writes out-parameters on every path.

use crate::abi_types::*;
use crate::future::{FutureSlot, Outcome};
use crate::handles::{self, Entry, KIND_FUTURE, KIND_OBJECT, KIND_STREAM};
use crate::host::{AbiError, ObjectRef};
use crate::{runtime, Package};
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

/// Runs `f`, converting panics into WABI_ERR_INTERNAL.
fn guard(f: impl FnOnce() -> wabi_status) -> wabi_status {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(WABI_ERR_INTERNAL)
}

unsafe fn request_slice<'a>(b: wabi_bytes) -> Result<&'a [u8], AbiError> {
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

pub extern "C" fn wabi_abi_version() -> u32 {
    WABI_ABI_VERSION
}

pub extern "C" fn wabi_abi_capabilities<P: Package>() -> u64 {
    let caps = WABI_CAP_SYNC_CALLS
        | WABI_CAP_ASYNC_CALLS
        | WABI_CAP_STREAMS
        | WABI_CAP_CANCELLATION
        | WABI_CAP_THREADED_CALLBACK
        | WABI_CAP_PAYLOAD_MSGPACK_1;
    if P::HOST_CALLS {
        caps | WABI_CAP_HOST_CALLS | WABI_CAP_HOST_CALLS_2
    } else {
        caps
    }
}

pub extern "C" fn wabi_contract_id<P: Package>() -> *const c_char {
    P::CONTRACT_ID.as_ptr()
}

pub extern "C" fn wabi_core_info<P: Package>() -> *const c_char {
    P::CORE_INFO.as_ptr()
}

pub extern "C" fn wabi_status_name(status: wabi_status) -> *const c_char {
    let s: &'static CStr = match status {
        WABI_OK => c"WABI_OK",
        WABI_PENDING => c"WABI_PENDING",
        WABI_READY => c"WABI_READY",
        WABI_STREAM_END => c"WABI_STREAM_END",
        WABI_ERR_INVALID_HANDLE => c"WABI_ERR_INVALID_HANDLE",
        WABI_ERR_INVALID_ARGUMENT => c"WABI_ERR_INVALID_ARGUMENT",
        WABI_ERR_UNKNOWN_METHOD => c"WABI_ERR_UNKNOWN_METHOD",
        WABI_ERR_CANCELLED => c"WABI_ERR_CANCELLED",
        WABI_ERR_BUSY => c"WABI_ERR_BUSY",
        WABI_ERR_ALREADY_TAKEN => c"WABI_ERR_ALREADY_TAKEN",
        WABI_ERR_UNSUPPORTED => c"WABI_ERR_UNSUPPORTED",
        WABI_ERR_INTERNAL => c"WABI_ERR_INTERNAL",
        _ => c"WABI_UNKNOWN_STATUS",
    };
    s.as_ptr()
}

// ------------------------------------------------------------------ memory

/// # Safety
/// `buffer` must have been returned by this library and not freed before.
pub unsafe extern "C" fn wabi_buffer_free(buffer: wabi_buffer) {
    let _ = catch_unwind(AssertUnwindSafe(|| buffer.free()));
}

// ------------------------------------------------------------------- calls

/// # Safety
/// `request` must be readable for `len` bytes; `out_response` writable.
pub unsafe extern "C" fn wabi_call_sync<P: Package>(
    object: u64,
    method_id: u32,
    request: wabi_bytes,
    out_response: *mut wabi_buffer,
) -> wabi_status {
    if out_response.is_null() {
        return WABI_ERR_INVALID_ARGUMENT;
    }
    *out_response = wabi_buffer::EMPTY;
    let r = catch_unwind(AssertUnwindSafe(|| -> Result<Vec<u8>, AbiError> {
        let req = request_slice(request)?;
        let obj = resolve_object(object)?;
        P::dispatch_sync(obj, method_id, req)
    }));
    let (status, bytes) = match r {
        Ok(Ok(v)) => (WABI_OK, v),
        Ok(Err(e)) => (e.status(), e.message().into_bytes()),
        Err(p) => (WABI_ERR_INTERNAL, panic_message(p).into_bytes()),
    };
    *out_response = wabi_buffer::from_vec(bytes);
    status
}

/// # Safety
/// `request` must be readable for `len` bytes; `out_future` writable.
pub unsafe extern "C" fn wabi_call_async<P: Package>(
    object: u64,
    method_id: u32,
    request: wabi_bytes,
    out_future: *mut u64,
) -> wabi_status {
    if out_future.is_null() {
        return WABI_ERR_INVALID_ARGUMENT;
    }
    *out_future = 0;
    guard(|| {
        let fut = match request_slice(request)
            .and_then(|req| Ok((req, resolve_object(object)?)))
            .and_then(|(req, obj)| P::dispatch_async(obj, method_id, req))
        {
            Ok(f) => f,
            Err(e) => return e.status(),
        };
        let h = handles::alloc(KIND_FUTURE);
        let slot = Arc::new(FutureSlot::new(h));
        handles::insert(h, Entry::Future(slot.clone()));
        let task_slot = slot.clone();
        // Host calls made by this operation belong to `object` (ABI 1.2).
        let fut = crate::host_calls::with_object(object, fut);
        let jh = runtime::get().spawn(async move {
            let outcome = match AssertUnwindSafe(fut).catch_unwind().await {
                Ok(Ok(v)) => Outcome::Value(v),
                Ok(Err(e)) => Outcome::Failed(e.status(), e.message()),
                Err(p) => Outcome::Failed(WABI_ERR_INTERNAL, panic_message(p)),
            };
            task_slot.complete(outcome);
        });
        slot.set_abort(jh.abort_handle());
        *out_future = h;
        WABI_OK
    })
}

pub extern "C" fn wabi_object_drop(object: u64) -> wabi_status {
    guard(|| match handles::remove(object, KIND_OBJECT) {
        Some(Entry::Object(mut o)) => {
            if let Some(hook) = o.on_drop.take() {
                hook();
            }
            WABI_OK
        }
        _ => WABI_ERR_INVALID_HANDLE,
    })
}

// ----------------------------------------------------------------- futures

/// # Safety
/// `callback`/`user_data` must stay valid until the callback has been invoked.
pub unsafe extern "C" fn wabi_future_subscribe(
    future: u64,
    callback: wabi_future_callback,
    user_data: *mut c_void,
) -> wabi_status {
    if callback.is_none() {
        return WABI_ERR_INVALID_ARGUMENT;
    }
    guard(|| match handles::future(future) {
        Some(f) => f.subscribe(callback, user_data),
        None => WABI_ERR_INVALID_HANDLE,
    })
}

pub extern "C" fn wabi_future_poll(future: u64) -> wabi_status {
    guard(|| handles::future(future).map(|f| f.poll()).unwrap_or(WABI_ERR_INVALID_HANDLE))
}

/// # Safety
/// `out_result` must be writable.
pub unsafe extern "C" fn wabi_future_take(future: u64, out_result: *mut wabi_buffer) -> wabi_status {
    if out_result.is_null() {
        return WABI_ERR_INVALID_ARGUMENT;
    }
    *out_result = wabi_buffer::EMPTY;
    guard(|| match handles::future(future) {
        Some(f) => {
            let (status, bytes) = f.take();
            *out_result = wabi_buffer::from_vec(bytes);
            status
        }
        None => WABI_ERR_INVALID_HANDLE,
    })
}

pub extern "C" fn wabi_future_cancel(future: u64) -> wabi_status {
    guard(|| match handles::future(future) {
        Some(f) => {
            f.cancel();
            WABI_OK
        }
        None => WABI_ERR_INVALID_HANDLE,
    })
}

pub extern "C" fn wabi_future_drop(future: u64) -> wabi_status {
    guard(|| match handles::remove(future, KIND_FUTURE) {
        Some(Entry::Future(f)) => {
            f.cancel(); // no-op if already complete
            WABI_OK
        }
        _ => WABI_ERR_INVALID_HANDLE,
    })
}

// ----------------------------------------------------------------- streams

/// # Safety
/// `out_future` must be writable.
pub unsafe extern "C" fn wabi_stream_next(stream: u64, out_future: *mut u64) -> wabi_status {
    if out_future.is_null() {
        return WABI_ERR_INVALID_ARGUMENT;
    }
    *out_future = 0;
    guard(|| match handles::stream(stream) {
        Some(s) => match s.next() {
            Ok(h) => {
                *out_future = h;
                WABI_OK
            }
            Err(status) => status,
        },
        None => WABI_ERR_INVALID_HANDLE,
    })
}

pub extern "C" fn wabi_stream_cancel(stream: u64) -> wabi_status {
    guard(|| match handles::stream(stream) {
        Some(s) => {
            s.cancel();
            WABI_OK
        }
        None => WABI_ERR_INVALID_HANDLE,
    })
}

pub extern "C" fn wabi_stream_drop(stream: u64) -> wabi_status {
    guard(|| match handles::remove(stream, KIND_STREAM) {
        Some(Entry::Stream(s)) => {
            s.cancel();
            WABI_OK
        }
        _ => WABI_ERR_INVALID_HANDLE,
    })
}

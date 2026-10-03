//! Rust mirror of the C types in abi/wabi_abi.h. Only `#[repr(C)]`
//! structs and fixed-width integers — nothing Rust-specific crosses the ABI.
#![allow(non_camel_case_types)]

use std::ffi::c_void;

pub type wabi_status = i32;
pub type wabi_future_callback = Option<unsafe extern "C" fn(user_data: *mut c_void, future: u64)>;
/// ABI 1.1: `void (*)(void* user_data, uint32_t method_id, const uint8_t* req, size_t len, uint64_t call)`
pub type wabi_host_handler =
    Option<unsafe extern "C" fn(user_data: *mut c_void, method_id: u32, request: *const u8, request_len: usize, call: u64)>;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct wabi_bytes {
    pub ptr: *const u8,
    pub len: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct wabi_buffer {
    pub ptr: *mut u8,
    pub len: usize,
}

impl wabi_buffer {
    pub const EMPTY: wabi_buffer = wabi_buffer { ptr: std::ptr::null_mut(), len: 0 };

    /// Hands a Rust allocation to the host. `Box<[u8]>` has no spare
    /// capacity, so (ptr, len) is all wabi_buffer_free needs to rebuild it.
    pub fn from_vec(v: Vec<u8>) -> wabi_buffer {
        if v.is_empty() {
            return Self::EMPTY;
        }
        let b = v.into_boxed_slice();
        let len = b.len();
        wabi_buffer { ptr: Box::into_raw(b) as *mut u8, len }
    }

    /// # Safety
    /// `self` must come from `from_vec` and not have been freed yet.
    pub unsafe fn free(self) {
        if !self.ptr.is_null() && self.len > 0 {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(self.ptr, self.len)));
        }
    }
}

/// ABI 1.1: optional host calls (WABI_CAP_HOST_CALLS).
pub const WABI_ABI_VERSION: u32 = (1 << 16) | 1;

pub const WABI_CAP_SYNC_CALLS: u64 = 1 << 0;
pub const WABI_CAP_ASYNC_CALLS: u64 = 1 << 1;
pub const WABI_CAP_STREAMS: u64 = 1 << 2;
pub const WABI_CAP_CANCELLATION: u64 = 1 << 3;
pub const WABI_CAP_THREADED_CALLBACK: u64 = 1 << 4;
pub const WABI_CAP_PAYLOAD_MSGPACK_1: u64 = 1 << 5;
pub const WABI_CAP_HOST_CALLS: u64 = 1 << 6;

pub const WABI_OK: wabi_status = 0;
pub const WABI_PENDING: wabi_status = 1;
pub const WABI_READY: wabi_status = 2;
pub const WABI_STREAM_END: wabi_status = 3;
pub const WABI_ERR_INVALID_HANDLE: wabi_status = -1;
pub const WABI_ERR_INVALID_ARGUMENT: wabi_status = -2;
pub const WABI_ERR_UNKNOWN_METHOD: wabi_status = -3;
pub const WABI_ERR_CANCELLED: wabi_status = -4;
pub const WABI_ERR_BUSY: wabi_status = -5;
pub const WABI_ERR_ALREADY_TAKEN: wabi_status = -6;
pub const WABI_ERR_UNSUPPORTED: wabi_status = -7;
pub const WABI_ERR_INTERNAL: wabi_status = -8;

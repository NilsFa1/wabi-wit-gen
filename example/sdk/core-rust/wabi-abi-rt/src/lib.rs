//! Hand-written, contract-agnostic runtime for Rust cores implementing the
//! Wabi native ABI (abi/wabi_abi.h, ABI.md). The Rust counterpart of
//! `runtime/go`. A Rust core consists of
//!
//!   * this crate: handles, futures, streams, payload codec, host calls and
//!     the generic implementation of every ABI function,
//!   * the generated module (wabi-wit-gen `rust` target): contract types,
//!     codec, one trait per resource, host-call stubs for WIT imports, the
//!     method-id dispatcher and the `#[export_name]` wrappers under the
//!     package symbol prefix,
//!   * the hand-written business logic implementing the generated traits.
//!
//!   abi_types.rs  Rust mirror of the C types
//!   exports.rs    generic implementation of the ABI functions (over [`Package`])
//!   handles.rs    handle table (never-reused u64 handles)
//!   future.rs     generic future state machine + callbacks
//!   stream.rs     generic pull stream on top of futures
//!   wire.rs       payload encoding v1 (MessagePack subset)
//!   host.rs       tiny API used by the generated dispatcher
//!   host_calls.rs ABI 1.1/1.2: the core calls the app (WIT imports)
//!   runtime.rs    core-owned tokio runtime

pub mod abi_types;
pub mod exports;
pub mod future;
pub mod handles;
pub mod host;
pub mod host_calls;
pub mod runtime;
pub mod stream;
pub mod wire;

use host::{AbiError, BoxFuture, ObjectRef};
use std::ffi::CStr;

/// One WIT package as seen by the generic exports. Implemented by the
/// generated code; the core only implements the generated traits.
pub trait Package: 'static {
    /// `namespace:package@1.0.0` (example)
    const CONTRACT_ID: &'static CStr;
    /// Free text identifying the core implementation.
    const CORE_INFO: &'static CStr;
    /// The package has WIT imports: advertise WABI_CAP_HOST_CALLS.
    const HOST_CALLS: bool;

    fn dispatch_sync(object: Option<ObjectRef>, method_id: u32, request: &[u8]) -> Result<Vec<u8>, AbiError>;

    fn dispatch_async(
        object: Option<ObjectRef>,
        method_id: u32,
        request: &[u8],
    ) -> Result<BoxFuture<Result<Vec<u8>, AbiError>>, AbiError>;
}

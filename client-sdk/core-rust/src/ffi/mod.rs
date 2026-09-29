//! FFI adapter: implements the generic Acme native ABI on top of the plain
//! Rust core. Swapping the core language means replacing *this layer plus the
//! core*; the SDKs only ever see abi/acme_client.h.
//!
//!   exports.rs   extern "C" entry points (the only exported symbols)
//!   handles.rs   handle table (never-reused u64 handles)
//!   future.rs    generic future state machine + callbacks
//!   stream.rs    generic pull stream on top of futures
//!   wire.rs      payload encoding v1 (MessagePack subset)
//!   host.rs      tiny API used by the generated dispatcher
//!   runtime.rs   core-owned tokio runtime

pub mod abi_types;
pub mod exports;
pub mod future;
pub mod handles;
pub mod host;
pub mod runtime;
pub mod stream;
pub mod wire;

/// The single line that binds the generated contract to this core's types.
pub struct Core;

impl crate::generated::Implementation for Core {
    type Client = crate::api::Client;
}

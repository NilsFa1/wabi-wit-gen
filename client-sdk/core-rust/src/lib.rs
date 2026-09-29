//! Acme client core — Rust implementation.
//!
//! ```text
//!   api/        idiomatic async Rust client (the actual logic)
//!   protocol/   backend frames (JSON over WebSocket)
//!   runtime/    request/response correlation, event hub
//!   transport/  transport abstraction (+ in-process loopback)
//!   websocket/  tokio-tungstenite transport
//!   crypto/     HMAC-SHA256 challenge/response
//!   mock/       mock backend (tests, examples)
//!   generated/  types + codec + dispatch, generated from wit/client.wit
//!
//! The generic native ABI adapter lives in the `acme-abi-rt` crate (runtime/rust).
//! ```

pub mod api;
pub mod crypto;
pub mod generated;
#[cfg(feature = "mock-backend")]
pub mod mock;
pub mod protocol;
pub mod runtime;
pub mod transport;
pub mod websocket;

pub use api::*;

/// The single line that binds the generated contract to this core's types.
/// The generated exports (`generated::exports`) reference `crate::Core`.
pub struct Core;

impl generated::Implementation for Core {
    const CORE_INFO: &'static std::ffi::CStr = c"acme-rust-core/0.1.0 (tokio)";
    type Client = api::Client;
}

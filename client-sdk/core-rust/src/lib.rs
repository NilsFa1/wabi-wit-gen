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
//!   ffi/        generic native ABI adapter (abi/acme_abi_functions.h)
//! ```

pub mod api;
pub mod crypto;
pub mod ffi;
pub mod generated;
#[cfg(feature = "mock-backend")]
pub mod mock;
pub mod protocol;
pub mod runtime;
pub mod transport;
pub mod websocket;

pub use api::*;

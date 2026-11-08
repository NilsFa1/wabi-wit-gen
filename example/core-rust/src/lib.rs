//! Example core — Rust implementation of `wit/client.wit`.
//!
//! ```text
//!   api/        idiomatic async Rust client (the actual logic)
//!   session     one session with the backend: sealed + signed requests, event channel
//!   crypto/     AES-256-GCM bodies, request digests
//!   protocol/   what travels over HTTP (paths, headers, JSON bodies)
//!   transport/  HTTP (reqwest) or the in-process mock (`mem://`)
//!   events      fan-out of events to subscriptions
//!   mock/       the mock service (in-process and as `demo-mock-server`)
//!   generated   types + codec + dispatch, generated from wit/client.wit (sdk/core-rust/generated.rs)
//!
//! The generic native ABI adapter is the `wabi-abi-rt` crate (sdk/core-rust/wabi-abi-rt).
//! ```

pub mod api;
pub mod crypto;
pub mod events;
#[path = "../../sdk/core-rust/generated.rs"]
pub mod generated;
#[cfg(feature = "mock-backend")]
pub mod mock;
pub mod protocol;
pub mod session;
pub mod transport;

pub use api::*;

/// The single line that binds the generated contract to this core's types.
/// The generated exports (`generated::exports`) reference `crate::Core`.
pub struct Core;

impl generated::Implementation for Core {
    const CORE_INFO: &'static std::ffi::CStr = c"demo-rust-core/0.1.0 (tokio, reqwest)";
    type Client = api::Client;
    type Transport = api::SharedTransport;
}

//! Idiomatic Rust API of the core. No FFI types anywhere in here: the same
//! `Client` can be used directly from Rust (see tests/client.rs) or exported
//! through the ABI adapter in `crate::ffi`.

mod client;

pub use crate::generated::{
    ClientError, ClientEvent, ClientOptions, ClientResource, Command, CommandResult, ConnectionState, Customer,
};
pub use client::Client;

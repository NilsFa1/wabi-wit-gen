//! What travels between client and backend: plain HTTP/1.1 requests whose
//! bodies are sealed JSON (see `crypto`).
//!
//! ```text
//!   POST /v1/sessions           {"client": "..."}            -> {"session": "<id>"}
//!   GET  /v1/events                                          -> server-sent events, one sealed JSON each
//!   GET  /v1/whoami                                          -> {"key": "<fingerprint>"}
//!   GET  /v1/customers?id=<id>                               -> customer
//!   POST /v1/customers          {"name": "...", "email": …}  -> customer
//!   POST /v1/commands           {"name": "...", "payload"}   -> {"name": "...", "payload"}
//! ```
//!
//! Headers: `x-demo-session` on everything but `POST /v1/sessions`;
//! `x-demo-signature` (base64 ECDSA signature of `crypto::request_digest`) if the
//! session is bound to a key, which `POST /v1/sessions` announces in
//! `x-demo-key` (base64 SubjectPublicKeyInfo). Errors are a status code plus a
//! sealed `{"code": "...", "message": "..."}`.
//!
//! This is *core-specific* and independent from the native ABI: the Go core
//! (core-go) speaks the same protocol against the same mock service.

use crate::generated::{ClientError, CommandResult, Customer};
use base64::Engine;
use serde::{Deserialize, Serialize};

pub const HEADER_SESSION: &str = "x-demo-session";
pub const HEADER_KEY: &str = "x-demo-key";
pub const HEADER_SIGNATURE: &str = "x-demo-signature";

pub const PATH_SESSIONS: &str = "/v1/sessions";
pub const PATH_EVENTS: &str = "/v1/events";
pub const PATH_WHOAMI: &str = "/v1/whoami";
pub const PATH_CUSTOMERS: &str = "/v1/customers";
pub const PATH_COMMANDS: &str = "/v1/commands";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionDto {
    pub session: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WhoamiDto {
    pub key: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ErrorBody {
    pub code: String,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EventBody {
    CustomerUpdated { customer: CustomerDto },
    MessageReceived { text: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CustomerDto {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CommandDto {
    pub name: String,
    /// base64 (standard alphabet)
    pub payload: String,
}

impl From<CustomerDto> for Customer {
    fn from(d: CustomerDto) -> Self {
        Customer { id: d.id, name: d.name, email: d.email }
    }
}

impl From<ErrorBody> for ClientError {
    fn from(e: ErrorBody) -> Self {
        match e.code.as_str() {
            "not-found" => ClientError::NotFound,
            "unauthorized" => ClientError::Unauthorized,
            _ => ClientError::ProtocolError(e.message.unwrap_or(e.code)),
        }
    }
}

impl ErrorBody {
    pub fn new(code: &str, message: impl Into<Option<String>>) -> Self {
        ErrorBody { code: code.into(), message: message.into() }
    }
}

pub fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

pub fn unb64(s: &str) -> Result<Vec<u8>, ClientError> {
    base64::engine::general_purpose::STANDARD.decode(s).map_err(|e| ClientError::ProtocolError(format!("bad base64: {e}")))
}

pub fn parse<T: for<'de> Deserialize<'de>>(json: &[u8]) -> Result<T, ClientError> {
    serde_json::from_slice(json).map_err(|e| ClientError::ProtocolError(format!("unexpected response shape: {e}")))
}

pub fn command_result(dto: CommandDto) -> Result<CommandResult, ClientError> {
    Ok(CommandResult { name: dto.name, payload: unb64(&dto.payload)? })
}

/// Percent-encodes a query value (everything but RFC 3986 unreserved characters).
pub fn query_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Inverse of `query_value`; `None` for malformed escapes or invalid UTF-8.
pub fn decode_query_value(s: &str) -> Option<String> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hex = [bytes.next()?, bytes.next()?];
            out.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            out.push(b);
        }
    }
    String::from_utf8(out).ok()
}

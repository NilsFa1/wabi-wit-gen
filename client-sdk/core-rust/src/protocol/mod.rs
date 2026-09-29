//! Backend wire protocol (client <-> server over one WebSocket).
//!
//! This is *core-specific* and completely independent from the native ABI:
//! a Go or Zig core may speak the same backend protocol, or a different one.
//! JSON text frames keep the backend debuggable with any WebSocket tool.

use crate::generated::{ClientError, CommandResult, Customer};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerFrame {
    Challenge { nonce: String },
    AuthOk,
    AuthFailed { reason: String },
    Response {
        id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ok: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<ErrorBody>,
    },
    Event { event: EventBody },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClientFrame {
    Auth { mac: String },
    Request { id: u64, method: String, params: Value },
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
            "network-error" => ClientError::NetworkError(e.message.unwrap_or_default()),
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
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| ClientError::ProtocolError(format!("bad base64 payload: {e}")))
}

pub fn parse<T: for<'de> Deserialize<'de>>(v: Value) -> Result<T, ClientError> {
    serde_json::from_value(v).map_err(|e| ClientError::ProtocolError(format!("unexpected response shape: {e}")))
}

pub fn command_result(v: Value) -> Result<CommandResult, ClientError> {
    let dto: CommandDto = parse(v)?;
    Ok(CommandResult { name: dto.name, payload: unb64(&dto.payload)? })
}

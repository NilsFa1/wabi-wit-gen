//! Transport abstraction: a full-duplex channel of text frames. The WebSocket
//! and the in-process loopback look identical to the rest of the core.

use crate::generated::ClientError;
use tokio::sync::mpsc;

#[cfg(feature = "mock-backend")]
pub mod memory;

pub struct Transport {
    pub outgoing: mpsc::Sender<String>,
    pub incoming: mpsc::Receiver<String>,
}

/// Two connected transports (client side, server side).
pub fn pair(capacity: usize) -> (Transport, Transport) {
    let (a_tx, b_rx) = mpsc::channel(capacity);
    let (b_tx, a_rx) = mpsc::channel(capacity);
    (Transport { outgoing: a_tx, incoming: a_rx }, Transport { outgoing: b_tx, incoming: b_rx })
}

pub async fn open(endpoint: &str) -> Result<Transport, ClientError> {
    if endpoint.starts_with("ws://") || endpoint.starts_with("wss://") {
        return crate::websocket::connect(endpoint).await;
    }
    #[cfg(feature = "mock-backend")]
    if let Some(name) = endpoint.strip_prefix("mem://") {
        return memory::connect(name);
    }
    Err(ClientError::NetworkError(format!("unsupported endpoint: {endpoint}")))
}

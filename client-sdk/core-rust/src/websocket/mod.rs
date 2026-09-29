//! WebSocket transport (tokio-tungstenite), bridged onto the channel-based
//! `Transport` so the connection logic is transport-agnostic.

use crate::generated::ClientError;
use crate::transport::{pair, Transport};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

pub async fn connect(url: &str) -> Result<Transport, ClientError> {
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .map_err(|e| ClientError::NetworkError(format!("websocket connect failed: {e}")))?;
    Ok(bridge(ws))
}

/// Accepts one server-side WebSocket (used by the mock server binary).
pub async fn accept<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(stream: S) -> Result<Transport, ClientError> {
    let ws = tokio_tungstenite::accept_async(stream)
        .await
        .map_err(|e| ClientError::NetworkError(format!("websocket accept failed: {e}")))?;
    Ok(bridge(ws))
}

fn bridge<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(ws: WebSocketStream<S>) -> Transport {
    let (local, remote) = pair(256);
    let Transport { outgoing: to_core, incoming: mut from_core } = remote;
    let (mut sink, mut source) = ws.split();
    // What the core writes to `local.outgoing` arrives in `from_core`.
    tokio::spawn(async move {
        while let Some(text) = from_core.recv().await {
            if sink.send(Message::text(text)).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    tokio::spawn(async move {
        while let Some(Ok(msg)) = source.next().await {
            match msg {
                Message::Text(t) => {
                    if to_core.send(t.to_string()).await.is_err() {
                        break;
                    }
                }
                Message::Close(_) => break,
                _ => {} // ping/pong handled by tungstenite; binary unused
            }
        }
        // dropping `to_core` signals EOF to the connection reader
    });
    local
}

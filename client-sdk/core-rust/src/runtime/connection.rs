use super::{Correlator, EventHub};
use crate::crypto;
use crate::generated::{ClientError, ClientEvent};
use crate::protocol::{ClientFrame, EventBody, ServerFrame};
use crate::transport::Transport;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// One authenticated backend connection. Requests are multiplexed over it and
/// matched to responses by id; server-pushed events go to the `EventHub`.
pub struct Connection {
    outgoing: mpsc::Sender<String>,
    correlator: Arc<Correlator>,
    shutdown: CancellationToken,
    closed: CancellationToken,
}

impl Connection {
    pub async fn establish(mut t: Transport, api_key: Option<&str>, events: Arc<EventHub>) -> Result<Arc<Self>, ClientError> {
        tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(&mut t, api_key))
            .await
            .map_err(|_| ClientError::NetworkError("handshake timed out".into()))??;

        let conn = Arc::new(Connection {
            outgoing: t.outgoing,
            correlator: Arc::default(),
            shutdown: CancellationToken::new(),
            closed: CancellationToken::new(),
        });
        tokio::spawn(read_loop(t.incoming, conn.correlator.clone(), events, conn.shutdown.clone(), conn.closed.clone()));
        Ok(conn)
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Option<Duration>) -> Result<Value, ClientError> {
        let pending = self.correlator.register()?;
        let frame = ClientFrame::Request { id: pending.id, method: method.into(), params };
        let text = serde_json::to_string(&frame).map_err(|e| ClientError::ProtocolError(e.to_string()))?;
        self.outgoing.send(text).await.map_err(|_| ClientError::Disconnected)?;
        // `pending` lives inside the future: if the caller is cancelled, its
        // Drop removes the correlation entry.
        let reply = async move {
            let mut pending = pending;
            (&mut pending.rx).await.unwrap_or(Err(ClientError::Disconnected))
        };
        match timeout {
            Some(t) => tokio::time::timeout(t, reply)
                .await
                .map_err(|_| ClientError::NetworkError(format!("request timed out after {} ms", t.as_millis())))?,
            None => reply.await,
        }
    }

    pub fn close(&self) {
        self.shutdown.cancel();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    /// Resolves once the read loop ended (server hung up or `close()`).
    pub async fn closed(&self) {
        self.closed.cancelled().await
    }
}

async fn handshake(t: &mut Transport, api_key: Option<&str>) -> Result<(), ClientError> {
    let nonce = match recv(t).await? {
        ServerFrame::Challenge { nonce } => nonce,
        other => return Err(ClientError::ProtocolError(format!("expected challenge, got {other:?}"))),
    };
    let mac = api_key.map(|k| crypto::sign(k, &nonce)).unwrap_or_default();
    let auth = serde_json::to_string(&ClientFrame::Auth { mac }).unwrap();
    t.outgoing.send(auth).await.map_err(|_| ClientError::Disconnected)?;
    match recv(t).await? {
        ServerFrame::AuthOk => Ok(()),
        ServerFrame::AuthFailed { .. } => Err(ClientError::Unauthorized),
        other => Err(ClientError::ProtocolError(format!("expected auth result, got {other:?}"))),
    }
}

async fn recv(t: &mut Transport) -> Result<ServerFrame, ClientError> {
    let text = t.incoming.recv().await.ok_or(ClientError::Disconnected)?;
    serde_json::from_str(&text).map_err(|e| ClientError::ProtocolError(format!("bad frame: {e}")))
}

async fn read_loop(
    mut incoming: mpsc::Receiver<String>,
    correlator: Arc<Correlator>,
    events: Arc<EventHub>,
    shutdown: CancellationToken,
    closed: CancellationToken,
) {
    loop {
        let text = tokio::select! {
            _ = shutdown.cancelled() => break,
            t = incoming.recv() => match t { Some(t) => t, None => break },
        };
        match serde_json::from_str::<ServerFrame>(&text) {
            Ok(ServerFrame::Response { id, ok, error }) => {
                let reply = match error {
                    Some(e) => Err(e.into()),
                    None => Ok(ok.unwrap_or(Value::Null)),
                };
                correlator.resolve(id, reply);
            }
            Ok(ServerFrame::Event { event }) => events.publish(match event {
                EventBody::CustomerUpdated { customer } => ClientEvent::CustomerUpdated(customer.into()),
                EventBody::MessageReceived { text } => ClientEvent::MessageReceived(text),
            }),
            Ok(_) | Err(_) => { /* unexpected frame: ignore, keep the connection */ }
        }
    }
    correlator.close(ClientError::Disconnected);
    closed.cancel();
}

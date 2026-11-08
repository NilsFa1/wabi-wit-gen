//! One session with the backend: every request body is sealed, every request
//! is signed by the app-held key (if the client has one), and one long-lived
//! response delivers the backend's events.

use crate::crypto::{self, BodyKey, AAD_EVENT, AAD_REQUEST, AAD_RESPONSE};
use crate::events::EventHub;
use crate::generated::{host_keys, ClientError, ClientEvent, ClientOptions};
use crate::protocol::{self, b64, unb64, ErrorBody, EventBody, SessionDto};
use crate::api::Link;
use crate::transport::{Chunks, Request};
use futures::StreamExt;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub struct Session {
    link: Arc<Link>,
    key: BodyKey,
    /// Name of the app-held key that signs every request.
    key_id: Option<String>,
    id: String,
    /// Cancelled once the event channel has ended: by `close()` or by the backend.
    closed: CancellationToken,
}

fn key_error(what: &str, e: host_keys::HostError) -> ClientError {
    ClientError::KeyError(format!("host-keys.{what}: {e}"))
}

impl Session {
    pub async fn open(options: &ClientOptions, link: Arc<Link>, events: Arc<EventHub>) -> Result<Arc<Session>, ClientError> {
        let mut session = Session {
            link,
            key: BodyKey::derive(options.api_key.as_deref().unwrap_or_default()),
            key_id: options.key_id.clone(),
            id: String::new(),
            closed: CancellationToken::new(),
        };
        let mut headers = vec![];
        if let Some(key_id) = &session.key_id {
            // The app's keystore answers; the private key never comes here.
            let spki = host_keys::public_key(key_id.clone()).await.map_err(|e| key_error("public-key", e))??;
            headers.push((protocol::HEADER_KEY, b64(&spki)));
        }
        let hello = json!({ "client": "demo-rust-core" });
        let opened: SessionDto = session.exchange("POST", protocol::PATH_SESSIONS, headers, Some(hello)).await?;
        session.id = opened.session;

        let request = session.request("GET", protocol::PATH_EVENTS, vec![], None).await?;
        let (status, chunks) = session.link.open_stream(request).await?;
        if status != 200 {
            return Err(ClientError::ProtocolError(format!("event channel: HTTP {status}")));
        }
        let session = Arc::new(session);
        tokio::spawn(pump(chunks, session.key.clone(), events, session.closed.clone()));
        Ok(session)
    }

    /// One request/response exchange. Fails with `disconnected` once the session is closed.
    pub async fn call<T: DeserializeOwned>(
        &self,
        method: &'static str,
        path: &str,
        body: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<T, ClientError> {
        let exchange = async {
            tokio::select! {
                r = self.exchange(method, path, vec![], body) => r,
                _ = self.closed.cancelled() => Err(ClientError::Disconnected),
            }
        };
        match timeout {
            Some(t) => tokio::time::timeout(t, exchange)
                .await
                .map_err(|_| ClientError::NetworkError(format!("request timed out after {} ms", t.as_millis())))?,
            None => exchange.await,
        }
    }

    pub fn close(&self) {
        self.closed.cancel();
    }

    pub fn is_closed(&self) -> bool {
        self.closed.is_cancelled()
    }

    /// Resolves once the session has ended (backend hung up or `close()`).
    pub async fn closed(&self) {
        self.closed.cancelled().await
    }

    /// Seals the body and signs the request.
    async fn request(
        &self,
        method: &'static str,
        path: &str,
        mut headers: Vec<(&'static str, String)>,
        body: Option<Value>,
    ) -> Result<Request, ClientError> {
        let body = body.map(|json| self.key.seal(AAD_REQUEST, json.to_string().as_bytes())).unwrap_or_default();
        if !self.id.is_empty() {
            headers.push((protocol::HEADER_SESSION, self.id.clone()));
        }
        if let Some(key_id) = &self.key_id {
            let digest = crypto::request_digest(method, path, &self.id, &body);
            let signature = host_keys::sign(key_id.clone(), digest).await.map_err(|e| key_error("sign", e))??;
            headers.push((protocol::HEADER_SIGNATURE, b64(&signature)));
        }
        Ok(Request { method, path: path.to_string(), headers, body })
    }

    async fn exchange<T: DeserializeOwned>(
        &self,
        method: &'static str,
        path: &str,
        headers: Vec<(&'static str, String)>,
        body: Option<Value>,
    ) -> Result<T, ClientError> {
        let request = self.request(method, path, headers, body).await?;
        let (status, sealed) = self.link.send(request).await?;
        let opened = self.key.open(AAD_RESPONSE, &sealed);
        match (status, opened) {
            (200, Some(json)) => protocol::parse(&json),
            (200, None) => Err(ClientError::ProtocolError("response cannot be decrypted".into())),
            // A backend with another key cannot even tell us why.
            (401, _) => Err(ClientError::Unauthorized),
            (_, Some(json)) => Err(protocol::parse::<ErrorBody>(&json)?.into()),
            (_, None) => Err(ClientError::ProtocolError(format!("HTTP {status}"))),
        }
    }
}

/// Reads server-sent events (`data: <base64 sealed JSON>` + blank line) until
/// the response ends or the session is closed.
async fn pump(mut chunks: Chunks, key: BodyKey, events: Arc<EventHub>, closed: CancellationToken) {
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        let chunk = tokio::select! {
            _ = closed.cancelled() => break,
            c = chunks.next() => match c { Some(Ok(c)) => c, _ => break },
        };
        buffer.extend_from_slice(&chunk);
        while let Some(end) = buffer.windows(2).position(|w| w == b"\n\n") {
            let frame: Vec<u8> = buffer.drain(..end + 2).collect();
            for line in String::from_utf8_lossy(&frame).lines() {
                let Some(data) = line.strip_prefix("data: ") else { continue }; // comments, keep-alives
                let event = unb64(data).ok().and_then(|sealed| key.open(AAD_EVENT, &sealed));
                match event.and_then(|json| serde_json::from_slice::<EventBody>(&json).ok()) {
                    Some(EventBody::CustomerUpdated { customer }) => events.publish(ClientEvent::CustomerUpdated(customer.into())),
                    Some(EventBody::MessageReceived { text }) => events.publish(ClientEvent::MessageReceived(text)),
                    None => {} // not for us or from a newer backend: skip
                }
            }
        }
    }
    closed.cancel();
}

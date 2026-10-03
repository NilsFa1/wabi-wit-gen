//! A tiny mock of the Wabi backend, speaking the real backend protocol.
//! Served in-process for `mem://` endpoints and over TCP by the
//! `wabi-mock-server` binary (for real WebSocket end-to-end tests).
//!
//! Behaviour that the cross-language test suites rely on:
//!   * API key `demo-key` (HMAC challenge/response), anything else -> unauthorized
//!   * customer `4711` exists; unknown ids -> not-found
//!   * create-customer emits `customer-updated`; empty name -> protocol-error
//!   * send-command: `echo` returns the payload, `broadcast` emits
//!     `message-received(payload as UTF-8)`, `sleep` waits <payload> ms,
//!     `fail` -> protocol-error, `hang-up` closes the connection.

use crate::crypto;
use crate::protocol::{b64, unb64, ClientFrame, CommandDto, CustomerDto, ErrorBody, EventBody, ServerFrame};
use crate::transport::Transport;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const DEMO_API_KEY: &str = "demo-key";

#[derive(Clone)]
pub struct MockConfig {
    pub api_key: String,
}

impl Default for MockConfig {
    fn default() -> Self {
        MockConfig { api_key: DEMO_API_KEY.into() }
    }
}

struct Store {
    customers: HashMap<String, CustomerDto>,
    next_id: u64,
}

pub async fn serve(mut t: Transport, cfg: MockConfig) {
    let out = t.outgoing.clone();
    let send = |f: ServerFrame| {
        let out = out.clone();
        async move { out.send(serde_json::to_string(&f).unwrap()).await.is_ok() }
    };

    let nonce = crypto::new_nonce();
    if !send(ServerFrame::Challenge { nonce: nonce.clone() }).await {
        return;
    }
    let authed = match t.incoming.recv().await.and_then(|s| serde_json::from_str::<ClientFrame>(&s).ok()) {
        Some(ClientFrame::Auth { mac }) => crypto::verify(&cfg.api_key, &nonce, &mac),
        _ => false,
    };
    if !authed {
        send(ServerFrame::AuthFailed { reason: "invalid credentials".into() }).await;
        return;
    }
    send(ServerFrame::AuthOk).await;

    let store = Arc::new(Mutex::new(Store {
        customers: HashMap::from([(
            "4711".to_string(),
            CustomerDto { id: "4711".into(), name: "Erika Mustermann".into(), email: Some("erika@example.com".into()) },
        )]),
        next_id: 5000,
    }));
    let (hangup_tx, mut hangup_rx) = mpsc::channel::<()>(1);
    // Cancels in-flight request tasks on hang-up, so every clone of the
    // outgoing sender is dropped and the client observes EOF.
    let alive = CancellationToken::new();
    let _cancel_on_exit = alive.clone().drop_guard();

    loop {
        let text = tokio::select! {
            _ = hangup_rx.recv() => return,
            t = t.incoming.recv() => match t { Some(t) => t, None => return },
        };
        let Ok(ClientFrame::Request { id, method, params }) = serde_json::from_str::<ClientFrame>(&text) else {
            continue;
        };
        let out = t.outgoing.clone();
        let store = store.clone();
        let hangup = hangup_tx.clone();
        let alive = alive.clone();
        // One task per request: slow requests never block fast ones.
        tokio::spawn(async move {
            let (result, events) = tokio::select! {
                r = handle(&method, params, &store, &hangup) => r,
                _ = alive.cancelled() => return,
            };
            for ev in events {
                let _ = out.send(serde_json::to_string(&ServerFrame::Event { event: ev }).unwrap()).await;
            }
            let frame = match result {
                Ok(v) => ServerFrame::Response { id, ok: Some(v), error: None },
                Err(e) => ServerFrame::Response { id, ok: None, error: Some(e) },
            };
            let _ = out.send(serde_json::to_string(&frame).unwrap()).await;
        });
    }
}

async fn handle(
    method: &str,
    p: Value,
    store: &Mutex<Store>,
    hangup: &mpsc::Sender<()>,
) -> (Result<Value, ErrorBody>, Vec<EventBody>) {
    let s = |k: &str| p.get(k).and_then(Value::as_str).map(str::to_string);
    match method {
        "get-customer" => {
            let id = s("id").unwrap_or_default();
            match store.lock().unwrap().customers.get(&id) {
                Some(c) => (Ok(json!(c)), vec![]),
                None => (Err(ErrorBody::new("not-found", None)), vec![]),
            }
        }
        "create-customer" => {
            let name = s("name").unwrap_or_default();
            if name.trim().is_empty() {
                return (Err(ErrorBody::new("invalid-argument", Some("name must not be empty".into()))), vec![]);
            }
            let c = {
                let mut st = store.lock().unwrap();
                st.next_id += 1;
                let c = CustomerDto { id: st.next_id.to_string(), name, email: s("email") };
                st.customers.insert(c.id.clone(), c.clone());
                c
            };
            (Ok(json!(c)), vec![EventBody::CustomerUpdated { customer: c }])
        }
        "send-command" => {
            let cmd: CommandDto = match serde_json::from_value(p) {
                Ok(c) => c,
                Err(e) => return (Err(ErrorBody::new("bad-request", Some(e.to_string()))), vec![]),
            };
            let payload = unb64(&cmd.payload).unwrap_or_default();
            let reply = |payload: &[u8]| Ok(json!(CommandDto { name: cmd.name.clone(), payload: b64(payload) }));
            match cmd.name.as_str() {
                "echo" => (reply(&payload), vec![]),
                "broadcast" => {
                    let text = String::from_utf8_lossy(&payload).into_owned();
                    (reply(&[]), vec![EventBody::MessageReceived { text }])
                }
                "sleep" => {
                    let ms: u64 = String::from_utf8_lossy(&payload).parse().unwrap_or(0);
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    (reply(&payload), vec![])
                }
                "fail" => (Err(ErrorBody::new("command-failed", Some("command failed on purpose".into()))), vec![]),
                "hang-up" => {
                    let _ = hangup.try_send(());
                    // never answered: the client must fail it with `disconnected`
                    std::future::pending::<()>().await;
                    unreachable!()
                }
                other => (Err(ErrorBody::new("unknown-command", Some(format!("unknown command `{other}`")))), vec![]),
            }
        }
        other => (Err(ErrorBody::new("unknown-method", Some(format!("unknown method `{other}`")))), vec![]),
    }
}

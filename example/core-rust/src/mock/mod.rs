//! The mock service that goes with this client. It speaks the real protocol
//! (see `protocol`): it decrypts and encrypts every body and verifies every
//! signature. It is independent of how requests reach it:
//!
//!   * in-process for `mem://` endpoints (`Mock::shared`),
//!   * over HTTP in the `demo-mock-server` binary (`server`, feature `mock-server`).
//!
//! Behaviour that the cross-language test suites rely on:
//!   * shared secret `demo-key`; a body sealed with anything else -> 401
//!   * a session opened with a key must sign every request with it -> else 401
//!   * every session has its own customers; `4711` exists, unknown ids -> 404
//!   * create-customer emits `customer-updated`; empty name -> 400
//!   * commands: `echo` returns the payload, `broadcast` emits
//!     `message-received(payload as UTF-8)`, `sleep` waits <payload> ms,
//!     `fail` -> 400, `hang-up` ends the session without an answer.

use crate::crypto::{self, BodyKey, AAD_EVENT, AAD_REQUEST, AAD_RESPONSE};
use crate::protocol::{self, b64, unb64, CommandDto, CustomerDto, ErrorBody, EventBody, SessionDto, WhoamiDto};
use p256::ecdsa::signature::hazmat::PrehashVerifier;
use p256::ecdsa::{Signature, VerifyingKey};
use p256::pkcs8::DecodePublicKey;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;

#[cfg(feature = "mock-server")]
pub mod server;

pub const DEMO_API_KEY: &str = "demo-key";

/// One HTTP request as the mock sees it (path incl. query, header names in lower case).
pub struct Call {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

pub struct Reply {
    pub status: u16,
    pub body: ReplyBody,
}

pub enum ReplyBody {
    Full(Vec<u8>),
    /// `text/event-stream`: chunks until the session ends.
    Events(mpsc::Receiver<Vec<u8>>),
}

pub struct Mock {
    key: BodyKey,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

struct Session {
    /// The key the session was opened with (SubjectPublicKeyInfo).
    key: Option<(Vec<u8>, VerifyingKey)>,
    customers: Mutex<HashMap<String, CustomerDto>>,
    next_id: Mutex<u64>,
    /// The open event channel of this session.
    events: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
}

type Failure = (u16, ErrorBody);

fn fail(status: u16, code: &str, message: &str) -> Failure {
    (status, ErrorBody::new(code, Some(message.to_string())))
}

fn unauthorized(message: &str) -> Failure {
    fail(401, "unauthorized", message)
}

impl Mock {
    pub fn new(api_key: &str) -> Arc<Mock> {
        Arc::new(Mock { key: BodyKey::derive(api_key), sessions: Mutex::default() })
    }

    /// The mock behind every `mem://` endpoint of this process.
    pub fn shared() -> Arc<Mock> {
        static SHARED: OnceLock<Arc<Mock>> = OnceLock::new();
        SHARED.get_or_init(|| Mock::new(DEMO_API_KEY)).clone()
    }

    pub async fn handle(&self, call: Call) -> Reply {
        match self.route(call).await {
            Ok(reply) => reply,
            Err((status, error)) => self.sealed(status, &error),
        }
    }

    fn sealed(&self, status: u16, body: &impl Serialize) -> Reply {
        let json = serde_json::to_vec(body).expect("serializable");
        Reply { status, body: ReplyBody::Full(self.key.seal(AAD_RESPONSE, &json)) }
    }

    fn ok(&self, body: &impl Serialize) -> Result<Reply, Failure> {
        Ok(self.sealed(200, body))
    }

    /// The decrypted JSON body of a request.
    fn body(&self, call: &Call) -> Result<Value, Failure> {
        let json = self.key.open(AAD_REQUEST, &call.body).ok_or_else(|| unauthorized("body cannot be decrypted"))?;
        serde_json::from_slice(&json).map_err(|e| fail(400, "bad-request", &e.to_string()))
    }

    async fn route(&self, call: Call) -> Result<Reply, Failure> {
        let method = call.method.as_str();
        if (method, call.path.as_str()) == ("POST", protocol::PATH_SESSIONS) {
            return self.open_session(&call);
        }
        let id = call.headers.get(protocol::HEADER_SESSION).ok_or_else(|| unauthorized("no session"))?;
        let session = self.sessions.lock().unwrap().get(id).cloned().ok_or_else(|| unauthorized("unknown session"))?;
        if let Some((_, key)) = &session.key {
            verify(key, &call, id)?;
        }
        match (method, call.path.as_str()) {
            ("GET", protocol::PATH_EVENTS) => {
                let (tx, rx) = mpsc::channel(1024);
                let _ = tx.try_send(b": ready\n\n".to_vec());
                *session.events.lock().unwrap() = Some(tx);
                Ok(Reply { status: 200, body: ReplyBody::Events(rx) })
            }
            ("GET", protocol::PATH_WHOAMI) => match &session.key {
                Some((spki, _)) => self.ok(&WhoamiDto { key: crypto::fingerprint(spki) }),
                None => Err(unauthorized("this session is not bound to a key")),
            },
            ("POST", protocol::PATH_CUSTOMERS) => {
                let body = self.body(&call)?;
                let text = |k: &str| body.get(k).and_then(Value::as_str).map(str::to_string);
                let name = text("name").unwrap_or_default();
                if name.trim().is_empty() {
                    return Err(fail(400, "invalid-argument", "name must not be empty"));
                }
                let customer = {
                    let mut next_id = session.next_id.lock().unwrap();
                    *next_id += 1;
                    CustomerDto { id: next_id.to_string(), name, email: text("email") }
                };
                session.customers.lock().unwrap().insert(customer.id.clone(), customer.clone());
                self.emit(id, &session, EventBody::CustomerUpdated { customer: customer.clone() });
                self.ok(&customer)
            }
            ("POST", protocol::PATH_COMMANDS) => {
                let command: CommandDto = serde_json::from_value(self.body(&call)?).map_err(|e| fail(400, "bad-request", &e.to_string()))?;
                self.command(id, &session, command).await
            }
            ("GET", path) => {
                let id = path
                    .strip_prefix(protocol::PATH_CUSTOMERS)
                    .and_then(|rest| rest.strip_prefix("?id="))
                    .and_then(protocol::decode_query_value)
                    .ok_or_else(|| fail(404, "not-found", "no such resource"))?;
                match session.customers.lock().unwrap().get(&id) {
                    Some(customer) => self.ok(customer),
                    None => Err(fail(404, "not-found", "no such customer")),
                }
            }
            _ => Err(fail(404, "not-found", "no such resource")),
        }
    }

    fn open_session(&self, call: &Call) -> Result<Reply, Failure> {
        self.body(call)?; // proves that the client has the shared secret
        let key = match call.headers.get(protocol::HEADER_KEY) {
            None => None,
            Some(encoded) => {
                let spki = unb64(encoded).map_err(|_| unauthorized("malformed key"))?;
                let key = VerifyingKey::from_public_key_der(&spki).map_err(|_| unauthorized("not an EC P-256 key"))?;
                verify(&key, call, "")?; // proves that the client can use the key
                Some((spki, key))
            }
        };
        let id = crypto::random_hex(16);
        let erika = CustomerDto { id: "4711".into(), name: "Erika Mustermann".into(), email: Some("erika@example.com".into()) };
        let session = Session {
            key,
            customers: Mutex::new(HashMap::from([(erika.id.clone(), erika)])),
            next_id: Mutex::new(5000),
            events: Mutex::default(),
        };
        self.sessions.lock().unwrap().insert(id.clone(), Arc::new(session));
        self.ok(&SessionDto { session: id })
    }

    async fn command(&self, id: &str, session: &Session, command: CommandDto) -> Result<Reply, Failure> {
        let payload = unb64(&command.payload).unwrap_or_default();
        let reply = |payload: &[u8]| self.ok(&CommandDto { name: command.name.clone(), payload: b64(payload) });
        match command.name.as_str() {
            "echo" => reply(&payload),
            "broadcast" => {
                self.emit(id, session, EventBody::MessageReceived { text: String::from_utf8_lossy(&payload).into_owned() });
                reply(&[])
            }
            "sleep" => {
                let ms: u64 = String::from_utf8_lossy(&payload).parse().unwrap_or(0);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                reply(&payload)
            }
            "fail" => Err(fail(400, "command-failed", "command failed on purpose")),
            "hang-up" => {
                self.end(id, session);
                // never answered: the client must fail the call with `disconnected`
                std::future::pending::<()>().await;
                unreachable!()
            }
            other => Err(fail(400, "unknown-command", &format!("unknown command `{other}`"))),
        }
    }

    /// Pushes an event to the session's event channel (if it is open).
    fn emit(&self, id: &str, session: &Session, event: EventBody) {
        let sealed = self.key.seal(AAD_EVENT, &serde_json::to_vec(&event).expect("serializable"));
        let frame = format!("data: {}\n\n", b64(&sealed)).into_bytes();
        let gone = match session.events.lock().unwrap().as_ref() {
            Some(tx) => matches!(tx.try_send(frame), Err(mpsc::error::TrySendError::Closed(_))),
            None => false,
        };
        if gone {
            self.end(id, session); // the client went away
        }
    }

    fn end(&self, id: &str, session: &Session) {
        self.sessions.lock().unwrap().remove(id);
        session.events.lock().unwrap().take(); // ends the event stream
    }
}

/// Checks `x-demo-signature` against the request as it arrived.
fn verify(key: &VerifyingKey, call: &Call, session: &str) -> Result<(), Failure> {
    let signature = call
        .headers
        .get(protocol::HEADER_SIGNATURE)
        .and_then(|s| unb64(s).ok())
        .and_then(|der| Signature::from_der(&der).ok())
        .ok_or_else(|| unauthorized("request is not signed"))?;
    let digest = crypto::request_digest(&call.method, &call.path, session, &call.body);
    key.verify_prehash(&digest, &signature).map_err(|_| unauthorized("signature does not match the request"))
}

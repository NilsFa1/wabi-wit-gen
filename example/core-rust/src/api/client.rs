use wabi_abi_rt::host::AbiStream;
use crate::generated::{
    ClientError, ClientEvent, ClientOptions, ClientResource, Command, CommandResult, ConnectionState, Customer,
};
use crate::api::{Link, SharedTransport};
use crate::events::EventHub;
use crate::protocol::{self, b64, CommandDto, CustomerDto, WhoamiDto};
use crate::session::Session;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A client owning one session with the backend.
///
/// Cheap to share (`Arc` inside). All methods are safe to call concurrently;
/// every call is its own HTTP request.
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    options: ClientOptions,
    state: Mutex<ConnectionState>,
    events: Arc<EventHub>,
    /// What the next session sends its requests over; a private one unless
    /// the host gave the client a shared transport.
    link: Mutex<Option<Arc<Link>>>,
    /// Current session (std mutex: only held for clone/replace).
    current: Mutex<Option<Arc<Session>>>,
    /// Serialises connect/disconnect.
    lifecycle: tokio::sync::Mutex<()>,
    /// Bumped whenever the current session is replaced/closed on purpose,
    /// so the watcher of an old session does not report a stale loss.
    generation: AtomicU64,
    /// Cancelled when the client is closed for good (handle dropped).
    closed: CancellationToken,
}

impl Inner {
    fn set_state(&self, s: ConnectionState) {
        let changed = {
            let mut cur = self.state.lock().unwrap();
            std::mem::replace(&mut *cur, s) != s
        };
        if changed {
            self.events.publish(ClientEvent::ConnectionStateChanged(s));
        }
    }

    fn session(&self) -> Result<Arc<Session>, ClientError> {
        match self.current.lock().unwrap().as_ref() {
            Some(c) if !c.is_closed() => Ok(c.clone()),
            _ => Err(ClientError::Disconnected),
        }
    }

    fn timeout(&self) -> Option<Duration> {
        match self.options.request_timeout_ms {
            0 => None,
            ms => Some(Duration::from_millis(ms as u64)),
        }
    }
}

/// Restores `disconnected` if a connect attempt is abandoned mid-way
/// (e.g. the host cancelled the future).
struct ConnectingGuard<'a>(&'a Inner, bool);
impl Drop for ConnectingGuard<'_> {
    fn drop(&mut self) {
        if self.1 {
            self.0.set_state(ConnectionState::Disconnected);
        }
    }
}

impl Client {
    /// A client on a given transport (`None`: its own, to `options.endpoint`).
    pub fn with_link(options: ClientOptions, link: Option<Arc<Link>>) -> Client {
        Client {
            inner: Arc::new(Inner {
                options,
                state: Mutex::new(ConnectionState::Disconnected),
                events: Arc::default(),
                link: Mutex::new(link),
                current: Mutex::new(None),
                lifecycle: tokio::sync::Mutex::new(()),
                generation: AtomicU64::new(0),
                closed: CancellationToken::new(),
            }),
        }
    }

    async fn call<T: DeserializeOwned>(&self, method: &'static str, path: &str, body: Option<Value>) -> Result<T, ClientError> {
        let session = self.inner.session()?;
        session.call(method, path, body, self.inner.timeout()).await
    }

    /// Closes the client for good: aborts in-flight work, ends all event
    /// streams. Idempotent. Called when the host drops the handle.
    pub fn close(&self) {
        let inner = &self.inner;
        if inner.closed.is_cancelled() {
            return;
        }
        inner.closed.cancel();
        inner.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(c) = inner.current.lock().unwrap().take() {
            c.close();
        }
        inner.set_state(ConnectionState::Disconnected);
        inner.events.close();
    }
}

impl ClientResource for Client {
    type Transport = SharedTransport;

    fn new(options: ClientOptions) -> Self {
        Client::with_link(options, None)
    }

    fn use_transport(&self, transport: Arc<SharedTransport>) {
        *self.inner.link.lock().unwrap() = Some(transport.link.clone());
    }

    fn state(&self) -> ConnectionState {
        *self.inner.state.lock().unwrap()
    }

    async fn connect(&self) -> Result<(), ClientError> {
        let inner = &self.inner;
        let _lifecycle = inner.lifecycle.lock().await;
        if inner.closed.is_cancelled() {
            return Err(ClientError::Disconnected);
        }
        if inner.session().is_ok() {
            return Ok(()); // already connected: idempotent
        }
        inner.set_state(ConnectionState::Connecting);
        let mut guard = ConnectingGuard(inner, true);

        let link = inner.link.lock().unwrap().clone().unwrap_or_else(|| Link::new(inner.options.endpoint.clone()));
        let session = tokio::select! {
            r = Session::open(&inner.options, link, inner.events.clone()) => r?,
            _ = inner.closed.cancelled() => return Err(ClientError::Disconnected),
        };

        let gen = inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *inner.current.lock().unwrap() = Some(session.clone());
        guard.1 = false;
        inner.set_state(ConnectionState::Connected);

        // Report a session ended by the backend or the network.
        let weak = Arc::downgrade(inner);
        tokio::spawn(async move {
            session.closed().await;
            if let Some(inner) = weak.upgrade() {
                if inner.generation.load(Ordering::SeqCst) == gen {
                    inner.current.lock().unwrap().take();
                    inner.set_state(ConnectionState::Disconnected);
                }
            }
        });
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), ClientError> {
        let inner = &self.inner;
        let _lifecycle = inner.lifecycle.lock().await;
        inner.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(c) = inner.current.lock().unwrap().take() {
            c.close();
        }
        inner.set_state(ConnectionState::Disconnected);
        Ok(())
    }

    async fn key_fingerprint(&self) -> Result<String, ClientError> {
        Ok(self.call::<WhoamiDto>("GET", protocol::PATH_WHOAMI, None).await?.key)
    }

    async fn get_customer(&self, id: String) -> Result<Customer, ClientError> {
        let path = format!("{}?id={}", protocol::PATH_CUSTOMERS, protocol::query_value(&id));
        Ok(self.call::<CustomerDto>("GET", &path, None).await?.into())
    }

    async fn create_customer(&self, name: String, email: Option<String>) -> Result<Customer, ClientError> {
        let body = json!({ "name": name, "email": email });
        Ok(self.call::<CustomerDto>("POST", protocol::PATH_CUSTOMERS, Some(body)).await?.into())
    }

    async fn send_command(&self, command: Command) -> Result<CommandResult, ClientError> {
        let body = json!({ "name": command.name, "payload": b64(&command.payload) });
        protocol::command_result(self.call::<CommandDto>("POST", protocol::PATH_COMMANDS, Some(body)).await?)
    }

    fn events(&self) -> AbiStream<ClientEvent> {
        self.inner.events.subscribe()
    }

    fn on_drop(&self) {
        self.close();
    }
}

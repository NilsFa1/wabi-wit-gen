use crate::ffi::host::AbiStream;
use crate::generated::{
    ClientError, ClientEvent, ClientOptions, ClientResource, Command, CommandResult, ConnectionState, Customer,
};
use crate::protocol::{self, b64, CommandDto, CustomerDto};
use crate::runtime::{Connection, EventHub};
use crate::transport;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A client owning one long-lived backend connection.
///
/// Cheap to share (`Arc` inside). All methods are safe to call concurrently;
/// requests are multiplexed over the single connection.
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    options: ClientOptions,
    state: Mutex<ConnectionState>,
    events: Arc<EventHub>,
    /// Current connection (std mutex: only held for clone/replace).
    current: Mutex<Option<Arc<Connection>>>,
    /// Serialises connect/disconnect.
    lifecycle: tokio::sync::Mutex<()>,
    /// Bumped whenever the current connection is replaced/closed on purpose,
    /// so the watcher of an old connection does not report a stale loss.
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

    fn connection(&self) -> Result<Arc<Connection>, ClientError> {
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
    async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, ClientError> {
        let conn = self.inner.connection()?;
        conn.request(method, params, self.inner.timeout()).await
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
    fn new(options: ClientOptions) -> Self {
        Client {
            inner: Arc::new(Inner {
                options,
                state: Mutex::new(ConnectionState::Disconnected),
                events: Arc::default(),
                current: Mutex::new(None),
                lifecycle: tokio::sync::Mutex::new(()),
                generation: AtomicU64::new(0),
                closed: CancellationToken::new(),
            }),
        }
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
        if inner.connection().is_ok() {
            return Ok(()); // already connected: idempotent
        }
        inner.set_state(ConnectionState::Connecting);
        let mut guard = ConnectingGuard(inner, true);

        let attempt = async {
            let t = transport::open(&inner.options.endpoint).await?;
            Connection::establish(t, inner.options.api_key.as_deref(), inner.events.clone()).await
        };
        let conn = tokio::select! {
            r = attempt => r?,
            _ = inner.closed.cancelled() => return Err(ClientError::Disconnected),
        };

        let gen = inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *inner.current.lock().unwrap() = Some(conn.clone());
        guard.1 = false;
        inner.set_state(ConnectionState::Connected);

        // Report connection loss initiated by the server/network.
        let weak = Arc::downgrade(inner);
        tokio::spawn(async move {
            conn.closed().await;
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

    async fn get_customer(&self, id: String) -> Result<Customer, ClientError> {
        let v = self.call("get-customer", json!({ "id": id })).await?;
        Ok(protocol::parse::<CustomerDto>(v)?.into())
    }

    async fn create_customer(&self, name: String, email: Option<String>) -> Result<Customer, ClientError> {
        let v = self.call("create-customer", json!({ "name": name, "email": email })).await?;
        Ok(protocol::parse::<CustomerDto>(v)?.into())
    }

    async fn send_command(&self, command: Command) -> Result<CommandResult, ClientError> {
        let dto = CommandDto { name: command.name, payload: b64(&command.payload) };
        let v = self.call("send-command", serde_json::to_value(dto).unwrap()).await?;
        protocol::command_result(v)
    }

    fn events(&self) -> AbiStream<ClientEvent> {
        self.inner.events.subscribe()
    }

    fn on_drop(&self) {
        self.close();
    }
}

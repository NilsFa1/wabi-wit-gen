use crate::api::Client;
use crate::generated::{ClientError, ClientOptions, TransportResource};
use crate::transport::{Chunks, Request, Transport};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// The connection pool to one backend, shared by every client that was given
/// this transport. This is the resource `transport` of the contract; the
/// channel itself (HTTP or the in-process mock) is `crate::transport`.
pub struct SharedTransport {
    pub link: Arc<Link>,
}

/// What a session sends its requests over.
pub struct Link {
    endpoint: String,
    /// Opened on first use: an unsupported endpoint fails `connect`, not the constructor.
    transport: OnceLock<Result<Transport, ClientError>>,
    requests: AtomicU64,
}

impl Link {
    pub fn new(endpoint: String) -> Arc<Link> {
        Arc::new(Link { endpoint, transport: OnceLock::new(), requests: AtomicU64::new(0) })
    }

    fn transport(&self) -> Result<&Transport, ClientError> {
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.transport.get_or_init(|| Transport::open(&self.endpoint)).as_ref().map_err(Clone::clone)
    }

    pub async fn send(&self, request: Request) -> Result<(u16, Vec<u8>), ClientError> {
        self.transport()?.send(request).await
    }

    pub async fn open_stream(&self, request: Request) -> Result<(u16, Chunks), ClientError> {
        self.transport()?.open_stream(request).await
    }
}

impl TransportResource for SharedTransport {
    type Client = Client;

    fn new(endpoint: String) -> Self {
        SharedTransport { link: Link::new(endpoint) }
    }

    fn request_count(&self) -> u64 {
        self.link.requests.load(Ordering::Relaxed)
    }

    fn open_client(&self, options: ClientOptions) -> Arc<Client> {
        Arc::new(Client::with_link(options, Some(self.link.clone())))
    }

    /// `other` was moved in by the host: nobody else can reach it any more.
    fn absorb(&self, other: Arc<Self>) {
        self.link.requests.fetch_add(other.request_count(), Ordering::Relaxed);
    }
}

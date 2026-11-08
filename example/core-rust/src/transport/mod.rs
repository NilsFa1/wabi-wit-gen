//! Transport abstraction: one HTTP exchange, or one long-lived response whose
//! body arrives in chunks (the event channel). Real HTTP and the in-process
//! mock look identical to the rest of the core.

use crate::generated::ClientError;
use futures::stream::BoxStream;

mod http;
#[cfg(feature = "mock-backend")]
mod memory;

pub struct Request {
    pub method: &'static str,
    pub path: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Vec<u8>,
}

/// Chunks of a response body, as they arrive.
pub type Chunks = BoxStream<'static, Result<Vec<u8>, ClientError>>;

pub enum Transport {
    Http(http::Http),
    #[cfg(feature = "mock-backend")]
    Memory(memory::Memory),
}

impl Transport {
    pub fn open(endpoint: &str) -> Result<Transport, ClientError> {
        if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            return Ok(Transport::Http(http::Http::new(endpoint)?));
        }
        #[cfg(feature = "mock-backend")]
        if endpoint.starts_with("mem://") {
            return Ok(Transport::Memory(memory::Memory::new()));
        }
        Err(ClientError::NetworkError(format!("unsupported endpoint: {endpoint}")))
    }

    /// Sends a request and reads the whole response: status and body.
    pub async fn send(&self, request: Request) -> Result<(u16, Vec<u8>), ClientError> {
        match self {
            Transport::Http(t) => t.send(request).await,
            #[cfg(feature = "mock-backend")]
            Transport::Memory(t) => t.send(request).await,
        }
    }

    /// Sends a request and returns as soon as the response has started.
    pub async fn open_stream(&self, request: Request) -> Result<(u16, Chunks), ClientError> {
        match self {
            Transport::Http(t) => t.open_stream(request).await,
            #[cfg(feature = "mock-backend")]
            Transport::Memory(t) => t.open_stream(request).await,
        }
    }
}

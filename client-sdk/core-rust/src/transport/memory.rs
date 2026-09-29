//! `mem://<name>` — connects to an in-process mock backend. Used by tests and
//! examples so every language can run end-to-end without a network.
//!
//!   mem://unreachable  -> fails like a refused TCP connection

use super::{pair, Transport};
use crate::generated::ClientError;
use crate::mock::{self, MockConfig};

pub fn connect(name: &str) -> Result<Transport, ClientError> {
    if name == "unreachable" {
        return Err(ClientError::NetworkError("connection refused".into()));
    }
    let (client, server) = pair(256);
    tokio::spawn(mock::serve(server, MockConfig::default()));
    Ok(client)
}

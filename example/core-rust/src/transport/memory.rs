//! `mem://<name>` — talks to the mock service in this process. Used by tests
//! and examples so every language can run end-to-end without a network. The
//! requests are byte for byte the ones the HTTP transport would send.

use super::{Chunks, Request};
use crate::generated::ClientError;
use crate::mock::{Call, Mock, ReplyBody};
use futures::StreamExt;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

pub struct Memory {
    mock: Arc<Mock>,
}

impl Memory {
    pub fn new() -> Memory {
        Memory { mock: Mock::shared() }
    }

    fn call(request: Request) -> Call {
        Call {
            method: request.method.to_string(),
            path: request.path,
            headers: request.headers.into_iter().map(|(name, value)| (name.to_string(), value)).collect(),
            body: request.body,
        }
    }

    pub async fn send(&self, request: Request) -> Result<(u16, Vec<u8>), ClientError> {
        let reply = self.mock.handle(Self::call(request)).await;
        match reply.body {
            ReplyBody::Full(body) => Ok((reply.status, body)),
            ReplyBody::Events(_) => Err(ClientError::ProtocolError("unexpected event stream".into())),
        }
    }

    pub async fn open_stream(&self, request: Request) -> Result<(u16, Chunks), ClientError> {
        let reply = self.mock.handle(Self::call(request)).await;
        Ok((
            reply.status,
            match reply.body {
                ReplyBody::Full(body) => futures::stream::once(async move { Ok(body) }).boxed(),
                ReplyBody::Events(chunks) => ReceiverStream::new(chunks).map(Ok).boxed(),
            },
        ))
    }
}

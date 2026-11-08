//! HTTP/1.1 transport (reqwest).

use super::{Chunks, Request};
use crate::generated::ClientError;
use futures::StreamExt;

pub struct Http {
    base: String,
    client: reqwest::Client,
}

fn network(e: reqwest::Error) -> ClientError {
    // the innermost cause says what actually happened ("Connection refused")
    let mut cause: &dyn std::error::Error = &e;
    while let Some(inner) = cause.source() {
        cause = inner;
    }
    ClientError::NetworkError(format!("{e}: {cause}"))
}

impl Http {
    pub fn new(endpoint: &str) -> Result<Http, ClientError> {
        let client = reqwest::Client::builder().build().map_err(network)?;
        Ok(Http { base: endpoint.trim_end_matches('/').to_string(), client })
    }

    async fn start(&self, request: Request) -> Result<reqwest::Response, ClientError> {
        let method = reqwest::Method::from_bytes(request.method.as_bytes()).expect("valid HTTP method");
        let mut builder = self.client.request(method, format!("{}{}", self.base, request.path));
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        builder.body(request.body).send().await.map_err(network)
    }

    pub async fn send(&self, request: Request) -> Result<(u16, Vec<u8>), ClientError> {
        let response = self.start(request).await?;
        let status = response.status().as_u16();
        Ok((status, response.bytes().await.map_err(network)?.to_vec()))
    }

    pub async fn open_stream(&self, request: Request) -> Result<(u16, Chunks), ClientError> {
        let response = self.start(request).await?;
        let status = response.status().as_u16();
        Ok((status, response.bytes_stream().map(|chunk| chunk.map(|b| b.to_vec()).map_err(network)).boxed()))
    }
}

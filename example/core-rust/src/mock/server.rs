//! The mock service over HTTP (axum): every request goes to `Mock::handle`.

use super::{Call, Mock, ReplyBody};
use axum::body::{Body, Bytes};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use futures::StreamExt;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::ReceiverStream;

pub async fn serve(listener: tokio::net::TcpListener, mock: Arc<Mock>) -> std::io::Result<()> {
    let app = Router::new().fallback(handle).layer(DefaultBodyLimit::disable()).with_state(mock);
    axum::serve(listener, app).await
}

async fn handle(State(mock): State<Arc<Mock>>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let headers = headers.iter().filter_map(|(name, value)| Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))).collect();
    let reply = mock.handle(Call { method: method.as_str().to_string(), path: uri.path_and_query().map(|p| p.as_str()).unwrap_or(uri.path()).to_string(), headers, body: body.to_vec() }).await;
    let status = StatusCode::from_u16(reply.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    match reply.body {
        ReplyBody::Full(body) => (status, [(header::CONTENT_TYPE, "application/octet-stream")], body).into_response(),
        ReplyBody::Events(chunks) => {
            let chunks = ReceiverStream::new(chunks).map(Ok::<_, Infallible>);
            (status, [(header::CONTENT_TYPE, "text/event-stream"), (header::CACHE_CONTROL, "no-cache")], Body::from_stream(chunks)).into_response()
        }
    }
}

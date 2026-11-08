//! demo-mock-server [addr] — the mock service over real HTTP.
//!   cargo run --features mock-server --bin demo-mock-server -- 127.0.0.1:9001
//! then use endpoint `http://127.0.0.1:9001` with api key `demo-key`.

use demo_client::mock::{server, Mock, DEMO_API_KEY};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:9001".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("demo-mock-server listening on http://{}", listener.local_addr()?);
    server::serve(listener, Mock::new(DEMO_API_KEY)).await
}

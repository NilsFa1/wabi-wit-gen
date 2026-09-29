//! acme-mock-server [addr] — serves the mock backend over real WebSockets.
//!   cargo run --bin acme-mock-server -- 127.0.0.1:9001
//! then use endpoint `ws://127.0.0.1:9001` with api key `demo-key`.

use acme_client::mock::{serve, MockConfig};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:9001".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("acme-mock-server listening on ws://{}", listener.local_addr()?);
    loop {
        let (tcp, peer) = listener.accept().await?;
        tokio::spawn(async move {
            match acme_client::websocket::accept(tcp).await {
                Ok(t) => serve(t, MockConfig::default()).await,
                Err(e) => eprintln!("{peer}: {e:?}"),
            }
        });
    }
}

//! Tests of the idiomatic Rust API — no FFI involved.

use wabi_client::*;
use futures::StreamExt;
use std::time::Duration;

fn opts(endpoint: &str) -> ClientOptions {
    ClientOptions { endpoint: endpoint.into(), api_key: Some("demo-key".into()), request_timeout_ms: 5_000 }
}

#[tokio::test]
async fn request_response_and_errors() {
    let c = Client::new(opts("mem://t"));
    assert_eq!(c.get_customer("4711".into()).await, Err(ClientError::Disconnected));
    c.connect().await.unwrap();
    assert_eq!(c.state(), ConnectionState::Connected);
    let cust = c.get_customer("4711".into()).await.unwrap();
    assert_eq!(cust.name, "Erika Mustermann");
    assert_eq!(c.get_customer("nope".into()).await, Err(ClientError::NotFound));
    assert!(matches!(
        c.send_command(Command { name: "fail".into(), payload: vec![] }).await,
        Err(ClientError::ProtocolError(_))
    ));
    c.disconnect().await.unwrap();
    assert_eq!(c.state(), ConnectionState::Disconnected);
}

#[tokio::test]
async fn unauthorized_and_unreachable() {
    let mut o = opts("mem://t");
    o.api_key = Some("wrong".into());
    assert_eq!(Client::new(o).connect().await, Err(ClientError::Unauthorized));
    assert!(matches!(Client::new(opts("mem://unreachable")).connect().await, Err(ClientError::NetworkError(_))));
}

#[tokio::test]
async fn requests_are_multiplexed() {
    let c = Client::new(opts("mem://t"));
    c.connect().await.unwrap();
    // A slow request must not block fast ones on the same socket.
    let slow = c.send_command(Command { name: "sleep".into(), payload: b"300".to_vec() });
    let fast = async {
        let t = std::time::Instant::now();
        for _ in 0..20 {
            c.get_customer("4711".into()).await.unwrap();
        }
        t.elapsed()
    };
    let (s, elapsed) = tokio::join!(slow, fast);
    s.unwrap();
    assert!(elapsed < Duration::from_millis(250), "fast requests waited for the slow one: {elapsed:?}");
}

#[tokio::test]
async fn events_and_connection_loss() {
    let c = Client::new(opts("mem://t"));
    let mut ev = c.events();
    c.connect().await.unwrap();
    assert_eq!(ev.next().await, Some(ClientEvent::ConnectionStateChanged(ConnectionState::Connecting)));
    assert_eq!(ev.next().await, Some(ClientEvent::ConnectionStateChanged(ConnectionState::Connected)));
    c.create_customer("Max".into(), None).await.unwrap();
    assert!(matches!(ev.next().await, Some(ClientEvent::CustomerUpdated(Customer { name, .. })) if name == "Max"));

    // Server hangs up: the pending request fails, state becomes disconnected.
    let r = c.send_command(Command { name: "hang-up".into(), payload: vec![] }).await;
    assert_eq!(r, Err(ClientError::Disconnected));
    assert_eq!(ev.next().await, Some(ClientEvent::ConnectionStateChanged(ConnectionState::Disconnected)));
    // Reconnect works.
    c.connect().await.unwrap();
    c.close();
    // Stream ends after close.
    while ev.next().await.is_some() {}
}

#[tokio::test]
async fn real_websocket_end_to_end() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let t = wabi_client::websocket::accept(tcp).await.unwrap();
                wabi_client::mock::serve(t, Default::default()).await;
            });
        }
    });
    let c = Client::new(opts(&format!("ws://{addr}")));
    c.connect().await.unwrap();
    let big = vec![7u8; 512 * 1024];
    let r = c.send_command(Command { name: "echo".into(), payload: big.clone() }).await.unwrap();
    assert_eq!(r.payload, big);
    assert_eq!(c.get_customer("4711".into()).await.unwrap().id, "4711");
    c.disconnect().await.unwrap();
}

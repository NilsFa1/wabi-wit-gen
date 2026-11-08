//! Tests of the idiomatic Rust API — no FFI involved.

use demo_client::*;
use futures::StreamExt;
use std::time::Duration;

fn opts(endpoint: &str) -> ClientOptions {
    ClientOptions { endpoint: endpoint.into(), api_key: Some("demo-key".into()), request_timeout_ms: 5_000, key_id: None }
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
    assert!(matches!(Client::new(opts("http://127.0.0.1:1")).connect().await, Err(ClientError::NetworkError(_))));
    assert!(matches!(Client::new(opts("ftp://x")).connect().await, Err(ClientError::NetworkError(_))));
    // a session without a key cannot ask who signed
    let c = Client::new(opts("mem://t"));
    c.connect().await.unwrap();
    assert_eq!(c.key_fingerprint().await, Err(ClientError::Unauthorized));
}

#[tokio::test]
async fn requests_run_concurrently() {
    let c = Client::new(opts("mem://t"));
    c.connect().await.unwrap();
    // A slow request must not block fast ones.
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
async fn events_and_session_loss() {
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
async fn clients_share_a_transport() {
    let transport = SharedTransport::new("mem://t".into());
    let a = transport.open_client(opts("ignored://"));
    let b = Client::new(opts("ftp://unsupported"));
    b.use_transport(std::sync::Arc::new(SharedTransport { link: transport.link.clone() }));
    a.connect().await.unwrap();
    b.connect().await.unwrap();
    let before = transport.request_count();
    a.get_customer("4711".into()).await.unwrap();
    b.get_customer("4711".into()).await.unwrap();
    assert_eq!(transport.request_count(), before + 2);

    let other = std::sync::Arc::new(SharedTransport::new("mem://t".into()));
    other.open_client(opts("x")).connect().await.unwrap();
    let (mine, theirs) = (transport.request_count(), other.request_count());
    assert!(theirs >= 2, "session + event channel");
    transport.absorb(other);
    assert_eq!(transport.request_count(), mine + theirs);
}

/// The same client over real HTTP against the mock server (`--features mock-server`).
#[cfg(feature = "mock-server")]
#[tokio::test]
async fn real_http_end_to_end() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(demo_client::mock::server::serve(listener, demo_client::mock::Mock::new("demo-key")));

    let c = Client::new(opts(&format!("http://{addr}")));
    let mut ev = c.events();
    c.connect().await.unwrap();
    let big = vec![7u8; 512 * 1024];
    let r = c.send_command(Command { name: "echo".into(), payload: big.clone() }).await.unwrap();
    assert_eq!(r.payload, big);
    assert_eq!(c.get_customer("4711".into()).await.unwrap().id, "4711");
    assert_eq!(c.get_customer("nö/../x&y=1".into()).await, Err(ClientError::NotFound));
    assert_eq!(c.get_customer("..".into()).await, Err(ClientError::NotFound));
    let made = c.create_customer("Jürgen 🚀".into(), None).await.unwrap();
    assert_eq!(c.get_customer(made.id.clone()).await.unwrap(), made);
    loop {
        match ev.next().await {
            Some(ClientEvent::CustomerUpdated(customer)) => break assert_eq!(customer, made),
            Some(_) => continue,
            None => panic!("event stream ended"),
        }
    }
    let r = c.send_command(Command { name: "hang-up".into(), payload: vec![] }).await;
    assert_eq!(r, Err(ClientError::Disconnected));
    c.connect().await.unwrap();
    c.disconnect().await.unwrap();

    let mut wrong = opts(&format!("http://{addr}"));
    wrong.api_key = Some("wrong".into());
    assert_eq!(Client::new(wrong).connect().await, Err(ClientError::Unauthorized));
}

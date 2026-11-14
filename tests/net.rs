//! Room codes and a real peer-to-peer connection (needs internet for the relay).

use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::timeout;
use watchsync::net::{self, Msg, NetEvent};

#[test]
fn room_code_rejects_garbage() {
    assert!(net::decode_code("hello").is_err());
    assert!(net::decode_code("").is_err());
    assert!(net::decode_code("abcd").is_err()); // valid hex, not an address
}

async fn next(rx: &mut mpsc::UnboundedReceiver<NetEvent>) -> NetEvent {
    timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("timed out waiting for a network event")
        .expect("channel closed")
}

#[tokio::test(flavor = "multi_thread")]
async fn host_and_guest_exchange_messages() {
    let (host_tx, mut host_rx) = mpsc::unbounded_channel();
    let (host_ep, code) = timeout(Duration::from_secs(30), net::host(host_tx))
        .await
        .expect("host timed out")
        .expect("host");

    // The code survives being pasted with stray whitespace/newlines.
    let pasted = format!("  {}\n", code);
    let decoded = net::decode_code(&pasted).unwrap();
    assert_eq!(decoded.id, host_ep.id());

    let (guest_tx, mut guest_rx) = mpsc::unbounded_channel();
    let (guest_ep, to_host) = timeout(
        Duration::from_secs(30),
        net::join(&pasted, "Riya".into(), guest_tx),
    )
    .await
    .expect("join timed out")
    .expect("join");

    // PeerJoined must arrive before the guest's Hello, or the host drops it.
    let to_guest = match next(&mut host_rx).await {
        NetEvent::PeerJoined { tx, .. } => tx,
        other => panic!("expected PeerJoined first, got {other:?}"),
    };
    match next(&mut host_rx).await {
        NetEvent::FromPeer { msg: Msg::Hello { name }, .. } => assert_eq!(name, "Riya"),
        other => panic!("expected Hello, got {other:?}"),
    }

    // guest → host → guest round trip
    to_host.send(Msg::Ping { t0: 1.0 }).unwrap();
    match next(&mut host_rx).await {
        NetEvent::FromPeer { msg: Msg::Ping { t0 }, .. } => assert_eq!(t0, 1.0),
        other => panic!("expected Ping, got {other:?}"),
    }
    to_guest.send(Msg::Pong { t0: 1.0, host_ms: 2.0 }).unwrap();
    match next(&mut guest_rx).await {
        NetEvent::FromHost(Msg::Pong { t0, host_ms }) => assert_eq!((t0, host_ms), (1.0, 2.0)),
        other => panic!("expected Pong, got {other:?}"),
    }

    // The guest leaving is noticed by the host.
    guest_ep.close().await;
    match next(&mut host_rx).await {
        NetEvent::PeerLeft { .. } => {}
        other => panic!("expected PeerLeft, got {other:?}"),
    }
    host_ep.close().await;
}

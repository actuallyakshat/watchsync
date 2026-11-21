//! Sending over the real internet, Cubic (QUIC's default) against BBR (what
//! WatchSync uses). Both Macs here are this one, forced through iroh's relay
//! so the bytes really cross this Mac's uplink and router queue. iroh's
//! public relay caps the speed, so compare the round-trip times: on a
//! bloated home router Cubic's climbed to ~2 s, BBR's stayed ~0.2 s (and a
//! direct send's speed is about window ÷ round trip).
//!
//!     cargo test --release --test speed -- --ignored --nocapture

use iroh::Endpoint;
use iroh::endpoint::{QuicTransportConfig, presets};
use std::sync::Arc;
use std::time::{Duration, Instant};

const ALPN: &[u8] = b"watchsync/speed-test/0";
const BYTES: usize = 40 * 1024 * 1024;

async fn endpoint(config: QuicTransportConfig) -> Endpoint {
    let ep = Endpoint::builder(presets::N0)
        .clear_ip_transports()
        .transport_config(config)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    ep.online().await;
    ep
}

/// MB/s for one send of `BYTES` from a sender using `config`.
async fn send_speed(config: QuicTransportConfig) -> f64 {
    let (tx, rx) = (endpoint(config).await, endpoint(watchsync::net::transport_config()).await);
    let addr = rx.addr();
    let server = tokio::spawn(async move {
        let conn = rx.accept().await.unwrap().await.unwrap();
        let mut recv = conn.accept_uni().await.unwrap();
        let mut got = 0;
        while let Some(chunk) = recv.read_chunk(1 << 20).await.unwrap() {
            got += chunk.len();
        }
        assert_eq!(got, BYTES);
        conn.close(0u32.into(), b"done");
        rx
    });
    let conn = tx.connect(addr, ALPN).await.unwrap();
    let mut send = conn.open_uni().await.unwrap();
    let buf = vec![7u8; 1 << 20];
    let start = Instant::now();
    for _ in 0..BYTES / buf.len() {
        send.write_all(&buf).await.unwrap();
    }
    send.finish().unwrap();
    let rx = tokio::time::timeout(Duration::from_secs(300), server).await.unwrap().unwrap();
    let secs = start.elapsed().as_secs_f64();
    let via = conn.paths().iter().find(|p| p.is_selected()).map(|p| if p.is_relay() { "relay" } else { "direct" });
    println!("  {:.1} MB/s ({via:?}, rtt {:?})", BYTES as f64 / 1e6 / secs, conn.paths().iter().next().map(|p| p.rtt()));
    tx.close().await;
    rx.close().await;
    BYTES as f64 / 1e6 / secs
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "measures this Mac's internet; slow"]
async fn bbr_keeps_the_round_trip_short_through_a_bloated_router() {
    let cubic = || {
        QuicTransportConfig::builder()
            .congestion_controller_factory(Arc::new(noq_proto::congestion::CubicConfig::default()))
            .build()
    };
    let (mut c, mut b) = (Vec::new(), Vec::new());
    for _ in 0..2 {
        println!("cubic:");
        c.push(send_speed(cubic()).await);
        println!("bbr:");
        b.push(send_speed(watchsync::net::transport_config()).await);
    }
    let avg = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    println!("cubic {:.1} MB/s, bbr {:.1} MB/s", avg(&c), avg(&b));
}

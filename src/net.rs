//! Peer-to-peer transport (iroh/QUIC). Messages are JSON lines on one
//! bidirectional stream per guest. Topology is a star around the host.

use crate::sync::{Crop, PlaybackState};
use anyhow::{Result, anyhow};
use iroh::{Endpoint, EndpointAddr, endpoint::presets};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

pub const ALPN: &[u8] = b"watchsync/0";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Intent {
    Play,
    Pause { position: f64 },
    Seek { position: f64 },
    SetCrop { crop: Crop },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PeerInfo {
    pub name: String,
    pub is_host: bool,
    pub fingerprint: Option<String>,
    pub drift_ms: Option<f64>,
    /// App version ("" = too old to say).
    #[serde(default)]
    pub version: String,
}

/// This build's version, exchanged so mismatched apps can be flagged.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

// New fields are `#[serde(default)]` and new variants are ignored by older
// apps (they drop lines they can't parse), so 0.1.x peers still interoperate.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Msg {
    /// guest → host, first message.
    Hello {
        name: String,
        #[serde(default)]
        version: String,
    },
    /// guest → host clock probe; host answers with Pong.
    Ping { t0: f64 },
    Pong { t0: f64, host_ms: f64 },
    /// guest → host: "I want to play/pause/seek".
    Intent(Intent),
    /// host → all: the new authoritative state.
    State(PlaybackState),
    /// guest → host, every second.
    Status {
        fingerprint: Option<String>,
        drift_ms: Option<f64>,
    },
    /// host → all: who is in the room.
    Peers(Vec<PeerInfo>),
    /// host → all: the room's picture crop and who chose it.
    Crop { crop: Crop, by: String },
}

#[derive(Debug)]
pub enum NetEvent {
    PeerJoined {
        id: u64,
        tx: mpsc::UnboundedSender<Msg>,
    },
    FromPeer {
        id: u64,
        msg: Msg,
    },
    PeerLeft {
        id: u64,
    },
    FromHost(Msg),
    HostLost,
}

fn err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{e}")
}

/// Room code = hex(JSON(EndpointAddr)). Carries the relay URL and direct
/// addresses, so joining doesn't depend on DNS lookup having propagated.
pub fn encode_code(addr: &EndpointAddr) -> String {
    hex::encode(serde_json::to_vec(addr).unwrap_or_default())
}

pub fn decode_code(code: &str) -> Result<EndpointAddr> {
    let cleaned: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = hex::decode(cleaned).map_err(|_| anyhow!("That doesn't look like a room code"))?;
    serde_json::from_slice(&bytes).map_err(|_| anyhow!("That doesn't look like a room code"))
}

/// Spawn a task that writes queued messages to the stream as JSON lines.
fn spawn_writer(mut send: iroh::endpoint::SendStream) -> mpsc::UnboundedSender<Msg> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let Ok(mut line) = serde_json::to_vec(&msg) else {
                continue;
            };
            line.push(b'\n');
            if send.write_all(&line).await.is_err() {
                break;
            }
        }
    });
    tx
}

/// Spawn a task that reads JSON lines from the stream.
fn spawn_reader(
    recv: iroh::endpoint::RecvStream,
    on_msg: impl Fn(Msg) + Send + 'static,
    on_close: impl FnOnce() + Send + 'static,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(recv).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(msg) = serde_json::from_str::<Msg>(&line) {
                on_msg(msg);
            }
        }
        on_close();
    });
}

/// Start hosting. Returns the endpoint (keep it alive) and the room code.
pub async fn host(events: mpsc::UnboundedSender<NetEvent>) -> Result<(Endpoint, String)> {
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(err)?;
    endpoint.online().await;
    let code = encode_code(&endpoint.addr());

    let ep = endpoint.clone();
    tokio::spawn(async move {
        let mut next_id = 1u64;
        while let Some(incoming) = ep.accept().await {
            let id = next_id;
            next_id += 1;
            let events = events.clone();
            tokio::spawn(async move {
                let Ok(conn) = incoming.await else { return };
                let Ok((send, recv)) = conn.accept_bi().await else {
                    return;
                };
                // Register the peer *before* reading, so its Hello can't
                // arrive at the engine ahead of PeerJoined.
                let tx = spawn_writer(send);
                let _ = events.send(NetEvent::PeerJoined { id, tx });
                let ev_msg = events.clone();
                let ev_close = events.clone();
                spawn_reader(
                    recv,
                    move |msg| {
                        let _ = ev_msg.send(NetEvent::FromPeer { id, msg });
                    },
                    move || {
                        let _ = ev_close.send(NetEvent::PeerLeft { id });
                    },
                );
                // Keep the connection alive until it closes.
                conn.closed().await;
            });
        }
    });
    Ok((endpoint, code))
}

/// Join a room. Returns the endpoint (keep it alive) and a sender to the host.
pub async fn join(
    code: &str,
    name: String,
    events: mpsc::UnboundedSender<NetEvent>,
) -> Result<(Endpoint, mpsc::UnboundedSender<Msg>)> {
    let addr = decode_code(code)?;
    let endpoint = Endpoint::bind(presets::N0).await.map_err(err)?;
    let conn = endpoint.connect(addr, ALPN).await.map_err(err)?;
    let (send, recv) = conn.open_bi().await.map_err(err)?;
    let tx = spawn_writer(send);
    let ev_msg = events.clone();
    spawn_reader(
        recv,
        move |msg| {
            let _ = ev_msg.send(NetEvent::FromHost(msg));
        },
        move || {
            let _ = events.send(NetEvent::HostLost);
        },
    );
    // A QUIC stream only reaches the other side once data is written,
    // so say hello right away.
    let _ = tx.send(Msg::Hello {
        name,
        version: APP_VERSION.to_string(),
    });
    // Keep the connection alive for as long as the endpoint lives.
    tokio::spawn(async move {
        conn.closed().await;
    });
    Ok((endpoint, tx))
}

//! Peer-to-peer transport (iroh/QUIC). Messages are JSON lines on one
//! bidirectional stream per guest. Topology is a star around the host.

use crate::sync::{Crop, PlaybackState};
use anyhow::{Result, anyhow};
use iroh::{Endpoint, EndpointAddr, endpoint::presets};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc;

pub const ALPN: &[u8] = b"watchsync/0";
/// The call's fallback relay between two people's Macs (see `relay`).
pub const ALPN_RELAY: &[u8] = b"watchsync/relay/0";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Intent {
    Play,
    Pause { position: f64 },
    Seek { position: f64 },
    SetCrop { crop: Crop },
}

/// A movie file someone offers to send with croc.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Offer {
    pub offer_id: String,
    /// Endpoint id and name of who offers it (filled in by the host).
    pub from_id: String,
    pub from: String,
    pub name: String,
    pub size: u64,
    pub fingerprint: String,
}

/// Quick reactions. Shown as words on the movie (its text renderer has no
/// colour emoji) and as emoji in the app.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reaction {
    Laugh,
    Wow,
    Love,
    Fire,
    Clap,
    Sad,
}

impl Reaction {
    /// "Riya {phrase}", for the movie window.
    pub fn phrase(self) -> &'static str {
        match self {
            Reaction::Laugh => "is laughing",
            Reaction::Wow => "is shocked",
            Reaction::Love => "loves this",
            Reaction::Fire => "says this is fire",
            Reaction::Clap => "is clapping",
            Reaction::Sad => "is tearing up",
        }
    }
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
    /// Endpoint id, so a successor can recognise itself ("" = 0.2.x app).
    #[serde(default)]
    pub id: String,
    /// The peer's own address code, so others can reach it if it becomes host.
    #[serde(default)]
    pub code: String,
    /// The movie's file name (sent for the host, so friends can find theirs).
    #[serde(default)]
    pub file_name: Option<String>,
    /// Their microphone / camera is on in the call.
    #[serde(default)]
    pub mic: bool,
    #[serde(default)]
    pub cam: bool,
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
        /// Endpoint id and address code ("" from 0.2.x apps, which then
        /// can't take over as host).
        #[serde(default)]
        id: String,
        #[serde(default)]
        code: String,
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
    /// guest → host: "call me this now".
    Rename { name: String },
    /// host → all: "I'm leaving; this peer (endpoint id) takes over".
    HostLeaving { successor: String },
    /// A chat line. Guests send it to the host with `from` empty; the host
    /// fills in the sender's name and passes it on to everyone else.
    Chat { from: String, text: String },
    /// A quick reaction, relayed the same way.
    React { from: String, reaction: Reaction },
    /// guest → host: "I can send my copy" (host fills in who).
    Offer(Offer),
    /// guest → host: stopped offering.
    OfferEnd { offer_id: String },
    /// host → all: every copy on offer in the room.
    Offers(Vec<Offer>),
    /// "Please send me offer `offer_id`", for the peer with endpoint id `to`.
    /// The host fills in who's asking (`from_id`, `from`).
    FileRequest {
        offer_id: String,
        to: String,
        from_id: String,
        from: String,
    },
    /// The croc code for one download, for the requester (`to`) only.
    FileCode {
        offer_id: String,
        to: String,
        code: String,
    },
    /// The sender can't serve this request right now.
    FileRefused {
        offer_id: String,
        to: String,
        reason: String,
    },
    /// guest → host: "my mic / camera is now on or off".
    Call { mic: bool, cam: bool },
    /// A WebRTC signalling message (offer, answer, ICE candidate) for the
    /// peer with endpoint id `to`. The host fills in who sent it (`from_id`)
    /// and passes it on; the engine never looks inside `data`.
    Signal {
        to: String,
        #[serde(default)]
        from_id: String,
        data: serde_json::Value,
    },
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
    /// A message on host connection number `conn`.
    FromHost { conn: u64, msg: Msg },
    /// Host connection number `conn` closed.
    HostLost { conn: u64 },
    /// Result of a background (re)connection attempt.
    Connected {
        attempt: u64,
        result: Result<HostLink, String>,
    },
}

/// A live connection to the host.
#[derive(Debug, Clone)]
pub struct HostLink {
    pub conn: u64,
    pub tx: mpsc::UnboundedSender<Msg>,
    pub connection: iroh::endpoint::Connection,
    /// The code we dialled.
    pub code: String,
}

fn err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{e}")
}

/// Room code = hex(JSON(EndpointAddr)). Carries the relay URL and direct
/// addresses, so joining doesn't depend on DNS lookup having propagated.
pub fn encode_code(addr: &EndpointAddr) -> String {
    hex::encode(serde_json::to_vec(addr).unwrap_or_default())
}

/// The web page that turns an invite link into `watchsync://join/<code>`.
/// The code travels after `#`, so it never reaches a server.
pub const INVITE_PAGE: &str = "https://actuallyakshat.github.io/watchsync/join/";

/// The link friends tap to join.
pub fn invite_link(code: &str) -> String {
    format!("{INVITE_PAGE}#{code}")
}

/// Accept any way a friend might pass the invite on: the web link, the
/// `watchsync://` link, or the bare code (with stray spaces or newlines).
pub fn extract_code(input: &str) -> String {
    let s = input.trim();
    let s = match s.rfind('#') {
        Some(i) => &s[i + 1..],
        None => s
            .strip_prefix("watchsync://join/")
            .or_else(|| s.strip_prefix("watchsync://join?code="))
            .or_else(|| s.strip_prefix("watchsync://"))
            .unwrap_or(s),
    };
    s.chars().filter(|c| !c.is_whitespace() && *c != '/').collect()
}

pub fn decode_code(code: &str) -> Result<EndpointAddr> {
    let cleaned = extract_code(code);
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

/// Bind an endpoint that can both join rooms and host one, and wait until
/// it's reachable (so its address code includes the relay).
pub async fn bind() -> Result<Endpoint> {
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec(), ALPN_RELAY.to_vec()])
        .bind()
        .await
        .map_err(err)?;
    endpoint.online().await;
    Ok(endpoint)
}

/// The address code others use to reach this endpoint.
pub fn code_of(endpoint: &Endpoint) -> String {
    encode_code(&endpoint.addr())
}

pub fn id_of(endpoint: &Endpoint) -> String {
    endpoint.id().to_string()
}

/// Accept guests on `endpoint` right away (a host that never hands over).
pub fn serve(endpoint: Endpoint, events: mpsc::UnboundedSender<NetEvent>) {
    let (relay, _) = mpsc::unbounded_channel();
    listen(endpoint, events, Arc::new(AtomicBool::new(true)), relay);
}

/// Handle every incoming connection on `endpoint` (call once, right after
/// binding): guests joining, while `serving` (this Mac is the host), and
/// friends' call relays at any time (handed to `relay`).
pub fn listen(
    endpoint: Endpoint,
    events: mpsc::UnboundedSender<NetEvent>,
    serving: Arc<AtomicBool>,
    relay: mpsc::UnboundedSender<iroh::endpoint::Connection>,
) {
    tokio::spawn(async move {
        let mut next_id = 1u64;
        while let Some(incoming) = endpoint.accept().await {
            let id = next_id;
            next_id += 1;
            let events = events.clone();
            let serving = serving.clone();
            let relay = relay.clone();
            tokio::spawn(async move {
                let Ok(conn) = incoming.await else { return };
                if conn.alpn() == ALPN_RELAY {
                    let _ = relay.send(conn);
                    return;
                }
                if conn.alpn() != ALPN || !serving.load(Ordering::SeqCst) {
                    // Not the host (yet): the guest tries again shortly.
                    conn.close(0u32.into(), b"not hosting");
                    return;
                }
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
}

/// Connect `endpoint` to the host at `code` and send `hello` first.
/// Messages arrive as `FromHost { conn }`, the end as `HostLost { conn }`.
pub async fn connect(
    endpoint: &Endpoint,
    code: &str,
    conn: u64,
    hello: Msg,
    events: mpsc::UnboundedSender<NetEvent>,
) -> Result<HostLink> {
    let addr = decode_code(code)?;
    let connection = endpoint.connect(addr, ALPN).await.map_err(err)?;
    let (send, recv) = connection.open_bi().await.map_err(err)?;
    let tx = spawn_writer(send);
    let ev_msg = events.clone();
    spawn_reader(
        recv,
        move |msg| {
            let _ = ev_msg.send(NetEvent::FromHost { conn, msg });
        },
        move || {
            let _ = events.send(NetEvent::HostLost { conn });
        },
    );
    // A QUIC stream only reaches the other side once data is written,
    // so say hello right away.
    let _ = tx.send(hello);
    Ok(HostLink {
        conn,
        tx,
        connection,
        code: code.trim().to_string(),
    })
}

/// The Hello a guest sends.
pub fn hello(name: &str, endpoint: &Endpoint) -> Msg {
    Msg::Hello {
        name: name.to_string(),
        version: APP_VERSION.to_string(),
        id: id_of(endpoint),
        code: code_of(endpoint),
    }
}

/// Start hosting on a fresh endpoint. Returns it (keep it alive) and the code.
pub async fn host(events: mpsc::UnboundedSender<NetEvent>) -> Result<(Endpoint, String)> {
    let endpoint = bind().await?;
    serve(endpoint.clone(), events);
    let code = code_of(&endpoint);
    Ok((endpoint, code))
}

/// Join a room from a fresh endpoint. Returns it (keep it alive) and the link.
pub async fn join(
    code: &str,
    name: String,
    events: mpsc::UnboundedSender<NetEvent>,
) -> Result<(Endpoint, HostLink)> {
    let endpoint = bind().await?;
    let link = connect(&endpoint, code, 0, hello(&name, &endpoint), events).await?;
    Ok((endpoint, link))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_links_and_codes_all_work() {
        let code = "7b2269";
        for input in [
            "7b2269",
            "  7b2269\n",
            "watchsync://join/7b2269",
            "watchsync://join/7b2269/",
            "watchsync://join?code=7b2269",
            "https://actuallyakshat.github.io/watchsync/join/#7b2269",
            "Join me! https://actuallyakshat.github.io/watchsync/join/#7b2269",
            &invite_link(code),
        ] {
            assert_eq!(extract_code(input), code, "{input:?}");
        }
    }

    #[test]
    fn an_invite_link_decodes_to_the_host() {
        let secret = iroh::SecretKey::from_bytes(&[7u8; 32]);
        let addr = EndpointAddr::from(secret.public());
        let link = invite_link(&encode_code(&addr));
        assert_eq!(decode_code(&link).unwrap().id, addr.id);
        assert!(decode_code("https://example.com/#nothex").is_err());
    }
}

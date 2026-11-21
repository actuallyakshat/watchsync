//! Peer-to-peer transport (iroh/QUIC). Messages are JSON lines on one
//! bidirectional stream per guest. Topology is a star around the host.

use crate::sync::{Crop, PlaybackState};
use anyhow::{Result, anyhow};
use iroh::{Endpoint, EndpointAddr, endpoint::presets};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, BufReader};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub const ALPN: &[u8] = b"watchsync/0";
/// The call's fallback relay between two people's Macs (see `relay`).
pub const ALPN_RELAY: &[u8] = b"watchsync/relay/0";
/// A movie file sent straight from one Mac to another (see `share`).
pub const ALPN_FILE: &[u8] = b"watchsync/file/0";

/// How an offered copy is sent: over the room's own direct connection, or
/// with croc (through croc's public relay).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    #[default]
    Direct,
    Croc,
}

impl Method {
    /// Offers from apps before direct sending existed were all croc.
    fn older_apps() -> Self {
        Method::Croc
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Intent {
    Play,
    Pause { position: f64 },
    Seek { position: f64 },
    SetCrop { crop: Crop },
    /// Subtitle size (mpv's sub-scale, 1 = normal).
    SetSubScale { scale: f64 },
}

/// A movie file someone offers to send.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Offer {
    pub offer_id: String,
    /// Endpoint id and name of who offers it (filled in by the host).
    pub from_id: String,
    pub from: String,
    pub name: String,
    pub size: u64,
    pub fingerprint: String,
    #[serde(default = "Method::older_apps")]
    pub method: Method,
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
    /// Sharing their screen (or a window).
    #[serde(default)]
    pub screen: bool,
    /// The room's short invite code (sent for the host), so whoever takes
    /// over keeps the same invite working ("" = none, or an older app).
    #[serde(default)]
    pub invite: String,
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
    /// host → all: the room's subtitle size and who chose it.
    SubScale { scale: f64, by: String },
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
        /// The asker's app can receive directly (older apps can't).
        #[serde(default)]
        direct: bool,
    },
    /// Go ahead, for the requester (`to`) only: the croc code, or
    /// `DIRECT_CODE` for a direct send (the sender only lets the requester's
    /// endpoint in, so there's no secret to pass on).
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
    /// guest → host: "my mic / camera / screen share is now on or off".
    Call {
        mic: bool,
        cam: bool,
        #[serde(default)]
        screen: bool,
    },
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
        alive: Liveness,
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
    pub alive: Liveness,
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

// ------------------------------------------------------------ invite codes
//
// The address code above is a few hundred characters, too long to share
// comfortably. Invites use a short code instead ("abc-defg-hij"), looked up
// on n0's public pkarr server (the one iroh already uses to find endpoints).
// The code is hashed into a signing key; the host signs a small record under
// it naming its endpoint id and relay. Anyone with the code can read the
// record, and could also sign a newer one to send later joiners elsewhere
// (the long link couldn't be redirected). Fine among friends; 47 random bits
// keep strangers from guessing a code.

/// Letters in an invite code (3-4-3, like "abc-defg-hij"): about 47 bits.
const INVITE_LEN: usize = 10;
/// How often a host republishes its invite record, and retries a failed one.
const INVITE_REPUBLISH: std::time::Duration = std::time::Duration::from_secs(4 * 60);
const INVITE_RETRY: std::time::Duration = std::time::Duration::from_secs(10);

fn format_invite(letters: &str) -> String {
    format!("{}-{}-{}", &letters[..3], &letters[3..7], &letters[7..])
}

/// A fresh random invite code.
pub fn new_invite() -> String {
    let mut letters = String::new();
    while letters.len() < INVITE_LEN {
        for b in iroh::SecretKey::generate().to_bytes() {
            // 234 = 9 × 26: skip the rest so every letter is equally likely.
            if b < 234 && letters.len() < INVITE_LEN {
                letters.push((b'a' + b % 26) as char);
            }
        }
    }
    format_invite(&letters)
}

/// The invite code in `input` (a link or the bare code, any case, hyphens
/// optional), as "abc-defg-hij". `None` if it's an address code instead.
pub fn parse_invite(input: &str) -> Option<String> {
    let letters: String = extract_code(input)
        .chars()
        .filter(|c| *c != '-')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (letters.len() == INVITE_LEN && letters.bytes().all(|b| b.is_ascii_lowercase()))
        .then(|| format_invite(&letters))
}

/// The key an invite code's record is signed with.
fn invite_key(invite: &str) -> iroh::SecretKey {
    use sha2::Digest as _;
    let h = sha2::Sha256::digest(format!("watchsync/invite/v1/{invite}").as_bytes());
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&h);
    iroh::SecretKey::from_bytes(&bytes)
}

fn pkarr_client(endpoint: &Endpoint) -> Result<iroh::address_lookup::PkarrRelayClient> {
    Ok(iroh::address_lookup::PkarrRelayClient::new(
        iroh::address_lookup::N0_DNS_PKARR_RELAY_PROD.parse().map_err(err)?,
        endpoint.tls_config().clone(),
        endpoint.dns_resolver().map_err(err)?.clone(),
    ))
}

/// Sign and upload the record pointing `invite` at `host`.
async fn publish_invite_once(
    client: &iroh::address_lookup::PkarrRelayClient,
    invite: &str,
    host: &EndpointAddr,
) -> Result<()> {
    use iroh::address_lookup::{EndpointData, EndpointInfo, UserData};
    // The relay is enough to reach the host (iroh finds direct paths from
    // there), and keeps the record well inside one DNS packet.
    let relays = host.addrs.iter().filter(|a| a.is_relay()).cloned().collect();
    let host_id = UserData::try_from(host.id.to_z32()).map_err(err)?;
    let data = EndpointData::new(relays).with_user_data(host_id);
    let key = invite_key(invite);
    let packet = EndpointInfo::from_parts(key.public(), data)
        .to_pkarr_signed_packet(&key, 30)
        .map_err(err)?;
    client.publish(&packet).await.map_err(err)
}

/// Keeps an invite code pointing at this Mac until dropped.
pub struct InvitePublisher(tokio::task::JoinHandle<()>);

impl Drop for InvitePublisher {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Start publishing `invite` for `endpoint`, republishing every few minutes
/// (friends may join hours in). The receiver fires on the first success.
pub fn publish_invite(
    endpoint: &Endpoint,
    invite: &str,
) -> Result<(InvitePublisher, tokio::sync::oneshot::Receiver<()>)> {
    let client = pkarr_client(endpoint)?;
    let (endpoint, invite) = (endpoint.clone(), invite.to_string());
    let (done, first) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let mut done = Some(done);
        loop {
            // Someone else took the code over (a host back from sleep, after
            // its successor republished): stop, rather than flip it back.
            if done.is_none()
                && let Ok(host) = lookup_invite(&client, &invite).await
                && host.id != endpoint.id()
            {
                break;
            }
            let ok = publish_invite_once(&client, &invite, &endpoint.addr()).await.is_ok();
            if ok && let Some(done) = done.take() {
                let _ = done.send(());
            }
            tokio::time::sleep(if ok { INVITE_REPUBLISH } else { INVITE_RETRY }).await;
        }
    });
    Ok((InvitePublisher(task), first))
}

/// The host an invite code currently points at.
async fn lookup_invite(
    client: &iroh::address_lookup::PkarrRelayClient,
    invite: &str,
) -> Result<EndpointAddr> {
    use iroh::address_lookup::EndpointInfo;
    let not_found = || anyhow!("no room has the code {invite}. Check it, or ask for a new invite");
    let packet = client
        .resolve(invite_key(invite).public())
        .await
        .map_err(|_| not_found())?;
    let info = EndpointInfo::from_pkarr_signed_packet(&packet).map_err(|_| not_found())?;
    let id = info
        .user_data()
        .and_then(|d| iroh::EndpointId::from_z32(d.as_ref()).ok())
        .ok_or_else(not_found)?;
    Ok(EndpointAddr::from_parts(id, info.addrs().cloned()))
}

/// Look up the host behind an invite code; returns its address code.
pub async fn resolve_invite(endpoint: &Endpoint, invite: &str) -> Result<String> {
    Ok(encode_code(&lookup_invite(&pkarr_client(endpoint)?, invite).await?))
}

/// A stream with nothing to say sends an empty line this often, so the
/// other side can tell a quiet connection from a dead one. (Older apps skip
/// lines that don't parse, so they ignore it.)
const KEEPALIVE: Duration = Duration::from_secs(1);

/// When a connection last heard from the other side: anything at all
/// (keepalives included), and a real message. Kept by the network tasks
/// themselves, not the engine: an engine busy for a few seconds (a slow
/// player, a sleepy Mac) must not take a healthy connection for a dead one
/// and drop it, which friends see as "left" then "joined".
#[derive(Clone, Debug)]
pub struct Liveness(Arc<(AtomicU64, AtomicU64)>);

fn clock_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

impl Liveness {
    fn new() -> Self {
        let now = clock_ms();
        Self(Arc::new((AtomicU64::new(now), AtomicU64::new(now))))
    }

    fn heard(&self, message: bool) {
        let now = clock_ms();
        self.0.0.store(now, Ordering::Relaxed);
        if message {
            self.0.1.store(now, Ordering::Relaxed);
        }
    }

    /// How long since the other side was last heard from at all.
    pub fn idle(&self) -> Duration {
        Duration::from_millis(clock_ms().saturating_sub(self.0.0.load(Ordering::Relaxed)))
    }

    /// How long since the other side last said something real. Its network
    /// still sends keepalives if its app is stuck; this tells them apart.
    pub fn quiet(&self) -> Duration {
        Duration::from_millis(clock_ms().saturating_sub(self.0.1.load(Ordering::Relaxed)))
    }
}

/// Spawn a task that writes queued messages to the stream as JSON lines,
/// and keepalives when there's nothing to send.
fn spawn_writer(mut send: iroh::endpoint::SendStream) -> mpsc::UnboundedSender<Msg> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();
    tokio::spawn(async move {
        loop {
            let line = match tokio::time::timeout(KEEPALIVE, rx.recv()).await {
                Ok(Some(msg)) => {
                    let Ok(mut line) = serde_json::to_vec(&msg) else {
                        continue;
                    };
                    line.push(b'\n');
                    line
                }
                Ok(None) => break, // the engine let go of this connection
                Err(_) => b"\n".to_vec(),
            };
            if send.write_all(&line).await.is_err() {
                break;
            }
        }
    });
    tx
}

/// Spawn a task that reads JSON lines from the stream, noting in `heard`
/// whenever anything arrives.
fn spawn_reader(
    recv: iroh::endpoint::RecvStream,
    heard: Liveness,
    on_msg: impl Fn(Msg) + Send + 'static,
    on_close: impl FnOnce() + Send + 'static,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(recv).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            heard.heard(!line.is_empty());
            if line.is_empty() {
                continue; // keepalive
            }
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
        .alpns(vec![ALPN.to_vec(), ALPN_RELAY.to_vec(), ALPN_FILE.to_vec()])
        .bind()
        .await
        .map_err(err)?;
    endpoint.online().await;
    Ok(endpoint)
}

/// The `FileCode` that means "connect to me directly".
pub const DIRECT_CODE: &str = "direct";

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
/// binding): guests joining, while `serving` (this Mac is the host), and at
/// any time friends' call relays and movie downloads (handed to `relay`;
/// tell them apart by `alpn()`).
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
                if conn.alpn() == ALPN_RELAY || conn.alpn() == ALPN_FILE {
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
                let alive = Liveness::new();
                let _ = events.send(NetEvent::PeerJoined { id, tx, alive: alive.clone() });
                let ev_msg = events.clone();
                let ev_close = events.clone();
                spawn_reader(
                    recv,
                    alive,
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
    let alive = Liveness::new();
    let ev_msg = events.clone();
    spawn_reader(
        recv,
        alive.clone(),
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
        alive,
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
    fn invite_codes_are_three_four_three_letters() {
        let a = new_invite();
        assert_eq!(a.len(), 12, "{a}");
        assert_eq!(a.split('-').map(str::len).collect::<Vec<_>>(), [3, 4, 3], "{a}");
        assert!(a.chars().all(|c| c == '-' || c.is_ascii_lowercase()), "{a}");
        assert_eq!(parse_invite(&a).as_deref(), Some(a.as_str()));
        assert_ne!(a, new_invite());
    }

    #[test]
    fn invite_codes_are_found_however_they_are_passed_on() {
        let code = "abc-defg-hij";
        for input in [
            "abc-defg-hij",
            "  ABC-DEFG-HIJ\n",
            "abcdefghij",
            "abc defg hij",
            "watchsync://join/abc-defg-hij",
            "watchsync://join/abc-defg-hij/",
            &invite_link(code),
            "Join me! https://actuallyakshat.github.io/watchsync/join/#abc-defg-hij",
        ] {
            assert_eq!(parse_invite(input).as_deref(), Some(code), "{input:?}");
        }
        // Every spelling signs with the same key.
        let key = |s: &str| invite_key(&parse_invite(s).unwrap()).public();
        assert_eq!(key("ABCDEFGHIJ"), key("abc-defg-hij"));
        assert_ne!(key("abc-defg-hik"), key("abc-defg-hij"));
    }

    #[test]
    fn address_codes_are_not_invite_codes() {
        let secret = iroh::SecretKey::from_bytes(&[7u8; 32]);
        let code = encode_code(&EndpointAddr::from(secret.public()));
        assert_eq!(parse_invite(&code), None);
        assert_eq!(parse_invite(&invite_link(&code)), None);
        for input in ["abc-defg-hi", "abc-defg-hijk", "abc-def1-hij", "", "7b2269"] {
            assert_eq!(parse_invite(input), None, "{input:?}");
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

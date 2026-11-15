//! The call's fallback when two Macs can't reach each other directly: a tiny
//! TURN server on 127.0.0.1 that the call window's WebRTC uses, which carries
//! the packets to the friend's Mac over iroh (which gets through, via its own
//! relay servers if it must).
//!
//! Each Mac has a virtual address, derived from its endpoint id, in a range
//! that's never routed (the RFC 5737 documentation ranges). A relayed
//! candidate is that address plus a port per allocation, so when WebRTC asks
//! to send to one, the friend's Mac is known from the room list alone.
//! Packets travel as QUIC datagrams, split into several when too big (and
//! dropped if a part is lost, as UDP would): a reliable stream would stall
//! the media behind every lost packet.

use crate::net::{self, ALPN_RELAY};
use crate::share::random_hex;
use crate::stun::{self, Class, Message, attr, method};
use anyhow::Result;
use bytes::Bytes;
use iroh::endpoint::Connection;
use iroh::{Endpoint, EndpointId};
use serde::Serialize;
use sha2::Digest as _;
use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub const REALM: &str = "watchsync";
const LIFETIME: u32 = 600;
/// Someone missing from the room list this long loses their relay (the call
/// window waits as long before hanging up).
pub const GRACE: Duration = Duration::from_secs(45);
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);
/// Parts of a split packet that don't all arrive within this are dropped.
const REASSEMBLY: Duration = Duration::from_secs(2);
/// Frame header: id (2), part (1), parts (1), to port (2), from port (2).
const FRAME_HEADER: usize = 8;
const FIRST_PORT: u16 = 49152;

/// What the call window gives WebRTC as its TURN server.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct TurnInfo {
    pub urls: String,
    pub username: String,
    pub credential: String,
}

/// This Mac's (or a friend's) virtual address: 762 possible, so a collision
/// in a room of four is rare, and detected.
pub fn virtual_ip(id: &str) -> Ipv4Addr {
    let h = sha2::Sha256::digest(id.as_bytes());
    let n = u16::from_be_bytes([h[0], h[1]]) % 762;
    let host = (n % 254) as u8 + 1;
    match n / 254 {
        0 => Ipv4Addr::new(192, 0, 2, host),
        1 => Ipv4Addr::new(198, 51, 100, host),
        _ => Ipv4Addr::new(203, 0, 113, host),
    }
}

/// A TURN allocation: one per RTCPeerConnection that asks.
struct Alloc {
    port: u16,
    expires: Instant,
    channels: HashMap<u16, SocketAddrV4>,
    by_peer: HashMap<SocketAddrV4, u16>,
}

struct Peer {
    id: EndpointId,
    code: String,
    /// Missing from the room list since.
    gone: Option<Instant>,
}

#[derive(Default)]
struct State {
    allocs: HashMap<SocketAddrV4, Alloc>,
    ports: HashMap<u16, SocketAddrV4>,
    next_port: u16,
    peers: HashMap<Ipv4Addr, Peer>,
    /// Two people (or someone and us) share one address: no relay for them.
    ambiguous: HashSet<Ipv4Addr>,
    links: HashMap<EndpointId, Connection>,
    dialing: HashSet<EndpointId>,
    next_frame: u16,
    next_txid: u64,
}

struct Inner {
    socket: UdpSocket,
    me: Ipv4Addr,
    my_id: String,
    username: String,
    key: [u8; 16],
    nonce: String,
    endpoint: Endpoint,
    state: Mutex<State>,
}

/// The running relay. Dropping it stops everything.
pub struct Relay {
    pub info: TurnInfo,
    inner: Arc<Inner>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
        for c in self.inner.state.lock().unwrap().links.values() {
            c.close(0u32.into(), b"left");
        }
    }
}

impl Relay {
    /// Start on `endpoint`; friends' relay connections arrive on `incoming`.
    pub async fn start(endpoint: Endpoint, incoming: mpsc::UnboundedReceiver<Connection>) -> Result<Self> {
        let socket = UdpSocket::bind("127.0.0.1:0").await?;
        let port = socket.local_addr()?.port();
        let username = random_hex(8)?;
        let password = random_hex(16)?;
        let my_id = net::id_of(&endpoint);
        let inner = Arc::new(Inner {
            socket,
            me: virtual_ip(&my_id),
            my_id,
            key: stun::long_term_key(&username, REALM, &password),
            username: username.clone(),
            nonce: random_hex(8)?,
            endpoint,
            state: Mutex::new(State {
                next_port: FIRST_PORT,
                ..State::default()
            }),
        });
        let tasks = vec![
            tokio::spawn(clients(inner.clone())),
            tokio::spawn(accept(inner.clone(), incoming)),
            tokio::spawn(housekeeping(inner.clone())),
        ];
        Ok(Self {
            info: TurnInfo {
                urls: format!("turn:127.0.0.1:{port}?transport=udp"),
                username,
                credential: password,
            },
            inner,
            tasks,
        })
    }

    /// Who's in the room: (endpoint id, address code). People who drop off
    /// keep their relay for the grace period.
    pub fn set_peers<'a>(&self, peers: impl IntoIterator<Item = (&'a str, &'a str)>) {
        let mut st = self.inner.state.lock().unwrap();
        let mut seen: HashMap<Ipv4Addr, EndpointId> = HashMap::new();
        let mut ambiguous = HashSet::new();
        for (id, code) in peers {
            if id.is_empty() || code.is_empty() || id == self.inner.my_id {
                continue;
            }
            let Ok(endpoint_id) = id.parse::<EndpointId>() else { continue };
            let ip = virtual_ip(id);
            if ip == self.inner.me || seen.get(&ip).is_some_and(|other| *other != endpoint_id) {
                ambiguous.insert(ip);
            }
            seen.insert(ip, endpoint_id);
            match st.peers.get_mut(&ip) {
                Some(p) if p.id == endpoint_id => p.gone = None,
                _ => {
                    st.peers.insert(ip, Peer { id: endpoint_id, code: code.to_string(), gone: None });
                }
            }
        }
        let now = Instant::now();
        for (ip, p) in st.peers.iter_mut() {
            if !seen.contains_key(ip) && p.gone.is_none() {
                p.gone = Some(now);
            }
        }
        if ambiguous != st.ambiguous {
            for ip in &ambiguous {
                eprintln!("call relay: two people share virtual address {ip}; no relay between them");
            }
            st.ambiguous = ambiguous;
        }
    }
}

// ------------------------------------------------------------- the TURN side

async fn clients(inner: Arc<Inner>) {
    let mut buf = vec![0u8; 65536];
    while let Ok((n, from)) = inner.socket.recv_from(&mut buf).await {
        let Some(from) = stun::v4(from) else { continue };
        if let Some(reply) = inner.on_client(&buf[..n], from) {
            let _ = inner.socket.send_to(&reply, from).await;
        }
    }
}

impl Inner {
    /// One packet from WebRTC; maybe a reply.
    fn on_client(self: &Arc<Self>, buf: &[u8], from: SocketAddrV4) -> Option<Vec<u8>> {
        if stun::is_channel_data(buf) {
            let channel = u16::from_be_bytes([*buf.first()?, *buf.get(1)?]);
            let len = u16::from_be_bytes([*buf.get(2)?, *buf.get(3)?]) as usize;
            let data = buf.get(4..4 + len)?;
            let (port, peer) = {
                let st = self.state.lock().unwrap();
                let a = st.allocs.get(&from)?;
                (a.port, *a.channels.get(&channel)?)
            };
            self.forward(peer, port, data);
            return None;
        }
        let msg = Message::parse(buf)?;
        match (msg.class, msg.method) {
            (Class::Request, method::BINDING) => Some(
                msg.reply(Class::Success)
                    .with(attr::XOR_MAPPED_ADDRESS, stun::xor_addr(from))
                    .encode(None),
            ),
            (Class::Indication, method::SEND) => {
                let peer = stun::parse_xor_addr(msg.get(attr::XOR_PEER_ADDRESS)?)?;
                let data = msg.get(attr::DATA)?;
                let port = self.state.lock().unwrap().allocs.get(&from)?.port;
                self.forward(peer, port, data);
                None
            }
            (Class::Request, _) => Some(self.request(&msg, buf, from)),
            _ => None,
        }
    }

    fn request(&self, msg: &Message, buf: &[u8], from: SocketAddrV4) -> Vec<u8> {
        let authentic = msg.get(attr::USERNAME) == Some(self.username.as_bytes()) && stun::check_integrity(buf, &self.key);
        if !authentic {
            // The first Allocate always comes without credentials.
            return msg
                .reply(Class::Error)
                .with(attr::ERROR_CODE, stun::error_code(401, "Unauthorized"))
                .with(attr::REALM, REALM.as_bytes().to_vec())
                .with(attr::NONCE, self.nonce.as_bytes().to_vec())
                .encode(None);
        }
        let key = Some(&self.key[..]);
        let error = |code, reason| msg.reply(Class::Error).with(attr::ERROR_CODE, stun::error_code(code, reason)).encode(key);
        let lifetime = |m: Message, secs: u32| m.with(attr::LIFETIME, secs.to_be_bytes().to_vec());
        let mut st = self.state.lock().unwrap();
        match msg.method {
            method::ALLOCATE => {
                if msg.get(attr::REQUESTED_ADDRESS_FAMILY).is_some_and(|f| f.first() == Some(&0x02)) {
                    return error(440, "Address Family not Supported");
                }
                let port = match st.allocs.get(&from) {
                    Some(a) => a.port, // a retransmission
                    None => {
                        let Some(port) = free_port(&mut st) else { return error(508, "Insufficient Capacity") };
                        st.ports.insert(port, from);
                        st.allocs.insert(
                            from,
                            Alloc {
                                port,
                                expires: Instant::now() + Duration::from_secs(LIFETIME.into()),
                                channels: HashMap::new(),
                                by_peer: HashMap::new(),
                            },
                        );
                        port
                    }
                };
                lifetime(
                    msg.reply(Class::Success)
                        .with(attr::XOR_RELAYED_ADDRESS, stun::xor_addr(SocketAddrV4::new(self.me, port)))
                        .with(attr::XOR_MAPPED_ADDRESS, stun::xor_addr(from)),
                    LIFETIME,
                )
                .encode(key)
            }
            method::REFRESH => {
                let secs = msg
                    .get(attr::LIFETIME)
                    .and_then(|v| v.try_into().ok())
                    .map(u32::from_be_bytes)
                    .unwrap_or(LIFETIME)
                    .min(3600);
                if !st.allocs.contains_key(&from) {
                    return error(437, "Allocation Mismatch");
                }
                if secs == 0 {
                    if let Some(a) = st.allocs.remove(&from) {
                        st.ports.remove(&a.port);
                    }
                } else if let Some(a) = st.allocs.get_mut(&from) {
                    a.expires = Instant::now() + Duration::from_secs(secs.into());
                }
                lifetime(msg.reply(Class::Success), secs).encode(key)
            }
            method::CREATE_PERMISSION => {
                // Only friends in the room are reachable at all, so every
                // permission is granted.
                if !st.allocs.contains_key(&from) {
                    return error(437, "Allocation Mismatch");
                }
                msg.reply(Class::Success).encode(key)
            }
            method::CHANNEL_BIND => {
                let channel = msg.get(attr::CHANNEL_NUMBER).and_then(|v| v.get(..2)).map(|v| u16::from_be_bytes([v[0], v[1]]));
                let peer = msg.get(attr::XOR_PEER_ADDRESS).and_then(stun::parse_xor_addr);
                let (Some(channel), Some(peer)) = (channel, peer) else { return error(400, "Bad Request") };
                if !(0x4000..=0x7FFF).contains(&channel) {
                    return error(400, "Bad Request");
                }
                let Some(a) = st.allocs.get_mut(&from) else { return error(437, "Allocation Mismatch") };
                a.channels.insert(channel, peer);
                a.by_peer.insert(peer, channel);
                msg.reply(Class::Success).encode(key)
            }
            _ => error(400, "Bad Request"),
        }
    }

    /// A packet from our allocation `from_port`, for a friend's relayed
    /// address `peer`.
    fn forward(self: &Arc<Self>, peer: SocketAddrV4, from_port: u16, data: &[u8]) {
        let ip = *peer.ip();
        let (link, dial, frame) = {
            let mut st = self.state.lock().unwrap();
            if st.ambiguous.contains(&ip) {
                return;
            }
            let Some(p) = st.peers.get(&ip) else { return };
            let (id, code) = (p.id, p.code.clone());
            let link = st.links.get(&id).cloned();
            let dial = (link.is_none() && st.dialing.insert(id)).then_some((id, code));
            st.next_frame = st.next_frame.wrapping_add(1);
            (link, dial, st.next_frame)
        };
        if let Some((id, code)) = dial {
            tokio::spawn(dial_peer(self.clone(), id, code));
        }
        if let Some(link) = link {
            send_frames(&link, frame, peer.port(), from_port, data);
        }
        // Packets sent while connecting are lost, like UDP's; WebRTC resends.
    }

    /// A packet from a friend for our allocation `to_port`.
    fn deliver(&self, to_port: u16, from: SocketAddrV4, data: &[u8]) {
        let (client, channel, txid) = {
            let mut st = self.state.lock().unwrap();
            let Some(client) = st.ports.get(&to_port).copied() else { return };
            let Some(a) = st.allocs.get(&client) else { return };
            let channel = a.by_peer.get(&from).copied();
            st.next_txid += 1;
            (client, channel, st.next_txid)
        };
        let bytes = match channel {
            Some(ch) => {
                let mut b = Vec::with_capacity(4 + data.len());
                b.extend_from_slice(&ch.to_be_bytes());
                b.extend_from_slice(&(data.len() as u16).to_be_bytes());
                b.extend_from_slice(data);
                b
            }
            None => {
                let mut id = [0u8; 12];
                id[4..].copy_from_slice(&txid.to_be_bytes());
                Message::new(Class::Indication, method::DATA, id)
                    .with(attr::XOR_PEER_ADDRESS, stun::xor_addr(from))
                    .with(attr::DATA, data.to_vec())
                    .encode(None)
            }
        };
        let _ = self.socket.try_send_to(&bytes, client.into());
    }
}

fn free_port(st: &mut State) -> Option<u16> {
    for _ in 0..=(u16::MAX - FIRST_PORT) {
        let port = st.next_port;
        st.next_port = if port == u16::MAX { FIRST_PORT } else { port + 1 };
        if !st.ports.contains_key(&port) {
            return Some(port);
        }
    }
    None
}

// ------------------------------------------------------------- the iroh side

/// Frames for one packet, each small enough for a datagram.
fn frames(max: usize, frame: u16, to_port: u16, from_port: u16, data: &[u8]) -> Vec<Bytes> {
    let room = max.saturating_sub(FRAME_HEADER);
    if room < 64 {
        return Vec::new();
    }
    let chunks: Vec<&[u8]> = if data.is_empty() { vec![data] } else { data.chunks(room).collect() };
    if chunks.len() > 255 {
        return Vec::new();
    }
    let count = chunks.len() as u8;
    chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            let mut b = Vec::with_capacity(FRAME_HEADER + chunk.len());
            b.extend_from_slice(&frame.to_be_bytes());
            b.push(i as u8);
            b.push(count);
            b.extend_from_slice(&to_port.to_be_bytes());
            b.extend_from_slice(&from_port.to_be_bytes());
            b.extend_from_slice(chunk);
            Bytes::from(b)
        })
        .collect()
}

fn send_frames(link: &Connection, frame: u16, to_port: u16, from_port: u16, data: &[u8]) {
    let Some(max) = link.max_datagram_size() else { return };
    for f in frames(max, frame, to_port, from_port, data) {
        if link.send_datagram(f).is_err() {
            return;
        }
    }
}

/// Puts split packets back together.
#[derive(Default)]
struct Reassembler {
    partial: HashMap<u16, (Instant, u16, u16, Vec<Option<Bytes>>)>,
}

impl Reassembler {
    /// A frame in; a whole packet out (to port, from port, data) once complete.
    fn add(&mut self, frame: &[u8]) -> Option<(u16, u16, Vec<u8>)> {
        if frame.len() < FRAME_HEADER {
            return None;
        }
        let id = u16::from_be_bytes([frame[0], frame[1]]);
        let (part, count) = (frame[2] as usize, frame[3] as usize);
        let to = u16::from_be_bytes([frame[4], frame[5]]);
        let from = u16::from_be_bytes([frame[6], frame[7]]);
        let body = &frame[FRAME_HEADER..];
        if count == 1 {
            return Some((to, from, body.to_vec()));
        }
        if part >= count || count == 0 {
            return None;
        }
        let now = Instant::now();
        self.partial.retain(|_, p| now.duration_since(p.0) < REASSEMBLY);
        let entry = self.partial.entry(id).or_insert_with(|| (now, to, from, vec![None; count]));
        if entry.3.len() != count {
            *entry = (now, to, from, vec![None; count]);
        }
        entry.3[part] = Some(Bytes::copy_from_slice(body));
        if entry.3.iter().all(Option::is_some) {
            let (_, to, from, parts) = self.partial.remove(&id)?;
            return Some((to, from, parts.into_iter().flatten().flat_map(|b| b.to_vec()).collect()));
        }
        None
    }
}

async fn dial_peer(inner: Arc<Inner>, id: EndpointId, code: String) {
    let result = async {
        let addr = net::decode_code(&code)?;
        let conn = tokio::time::timeout(DIAL_TIMEOUT, inner.endpoint.connect(addr, ALPN_RELAY))
            .await
            .map_err(|_| anyhow::anyhow!("timed out"))?
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        anyhow::Ok(conn)
    }
    .await;
    inner.state.lock().unwrap().dialing.remove(&id);
    match result {
        Ok(conn) => read_link(inner, conn).await,
        Err(e) => eprintln!("call relay: couldn't reach {}: {e}", id.fmt_short()),
    }
}

async fn accept(inner: Arc<Inner>, mut incoming: mpsc::UnboundedReceiver<Connection>) {
    while let Some(conn) = incoming.recv().await {
        tokio::spawn(read_link(inner.clone(), conn));
    }
}

/// Receive a friend's packets until the connection closes.
async fn read_link(inner: Arc<Inner>, conn: Connection) {
    let id = conn.remote_id();
    let from_ip = virtual_ip(&id.to_string());
    {
        let mut st = inner.state.lock().unwrap();
        // Only people in the room (or just gone) may use our relay.
        if !st.peers.values().any(|p| p.id == id) || st.ambiguous.contains(&from_ip) {
            drop(st);
            conn.close(0u32.into(), b"not in the room");
            return;
        }
        st.links.entry(id).or_insert_with(|| conn.clone());
    }
    let mut parts = Reassembler::default();
    while let Ok(d) = conn.read_datagram().await {
        if let Some((to, from, data)) = parts.add(&d) {
            inner.deliver(to, SocketAddrV4::new(from_ip, from), &data);
        }
    }
    let mut st = inner.state.lock().unwrap();
    if st.links.get(&id).is_some_and(|c| c.stable_id() == conn.stable_id()) {
        st.links.remove(&id);
    }
}

/// Expire allocations, and friends gone longer than the grace period.
async fn housekeeping(inner: Arc<Inner>) {
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tick.tick().await;
        let now = Instant::now();
        let mut st = inner.state.lock().unwrap();
        let expired: Vec<SocketAddrV4> = st.allocs.iter().filter(|(_, a)| a.expires < now).map(|(c, _)| *c).collect();
        for c in expired {
            if let Some(a) = st.allocs.remove(&c) {
                st.ports.remove(&a.port);
            }
        }
        let gone: Vec<(Ipv4Addr, EndpointId)> = st
            .peers
            .iter()
            .filter(|(_, p)| p.gone.is_some_and(|t| now.duration_since(t) > GRACE))
            .map(|(ip, p)| (*ip, p.id))
            .collect();
        for (ip, id) in gone {
            st.peers.remove(&ip);
            if let Some(c) = st.links.remove(&id) {
                c.close(0u32.into(), b"left the room");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_addresses_are_in_never_routed_ranges_and_stable() {
        for id in ["a", "b", "some endpoint id", "another"] {
            let ip = virtual_ip(id);
            let o = ip.octets();
            assert!(matches!((o[0], o[1], o[2]), (192, 0, 2) | (198, 51, 100) | (203, 0, 113)), "{ip}");
            assert!(o[3] >= 1 && o[3] <= 254);
            assert_eq!(virtual_ip(id), ip);
        }
    }

    #[test]
    fn big_packets_are_split_and_put_back_together() {
        let data: Vec<u8> = (0..3000u32).map(|i| i as u8).collect();
        let parts = frames(1200, 9, 50000, 50001, &data);
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|p| p.len() <= 1200));
        let mut r = Reassembler::default();
        // Out of order is fine.
        assert!(r.add(&parts[2]).is_none());
        assert!(r.add(&parts[0]).is_none());
        assert_eq!(r.add(&parts[1]), Some((50000, 50001, data.clone())));

        // A lost part loses the packet, nothing else.
        let next = frames(1200, 10, 50000, 50001, &data);
        assert!(r.add(&next[0]).is_none());
        let small = frames(1200, 11, 50000, 50001, b"hi");
        assert_eq!(r.add(&small[0]), Some((50000, 50001, b"hi".to_vec())));

        // Too little room for anything useful: nothing sent.
        assert!(frames(40, 1, 1, 1, b"x").is_empty());
    }
}

//! The call's fallback relay end to end: two Macs' relays joined over real
//! iroh, and on each side a TURN client doing what WebRTC does (the 401
//! challenge, Allocate, permissions, Send, ChannelBind and ChannelData).

use std::net::SocketAddrV4;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout};
use watchsync::net;
use watchsync::relay::{REALM, Relay, TurnInfo};
use watchsync::stun::{self, Class, Message, attr, method};

struct Mac {
    relay: Relay,
    id: String,
    code: String,
    _endpoint: iroh::Endpoint,
}

async fn mac() -> Mac {
    let endpoint = timeout(Duration::from_secs(30), net::bind()).await.expect("online").unwrap();
    let (events, _) = mpsc::unbounded_channel();
    let (relay_tx, relay_rx) = mpsc::unbounded_channel();
    net::listen(endpoint.clone(), events, Arc::new(AtomicBool::new(false)), relay_tx);
    let relay = Relay::start(endpoint.clone(), relay_rx).await.unwrap();
    Mac { id: net::id_of(&endpoint), code: net::code_of(&endpoint), relay, _endpoint: endpoint }
}

/// What WebRTC does with a TURN server, minimally.
struct Client {
    sock: UdpSocket,
    server: SocketAddrV4,
    key: [u8; 16],
    info: TurnInfo,
    nonce: Vec<u8>,
    tx: u8,
}

impl Client {
    async fn new(info: &TurnInfo) -> Self {
        let port: u16 = info.urls.trim_start_matches("turn:127.0.0.1:").split('?').next().unwrap().parse().unwrap();
        Self {
            sock: UdpSocket::bind("127.0.0.1:0").await.unwrap(),
            server: SocketAddrV4::new([127, 0, 0, 1].into(), port),
            key: stun::long_term_key(&info.username, REALM, &info.credential),
            info: info.clone(),
            nonce: Vec::new(),
            tx: 0,
        }
    }

    async fn recv(&self) -> Vec<u8> {
        let mut buf = vec![0u8; 65536];
        let n = timeout(Duration::from_secs(5), self.sock.recv(&mut buf)).await.expect("a reply").unwrap();
        buf.truncate(n);
        buf
    }

    /// A signed request; returns the (verified) response.
    async fn request(&mut self, method: u16, attrs: Vec<(u16, Vec<u8>)>) -> Message {
        self.tx += 1;
        let mut m = Message::new(Class::Request, method, [self.tx; 12])
            .with(attr::USERNAME, self.info.username.as_bytes().to_vec())
            .with(attr::REALM, REALM.as_bytes().to_vec())
            .with(attr::NONCE, self.nonce.clone());
        m.attrs.extend(attrs);
        self.sock.send_to(&m.encode(Some(&self.key)), self.server).await.unwrap();
        let reply = self.recv().await;
        assert!(stun::check_integrity(&reply, &self.key), "replies to signed requests are signed");
        Message::parse(&reply).unwrap()
    }

    async fn allocate(&mut self) -> SocketAddrV4 {
        // First without credentials, like WebRTC: a 401 tells us the realm and nonce.
        let m = Message::new(Class::Request, method::ALLOCATE, [0; 12]).with(0x0019, vec![17, 0, 0, 0]);
        self.sock.send_to(&m.encode(None), self.server).await.unwrap();
        let challenge = Message::parse(&self.recv().await).unwrap();
        assert_eq!(challenge.class, Class::Error);
        assert_eq!(&challenge.get(attr::ERROR_CODE).unwrap()[2..4], &[4, 1]);
        assert_eq!(challenge.get(attr::REALM), Some(REALM.as_bytes()));
        self.nonce = challenge.get(attr::NONCE).unwrap().to_vec();

        let ok = self.request(method::ALLOCATE, vec![(0x0019, vec![17, 0, 0, 0])]).await;
        assert_eq!(ok.class, Class::Success);
        assert!(ok.get(attr::LIFETIME).is_some() && ok.get(attr::XOR_MAPPED_ADDRESS).is_some());
        stun::parse_xor_addr(ok.get(attr::XOR_RELAYED_ADDRESS).unwrap()).unwrap()
    }

    async fn send(&self, to: SocketAddrV4, data: &[u8]) {
        let m = Message::new(Class::Indication, method::SEND, [9; 12])
            .with(attr::XOR_PEER_ADDRESS, stun::xor_addr(to))
            .with(attr::DATA, data.to_vec());
        self.sock.send_to(&m.encode(None), self.server).await.unwrap();
    }

    /// Next Data indication: (from, data).
    async fn data(&self) -> Option<(SocketAddrV4, Vec<u8>)> {
        let mut buf = vec![0u8; 65536];
        let n = timeout(Duration::from_millis(300), self.sock.recv(&mut buf)).await.ok()?.ok()?;
        let m = Message::parse(&buf[..n])?;
        (m.method == method::DATA).then(|| (stun::parse_xor_addr(m.get(attr::XOR_PEER_ADDRESS).unwrap()).unwrap(), m.get(attr::DATA).unwrap().to_vec()))
    }
}

/// Keep sending until it arrives (the first packets go while the Macs connect).
async fn until_arrives(from: &Client, to: &Client, dest: SocketAddrV4, data: &[u8]) -> (SocketAddrV4, Vec<u8>) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        from.send(dest, data).await;
        if let Some(got) = to.data().await {
            return got;
        }
        assert!(Instant::now() < deadline, "the packet never arrived");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_macs_relay_call_packets_over_iroh() {
    let (a, b) = (mac().await, mac().await);
    a.relay.set_peers([(b.id.as_str(), b.code.as_str())]);
    b.relay.set_peers([(a.id.as_str(), a.code.as_str())]);

    let mut ca = Client::new(&a.relay.info).await;
    let mut cb = Client::new(&b.relay.info).await;
    let (ra, rb) = (ca.allocate().await, cb.allocate().await);
    assert_ne!(ra.ip(), rb.ip(), "each Mac has its own virtual address");
    for (c, peer) in [(&mut ca, rb), (&mut cb, ra)] {
        let ok = c.request(method::CREATE_PERMISSION, vec![(attr::XOR_PEER_ADDRESS, stun::xor_addr(peer))]).await;
        assert_eq!(ok.class, Class::Success);
    }

    // A → B with Send indications.
    let (from, data) = until_arrives(&ca, &cb, rb, b"hello from a").await;
    assert_eq!((from, data.as_slice()), (ra, &b"hello from a"[..]));

    // B → A over a channel, with a packet too big for one datagram.
    let ok = cb
        .request(method::CHANNEL_BIND, vec![(attr::CHANNEL_NUMBER, vec![0x40, 0x01, 0, 0]), (attr::XOR_PEER_ADDRESS, stun::xor_addr(ra))])
        .await;
    assert_eq!(ok.class, Class::Success);
    let big: Vec<u8> = (0..3000u32).map(|i| (i * 7) as u8).collect();
    let mut channel_data = vec![0x40, 0x01];
    channel_data.extend_from_slice(&(big.len() as u16).to_be_bytes());
    channel_data.extend_from_slice(&big);
    let deadline = Instant::now() + Duration::from_secs(10);
    let got = loop {
        cb.sock.send_to(&channel_data, cb.server).await.unwrap();
        if let Some(got) = ca.data().await {
            break got;
        }
        assert!(Instant::now() < deadline, "the big packet never arrived");
    };
    assert_eq!(got, (rb, big));

    // A host handover briefly empties the room list: the relay keeps going.
    a.relay.set_peers([]);
    b.relay.set_peers([]);
    a.relay.set_peers([(b.id.as_str(), b.code.as_str())]);
    b.relay.set_peers([(a.id.as_str(), a.code.as_str())]);
    // (B → A: A has no channel, so it arrives as a Data indication.)
    until_arrives(&cb, &ca, ra, b"still here").await;

    // Freeing the allocation stops delivery to it.
    let ok = ca.request(method::REFRESH, vec![(attr::LIFETIME, 0u32.to_be_bytes().to_vec())]).await;
    assert_eq!(ok.class, Class::Success);
    cb.send(ra, b"anyone?").await;
    assert!(ca.data().await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_credentials_get_nothing() {
    let a = mac().await;
    let mut info = a.relay.info.clone();
    info.credential = "guess".into();
    let mut c = Client::new(&info).await;
    c.nonce = b"x".to_vec();
    c.tx += 1;
    let m = Message::new(Class::Request, method::ALLOCATE, [1; 12])
        .with(attr::USERNAME, info.username.as_bytes().to_vec())
        .with(attr::REALM, REALM.as_bytes().to_vec());
    c.sock.send_to(&m.encode(Some(&c.key)), c.server).await.unwrap();
    let reply = Message::parse(&c.recv().await).unwrap();
    assert_eq!(reply.class, Class::Error);
    assert!(reply.get(attr::XOR_RELAYED_ADDRESS).is_none());
}

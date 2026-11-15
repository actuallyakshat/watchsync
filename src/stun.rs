//! Just enough STUN (RFC 8489) to run a TURN server (RFC 8656) for the call's
//! own fallback relay: messages, attributes, XOR addresses and
//! MESSAGE-INTEGRITY with long-term credentials.

use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use sha1::Sha1;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

pub const MAGIC: u32 = 0x2112_A442;
const HEADER: usize = 20;

pub mod method {
    pub const BINDING: u16 = 0x001;
    pub const ALLOCATE: u16 = 0x003;
    pub const REFRESH: u16 = 0x004;
    pub const SEND: u16 = 0x006;
    pub const DATA: u16 = 0x007;
    pub const CREATE_PERMISSION: u16 = 0x008;
    pub const CHANNEL_BIND: u16 = 0x009;
}

pub mod attr {
    pub const USERNAME: u16 = 0x0006;
    pub const MESSAGE_INTEGRITY: u16 = 0x0008;
    pub const ERROR_CODE: u16 = 0x0009;
    pub const CHANNEL_NUMBER: u16 = 0x000C;
    pub const LIFETIME: u16 = 0x000D;
    pub const XOR_PEER_ADDRESS: u16 = 0x0012;
    pub const DATA: u16 = 0x0013;
    pub const REALM: u16 = 0x0014;
    pub const NONCE: u16 = 0x0015;
    pub const XOR_RELAYED_ADDRESS: u16 = 0x0016;
    pub const REQUESTED_ADDRESS_FAMILY: u16 = 0x0017;
    pub const XOR_MAPPED_ADDRESS: u16 = 0x0020;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Request,
    Indication,
    Success,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub class: Class,
    pub method: u16,
    pub txid: [u8; 12],
    pub attrs: Vec<(u16, Vec<u8>)>,
}

/// TURN ChannelData messages start with 0b01; STUN messages with 0b00.
pub fn is_channel_data(buf: &[u8]) -> bool {
    buf.first().is_some_and(|b| b & 0xC0 == 0x40)
}

impl Message {
    pub fn new(class: Class, method: u16, txid: [u8; 12]) -> Self {
        Self {
            class,
            method,
            txid,
            attrs: Vec::new(),
        }
    }

    /// A reply to `self` (same method and transaction).
    pub fn reply(&self, class: Class) -> Self {
        Self::new(class, self.method, self.txid)
    }

    pub fn with(mut self, kind: u16, value: Vec<u8>) -> Self {
        self.attrs.push((kind, value));
        self
    }

    pub fn get(&self, kind: u16) -> Option<&[u8]> {
        self.attrs.iter().find(|(k, _)| *k == kind).map(|(_, v)| v.as_slice())
    }

    pub fn all(&self, kind: u16) -> impl Iterator<Item = &[u8]> {
        self.attrs.iter().filter(move |(k, _)| *k == kind).map(|(_, v)| v.as_slice())
    }

    pub fn parse(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER || buf[0] & 0xC0 != 0 {
            return None;
        }
        let ty = u16::from_be_bytes([buf[0], buf[1]]);
        let len = u16::from_be_bytes([buf[2], buf[3]]) as usize;
        if u32::from_be_bytes(buf[4..8].try_into().ok()?) != MAGIC || len % 4 != 0 || HEADER + len > buf.len() {
            return None;
        }
        let method = (ty & 0x000F) | ((ty >> 1) & 0x0070) | ((ty >> 2) & 0x0F80);
        let class = match ((ty >> 4) & 1) | ((ty >> 7) & 2) {
            0 => Class::Request,
            1 => Class::Indication,
            2 => Class::Success,
            _ => Class::Error,
        };
        let mut attrs = Vec::new();
        let body = &buf[HEADER..HEADER + len];
        let mut i = 0;
        while i + 4 <= body.len() {
            let kind = u16::from_be_bytes([body[i], body[i + 1]]);
            let n = u16::from_be_bytes([body[i + 2], body[i + 3]]) as usize;
            let value = body.get(i + 4..i + 4 + n)?;
            attrs.push((kind, value.to_vec()));
            i += 4 + n.div_ceil(4) * 4;
        }
        Some(Self {
            class,
            method,
            txid: buf[8..20].try_into().ok()?,
            attrs,
        })
    }

    /// The message's bytes, signed with MESSAGE-INTEGRITY if `key` is given.
    pub fn encode(&self, key: Option<&[u8]>) -> Vec<u8> {
        let c = match self.class {
            Class::Request => 0u16,
            Class::Indication => 1,
            Class::Success => 2,
            Class::Error => 3,
        };
        let m = self.method;
        let ty = (m & 0x000F) | ((m & 0x0070) << 1) | ((m & 0x0F80) << 2) | ((c & 1) << 4) | ((c & 2) << 7);
        let mut out = Vec::with_capacity(128);
        out.extend_from_slice(&ty.to_be_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&MAGIC.to_be_bytes());
        out.extend_from_slice(&self.txid);
        for (kind, value) in &self.attrs {
            out.extend_from_slice(&kind.to_be_bytes());
            out.extend_from_slice(&(value.len() as u16).to_be_bytes());
            out.extend_from_slice(value);
            out.resize(out.len().div_ceil(4) * 4, 0);
        }
        if let Some(key) = key {
            let len = (out.len() - HEADER + 24) as u16;
            out[2..4].copy_from_slice(&len.to_be_bytes());
            let mac = hmac_sha1(key, &out);
            out.extend_from_slice(&attr::MESSAGE_INTEGRITY.to_be_bytes());
            out.extend_from_slice(&20u16.to_be_bytes());
            out.extend_from_slice(&mac);
        } else {
            let len = (out.len() - HEADER) as u16;
            out[2..4].copy_from_slice(&len.to_be_bytes());
        }
        out
    }
}

fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    mac.update(data);
    mac.finalize().into_bytes().into()
}

/// The long-term credential key: MD5(username ":" realm ":" password).
pub fn long_term_key(username: &str, realm: &str, password: &str) -> [u8; 16] {
    Md5::digest(format!("{username}:{realm}:{password}").as_bytes()).into()
}

/// Does `buf`'s MESSAGE-INTEGRITY match `key`? (Attributes after it, like
/// FINGERPRINT, are not covered, as the RFC says.)
pub fn check_integrity(buf: &[u8], key: &[u8]) -> bool {
    let Some(len) = buf.get(2..4).map(|b| u16::from_be_bytes([b[0], b[1]]) as usize) else {
        return false;
    };
    let end = (HEADER + len).min(buf.len());
    let mut i = HEADER;
    while i + 4 <= end {
        let kind = u16::from_be_bytes([buf[i], buf[i + 1]]);
        let n = u16::from_be_bytes([buf[i + 2], buf[i + 3]]) as usize;
        if kind == attr::MESSAGE_INTEGRITY {
            if n != 20 || i + 24 > end {
                return false;
            }
            let mut signed = buf[..i].to_vec();
            signed[2..4].copy_from_slice(&((i - HEADER + 24) as u16).to_be_bytes());
            return hmac_sha1(key, &signed) == buf[i + 4..i + 24];
        }
        i += 4 + n.div_ceil(4) * 4;
    }
    false
}

pub fn xor_addr(addr: SocketAddrV4) -> Vec<u8> {
    let port = addr.port() ^ (MAGIC >> 16) as u16;
    let ip = u32::from(*addr.ip()) ^ MAGIC;
    let mut v = vec![0, 0x01];
    v.extend_from_slice(&port.to_be_bytes());
    v.extend_from_slice(&ip.to_be_bytes());
    v
}

/// IPv4 only (the relay never hands out anything else).
pub fn parse_xor_addr(v: &[u8]) -> Option<SocketAddrV4> {
    if v.len() < 8 || v[1] != 0x01 {
        return None;
    }
    let port = u16::from_be_bytes([v[2], v[3]]) ^ (MAGIC >> 16) as u16;
    let ip = u32::from_be_bytes(v[4..8].try_into().ok()?) ^ MAGIC;
    Some(SocketAddrV4::new(Ipv4Addr::from(ip), port))
}

pub fn v4(addr: SocketAddr) -> Option<SocketAddrV4> {
    match addr {
        SocketAddr::V4(a) => Some(a),
        SocketAddr::V6(_) => None,
    }
}

pub fn error_code(code: u16, reason: &str) -> Vec<u8> {
    let mut v = vec![0, 0, (code / 100) as u8, (code % 100) as u8];
    v.extend_from_slice(reason.as_bytes());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 5769 §2.1, "Sample Request".
    const SAMPLE: [u8; 108] = [
        0x00, 0x01, 0x00, 0x58, 0x21, 0x12, 0xa4, 0x42, 0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf,
        0xae, 0x80, 0x22, 0x00, 0x10, 0x53, 0x54, 0x55, 0x4e, 0x20, 0x74, 0x65, 0x73, 0x74, 0x20, 0x63, 0x6c, 0x69, 0x65,
        0x6e, 0x74, 0x00, 0x24, 0x00, 0x04, 0x6e, 0x00, 0x01, 0xff, 0x80, 0x29, 0x00, 0x08, 0x93, 0x2f, 0xf9, 0xb1, 0x51,
        0x26, 0x3b, 0x36, 0x00, 0x06, 0x00, 0x09, 0x65, 0x76, 0x74, 0x6a, 0x3a, 0x68, 0x36, 0x76, 0x59, 0x20, 0x20, 0x20,
        0x00, 0x08, 0x00, 0x14, 0x9a, 0xea, 0xa7, 0x0c, 0xbf, 0xd8, 0xcb, 0x56, 0x78, 0x1e, 0xf2, 0xb5, 0xb2, 0xd3, 0xf2,
        0x49, 0xc1, 0xb5, 0x71, 0xa2, 0x80, 0x28, 0x00, 0x04, 0xe5, 0x7a, 0x3b, 0xcf,
    ];

    #[test]
    fn the_rfc_sample_request_parses_and_its_integrity_checks_out() {
        let m = Message::parse(&SAMPLE).unwrap();
        assert_eq!((m.class, m.method), (Class::Request, method::BINDING));
        assert_eq!(m.get(attr::USERNAME), Some(&b"evtj:h6vY"[..]));
        assert!(check_integrity(&SAMPLE, b"VOkJxbRl1RmTxUk/WvJxBt"));
        assert!(!check_integrity(&SAMPLE, b"wrong"));
    }

    #[test]
    fn what_we_sign_checks_out_and_round_trips() {
        let key = long_term_key("user", "watchsync", "secret");
        let addr = SocketAddrV4::new(Ipv4Addr::new(203, 0, 113, 7), 50001);
        let m = Message::new(Class::Success, method::ALLOCATE, [7; 12])
            .with(attr::XOR_RELAYED_ADDRESS, xor_addr(addr))
            .with(attr::USERNAME, b"odd".to_vec()) // needs padding
            .with(attr::LIFETIME, 600u32.to_be_bytes().to_vec());
        let bytes = m.encode(Some(&key));
        assert!(check_integrity(&bytes, &key));
        let back = Message::parse(&bytes).unwrap();
        assert_eq!((back.class, back.method, back.txid), (Class::Success, method::ALLOCATE, [7; 12]));
        assert_eq!(parse_xor_addr(back.get(attr::XOR_RELAYED_ADDRESS).unwrap()), Some(addr));
        assert_eq!(back.get(attr::USERNAME), Some(&b"odd"[..]));
        // Every class and method survives the type field's bit shuffle.
        for class in [Class::Request, Class::Indication, Class::Success, Class::Error] {
            for method in [method::BINDING, method::SEND, method::CHANNEL_BIND] {
                let m = Message::new(class, method, [1; 12]);
                let back = Message::parse(&m.encode(None)).unwrap();
                assert_eq!((back.class, back.method), (class, method));
            }
        }
    }

    #[test]
    fn channel_data_is_told_apart() {
        assert!(is_channel_data(&[0x40, 0x00, 0, 0]));
        assert!(!is_channel_data(&SAMPLE));
        assert!(Message::parse(&[0x40, 0, 0, 0]).is_none());
    }
}

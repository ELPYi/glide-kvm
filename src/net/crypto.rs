//! Packet sealing: ChaCha20-Poly1305 with a per-session key derived from the
//! shared secret, plus a sliding replay window.
//!
//! Packet layout:  [magic:1][dir:1][session:8][counter:8] [ciphertext+tag]
//! The 18-byte header is authenticated as associated data.

use chacha20poly1305::aead::{Aead, KeyInit, OsRng, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use sha2::{Digest, Sha256};

pub const MAGIC: u8 = 0x47; // 'G'
pub const HEADER_LEN: usize = 18;
pub const DIR_TO_CLIENT: u8 = 0;
pub const DIR_TO_SERVER: u8 = 1;
/// Session id used for [`ClientMsg::Knock`](super::protocol::ClientMsg::Knock),
/// which a client sends before it knows the server's real session.
pub const KNOCK_SESSION: u64 = 0;

pub type Cipher = ChaCha20Poly1305;

pub fn session_cipher(secret: &str, session: u64) -> Cipher {
    let mut h = Sha256::new();
    h.update(b"glide-v4\0");
    h.update(secret.as_bytes());
    h.update(b"\0");
    h.update(session.to_le_bytes());
    let key: [u8; 32] = h.finalize().into();
    ChaCha20Poly1305::new(Key::from_slice(&key))
}

pub fn generate_secret() -> String {
    let key = ChaCha20Poly1305::generate_key(&mut OsRng);
    key.iter().map(|b| format!("{b:02x}")).collect()
}

fn nonce(dir: u8, counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[0] = dir;
    n[4..].copy_from_slice(&counter.to_le_bytes());
    n
}

pub struct Header {
    pub dir: u8,
    pub session: u64,
    pub counter: u64,
}

pub fn parse_header(buf: &[u8]) -> Option<Header> {
    if buf.len() < HEADER_LEN + 16 || buf[0] != MAGIC {
        return None;
    }
    Some(Header {
        dir: buf[1],
        session: u64::from_le_bytes(buf[2..10].try_into().ok()?),
        counter: u64::from_le_bytes(buf[10..18].try_into().ok()?),
    })
}

pub fn seal(cipher: &Cipher, dir: u8, session: u64, counter: u64, msg: &[u8]) -> Vec<u8> {
    let mut header = [0u8; HEADER_LEN];
    header[0] = MAGIC;
    header[1] = dir;
    header[2..10].copy_from_slice(&session.to_le_bytes());
    header[10..18].copy_from_slice(&counter.to_le_bytes());
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce(dir, counter)), Payload { msg, aad: &header })
        .expect("chacha20poly1305 encrypt cannot fail");
    let mut out = Vec::with_capacity(HEADER_LEN + ct.len());
    out.extend_from_slice(&header);
    out.extend_from_slice(&ct);
    out
}

/// Authenticates and decrypts. Caller must check the replay window afterwards.
pub fn open(cipher: &Cipher, h: &Header, buf: &[u8]) -> Option<Vec<u8>> {
    let (header, ct) = buf.split_at(HEADER_LEN);
    cipher
        .decrypt(
            Nonce::from_slice(&nonce(h.dir, h.counter)),
            Payload { msg: ct, aad: header },
        )
        .ok()
}

/// Accepts each counter at most once; tolerates up to 64 packets of reordering.
#[derive(Default)]
pub struct ReplayWindow {
    top: u64,
    bits: u64,
    any: bool,
}

impl ReplayWindow {
    pub fn accept(&mut self, ctr: u64) -> bool {
        if !self.any {
            self.any = true;
            self.top = ctr;
            self.bits = 1;
            return true;
        }
        if ctr > self.top {
            let shift = ctr - self.top;
            self.bits = if shift >= 64 { 0 } else { self.bits << shift };
            self.bits |= 1;
            self.top = ctr;
            true
        } else {
            let back = self.top - ctr;
            if back >= 64 || self.bits & (1 << back) != 0 {
                false
            } else {
                self.bits |= 1 << back;
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open() {
        let c = session_cipher("pw", 42);
        let pkt = seal(&c, DIR_TO_CLIENT, 42, 7, b"hello");
        let h = parse_header(&pkt).unwrap();
        assert_eq!((h.session, h.counter), (42, 7));
        assert_eq!(open(&c, &h, &pkt).unwrap(), b"hello");

        let wrong = session_cipher("other", 42);
        assert!(open(&wrong, &h, &pkt).is_none());

        let mut tampered = pkt.clone();
        tampered[11] ^= 1; // counter is authenticated
        let h2 = parse_header(&tampered).unwrap();
        assert!(open(&c, &h2, &tampered).is_none());
    }

    #[test]
    fn replay() {
        let mut w = ReplayWindow::default();
        assert!(w.accept(10));
        assert!(!w.accept(10));
        assert!(w.accept(12));
        assert!(w.accept(11)); // reordered, still fine
        assert!(!w.accept(11));
        assert!(w.accept(200));
        assert!(!w.accept(12)); // too old
    }
}

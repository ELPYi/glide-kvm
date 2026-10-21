//! Pairing: connect a new PC with a short code instead of copying the
//! 64-character secret.
//!
//! The sharing PC shows a 6-digit code. The new PC sends it through SPAKE2, a
//! *password-authenticated key exchange*: both sides end up with the same
//! strong key only if they used the same code, and an eavesdropper (or a
//! guesser) learns nothing that lets them test codes offline. Each guess
//! needs a live attempt, and a code allows only [`MAX_ATTEMPTS`]. The
//! server then sends the real secret encrypted under that key.
//!
//! ```text
//! client                                       server (showing code)
//!   Hello  { spake_a, name }        ──────►    finish SPAKE2 -> K
//!                                   ◄──────    Reply  { spake_b, AEAD_K(secret, port) }
//!   finish SPAKE2 -> K, decrypt
//!   (wrong code => decrypt fails)
//!   Confirm{ AEAD_K(name) }         ──────►    proves the client knew the code
//!                                   ◄──────    Ack    { AEAD_K("ok") }   -> device added
//! ```
//!
//! Packets start with [`MAGIC`] (`'P'`) so they never collide with session
//! traffic (`'G'`). Both sides retransmit until answered (UDP can drop).

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use chacha20poly1305::aead::{Aead, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use spake2::{Ed25519Group, Identity, Password, Spake2};

pub const MAGIC: u8 = 0x50; // 'P'
/// A code stays valid this long.
pub const CODE_LIFETIME: Duration = Duration::from_secs(300);
/// Wrong tries allowed per code before a new one is generated.
pub const MAX_ATTEMPTS: u8 = 3;
/// An answered attempt that isn't confirmed within this counts as wrong.
const CONFIRM_WITHIN: Duration = Duration::from_secs(10);
const RESEND_EVERY: Duration = Duration::from_millis(300);

const T_POKE: u8 = 0;
const T_HELLO: u8 = 1;
const T_REPLY: u8 = 2;
const T_CONFIRM: u8 = 3;
const T_ACK: u8 = 4;

const SPAKE_MSG_LEN: usize = 33;
const ID_CLIENT: &[u8] = b"glide-pair-client";
const ID_SERVER: &[u8] = b"glide-pair-server";

/// What a successful pairing gives the new PC.
#[derive(Debug, Clone, PartialEq)]
pub struct Paired {
    pub secret: String,
    /// The sharing PC's port (the new PC keeps its own; this is for reaching it).
    pub port: u16,
}

/// A fresh random 6-digit code.
pub fn new_code() -> String {
    use chacha20poly1305::aead::rand_core::RngCore;
    format!("{:06}", OsRng.next_u32() % 1_000_000)
}

/// "482 913", "482-913", " 482913 " all mean the same code.
pub fn normalize_code(input: &str) -> String {
    input.chars().filter(char::is_ascii_digit).collect()
}

/// Display form: "482 913".
pub fn pretty_code(code: &str) -> String {
    if code.len() == 6 {
        format!("{} {}", &code[..3], &code[3..])
    } else {
        code.to_string()
    }
}

/// A packet that makes the *server's* firewall accept the client's first
/// packet (see the server pairing UI's optional "other PC's address").
pub fn poke_packet() -> Vec<u8> {
    vec![MAGIC, T_POKE]
}

fn aead(key: &[u8]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(&key[..32]))
}

/// Each message type uses its own nonce; every key is single-use.
fn nonce(tag: u8) -> Nonce {
    let mut n = [0u8; 12];
    n[0] = tag;
    *Nonce::from_slice(&n)
}

fn seal(key: &[u8], tag: u8, plain: &[u8]) -> Vec<u8> {
    let mut out = vec![MAGIC, tag];
    out.extend(aead(key).encrypt(&nonce(tag), plain).expect("encrypt"));
    out
}

fn open(key: &[u8], tag: u8, pkt: &[u8]) -> Option<Vec<u8>> {
    (pkt.len() > 2 && pkt[0] == MAGIC && pkt[1] == tag)
        .then(|| aead(key).decrypt(&nonce(tag), &pkt[2..]).ok())
        .flatten()
}

// ------------------------------------------------------------------ server

enum State {
    Waiting,
    /// Replied to a hello; waiting for the client's confirm.
    Answered {
        from: SocketAddr,
        hello: Vec<u8>,
        key: Vec<u8>,
        reply: Vec<u8>,
        at: Instant,
    },
    /// Done; kept so a lost ack can be re-sent.
    Done {
        from: SocketAddr,
        ack: Vec<u8>,
    },
}

/// The sharing PC's side of pairing. Pure state machine: feed it packets and
/// time, send what it returns.
pub struct ServerPairing {
    code: String,
    created: Instant,
    failed_attempts: u8,
    secret: String,
    port: u16,
    state: State,
}

/// Result of feeding one packet to [`ServerPairing::handle`].
#[derive(Default)]
pub struct Step {
    pub reply: Option<Vec<u8>>,
    /// A new PC proved it knew the code: its chosen name.
    pub paired: Option<String>,
}

impl ServerPairing {
    pub fn new(secret: &str, port: u16, now: Instant) -> Self {
        ServerPairing {
            code: new_code(),
            created: now,
            failed_attempts: 0,
            secret: secret.to_string(),
            port,
            state: State::Waiting,
        }
    }

    pub fn code(&self) -> &str {
        &self.code
    }

    pub fn expires_in(&self, now: Instant) -> Duration {
        CODE_LIFETIME.saturating_sub(now.duration_since(self.created))
    }

    /// Wrong tries on the current code (resets when a new code is issued).
    pub fn failed_attempts(&self) -> u8 {
        self.failed_attempts
    }

    pub fn is_done(&self) -> bool {
        matches!(self.state, State::Done { .. })
    }

    /// An attempt is in flight (show "checking…").
    pub fn is_checking(&self) -> bool {
        matches!(self.state, State::Answered { .. })
    }

    /// Housekeeping: expire codes, time out unconfirmed attempts.
    pub fn tick(&mut self, now: Instant) {
        if let State::Answered { at, .. } = self.state {
            if now.duration_since(at) > CONFIRM_WITHIN {
                self.fail(now);
            }
        }
        if !self.is_done() && self.expires_in(now).is_zero() {
            self.reissue(now);
        }
    }

    fn fail(&mut self, now: Instant) {
        self.state = State::Waiting;
        self.failed_attempts += 1;
        if self.failed_attempts >= MAX_ATTEMPTS {
            self.reissue(now);
        }
    }

    fn reissue(&mut self, now: Instant) {
        self.code = new_code();
        self.created = now;
        self.failed_attempts = 0;
        self.state = State::Waiting;
    }

    pub fn handle(&mut self, pkt: &[u8], from: SocketAddr, now: Instant) -> Step {
        if pkt.len() < 2 || pkt[0] != MAGIC {
            return Step::default();
        }
        match pkt[1] {
            T_HELLO => self.on_hello(pkt, from, now),
            T_CONFIRM => self.on_confirm(pkt, from, now),
            _ => Step::default(),
        }
    }

    fn on_hello(&mut self, pkt: &[u8], from: SocketAddr, now: Instant) -> Step {
        match &self.state {
            // Retransmitted hello: repeat our reply.
            State::Answered {
                from: f, hello, reply, ..
            } if *f == from && hello == pkt => {
                return Step {
                    reply: Some(reply.clone()),
                    paired: None,
                };
            }
            State::Done { .. } => return Step::default(),
            // A different hello while one was pending: that one evidently failed
            // (e.g. a typo, then a retry). Count it, then handle this one.
            State::Answered { .. } => self.fail(now),
            State::Waiting => {}
        }
        if pkt.len() < 2 + SPAKE_MSG_LEN {
            return Step::default();
        }
        let (s, msg_b) = Spake2::<Ed25519Group>::start_b(
            &Password::new(self.code.as_bytes()),
            &Identity::new(ID_CLIENT),
            &Identity::new(ID_SERVER),
        );
        let Ok(key) = s.finish(&pkt[2..2 + SPAKE_MSG_LEN]) else {
            return Step::default();
        };

        let mut payload = self.port.to_le_bytes().to_vec();
        payload.extend_from_slice(self.secret.as_bytes());
        let sealed = seal(&key, T_REPLY, &payload);
        let mut reply = vec![MAGIC, T_REPLY];
        reply.extend_from_slice(&msg_b);
        reply.extend_from_slice(&sealed[2..]);

        self.state = State::Answered {
            from,
            hello: pkt.to_vec(),
            key,
            reply: reply.clone(),
            at: now,
        };
        Step {
            reply: Some(reply),
            paired: None,
        }
    }

    fn on_confirm(&mut self, pkt: &[u8], from: SocketAddr, now: Instant) -> Step {
        match &self.state {
            State::Done { from: f, ack } if *f == from => Step {
                reply: Some(ack.clone()),
                paired: None,
            },
            State::Answered { from: f, key, .. } if *f == from => match open(key, T_CONFIRM, pkt) {
                Some(name) => {
                    let ack = seal(key, T_ACK, b"ok");
                    let name = String::from_utf8_lossy(&name).trim().to_string();
                    self.state = State::Done { from, ack: ack.clone() };
                    Step {
                        reply: Some(ack),
                        paired: Some(if name.is_empty() { "New PC".into() } else { name }),
                    }
                }
                None => {
                    self.fail(now);
                    Step::default()
                }
            },
            _ => Step::default(),
        }
    }
}

// ------------------------------------------------------------------ client

/// Pair with the server at `server` using `code`, over `sock` (bind it to
/// glide's port so the server's optional poke reaches it). Blocks up to
/// `timeout`. `name` is how this PC will appear on the server.
pub fn pair_with(sock: &UdpSocket, server: SocketAddr, code: &str, name: &str, timeout: Duration) -> Result<Paired> {
    let code = normalize_code(code);
    if code.len() != 6 {
        bail!("the code has 6 digits");
    }
    let deadline = Instant::now() + timeout;
    sock.set_read_timeout(Some(Duration::from_millis(50)))?;

    let (s, msg_a) = Spake2::<Ed25519Group>::start_a(
        &Password::new(code.as_bytes()),
        &Identity::new(ID_CLIENT),
        &Identity::new(ID_SERVER),
    );
    let name_bytes: Vec<u8> = name.as_bytes().iter().copied().take(64).collect();
    let mut hello = vec![MAGIC, T_HELLO];
    hello.extend_from_slice(&msg_a);
    hello.extend_from_slice(&name_bytes);

    // 1. Hello until we get a reply.
    let reply = exchange(sock, server, &hello, deadline, |p| {
        p.len() > 2 + SPAKE_MSG_LEN && p[1] == T_REPLY
    })
    .ok_or_else(|| anyhow!("no answer from {server}. Is glide open there with a pairing code showing?"))?;
    let key = s
        .finish(&reply[2..2 + SPAKE_MSG_LEN])
        .map_err(|_| anyhow!("pairing failed"))?;
    let mut sealed = vec![MAGIC, T_REPLY];
    sealed.extend_from_slice(&reply[2 + SPAKE_MSG_LEN..]);
    let payload = open(&key, T_REPLY, &sealed).ok_or_else(|| anyhow!("wrong code"))?;
    if payload.len() < 3 {
        bail!("pairing failed");
    }
    let port = u16::from_le_bytes([payload[0], payload[1]]);
    let secret = String::from_utf8(payload[2..].to_vec()).map_err(|_| anyhow!("pairing failed"))?;

    // 2. Confirm until acked.
    let confirm = seal(&key, T_CONFIRM, &name_bytes);
    exchange(sock, server, &confirm, deadline, |p| open(&key, T_ACK, p).is_some())
        .ok_or_else(|| anyhow!("{server} stopped answering while pairing"))?;
    Ok(Paired { secret, port })
}

/// Send `pkt` every [`RESEND_EVERY`] until a packet from `server` passes `accept`.
fn exchange(
    sock: &UdpSocket,
    server: SocketAddr,
    pkt: &[u8],
    deadline: Instant,
    accept: impl Fn(&[u8]) -> bool,
) -> Option<Vec<u8>> {
    let mut buf = [0u8; 1500];
    let mut last_send: Option<Instant> = None;
    while Instant::now() < deadline {
        if last_send.is_none_or(|t| t.elapsed() >= RESEND_EVERY) {
            let _ = sock.send_to(pkt, server);
            last_send = Some(Instant::now());
        }
        if let Ok((n, src)) = sock.recv_from(&mut buf) {
            let p = &buf[..n];
            if src.ip() == server.ip() && p.len() >= 2 && p[0] == MAGIC && accept(p) {
                return Some(p.to_vec());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Runs a ServerPairing on a loopback socket; returns (address, state, names paired).
    type Served = (SocketAddr, Arc<Mutex<ServerPairing>>, Arc<Mutex<Vec<String>>>);

    fn serve(secret: &str) -> Served {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = sock.local_addr().unwrap();
        let pairing = Arc::new(Mutex::new(ServerPairing::new(secret, 24801, Instant::now())));
        let paired = Arc::new(Mutex::new(Vec::new()));
        let (p, names) = (pairing.clone(), paired.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 1500];
            loop {
                let (n, src) = sock.recv_from(&mut buf).unwrap();
                let step = p.lock().unwrap().handle(&buf[..n], src, Instant::now());
                if let Some(r) = step.reply {
                    sock.send_to(&r, src).unwrap();
                }
                if let Some(name) = step.paired {
                    names.lock().unwrap().push(name);
                }
            }
        });
        (addr, pairing, paired)
    }

    fn client() -> UdpSocket {
        UdpSocket::bind("127.0.0.1:0").unwrap()
    }

    #[test]
    fn right_code_transfers_the_secret() {
        let (addr, pairing, names) = serve("the-real-secret-0123456789");
        let code = pairing.lock().unwrap().code().to_string();
        let typed = pretty_code(&code); // "123 456", as a person would type it
        let got = pair_with(&client(), addr, &typed, "laptop", Duration::from_secs(5)).unwrap();
        assert_eq!(
            got,
            Paired {
                secret: "the-real-secret-0123456789".into(),
                port: 24801
            }
        );
        assert_eq!(*names.lock().unwrap(), vec!["laptop".to_string()]);
        assert!(pairing.lock().unwrap().is_done());
    }

    #[test]
    fn wrong_code_fails_and_a_retry_with_the_right_one_works() {
        let (addr, pairing, names) = serve("the-real-secret-0123456789");
        let code = pairing.lock().unwrap().code().to_string();
        let wrong = format!("{:06}", (code.parse::<u32>().unwrap() + 1) % 1_000_000);
        let err = pair_with(&client(), addr, &wrong, "x", Duration::from_secs(5)).unwrap_err();
        assert_eq!(err.to_string(), "wrong code");
        assert!(names.lock().unwrap().is_empty());

        // The typo'd attempt is counted, but the code still works.
        pair_with(&client(), addr, &code, "laptop", Duration::from_secs(5)).unwrap();
        assert_eq!(pairing.lock().unwrap().failed_attempts(), 1);
    }

    #[test]
    fn code_is_replaced_after_too_many_wrong_tries() {
        let mut s = ServerPairing::new("secret", 1, Instant::now());
        let now = Instant::now() + Duration::from_secs(60);
        for _ in 0..MAX_ATTEMPTS {
            s.state = State::Answered {
                from: "127.0.0.1:1".parse().unwrap(),
                hello: vec![],
                key: vec![0; 32],
                reply: vec![],
                at: now,
            };
            s.tick(now + CONFIRM_WITHIN + Duration::from_millis(1));
        }
        assert_eq!(s.failed_attempts(), 0, "counter resets with the new code");
        let reissued_at = now + CONFIRM_WITHIN + Duration::from_millis(1);
        assert_eq!(s.expires_in(reissued_at), CODE_LIFETIME, "a fresh code was issued");
        assert!(!s.is_checking());
    }

    #[test]
    fn codes_expire() {
        let t0 = Instant::now();
        let mut s = ServerPairing::new("secret", 1, t0);
        assert_eq!(s.expires_in(t0), CODE_LIFETIME);
        s.tick(t0 + CODE_LIFETIME);
        assert_eq!(
            s.expires_in(t0 + CODE_LIFETIME),
            CODE_LIFETIME,
            "a fresh code was issued"
        );
    }

    #[test]
    fn nobody_answering_times_out() {
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = silent.local_addr().unwrap();
        let err = pair_with(&client(), addr, "123456", "x", Duration::from_millis(400)).unwrap_err();
        assert!(err.to_string().contains("no answer"), "{err}");
    }

    #[test]
    fn code_formats() {
        assert_eq!(normalize_code(" 482-913 "), "482913");
        assert_eq!(pretty_code("482913"), "482 913");
        let c = new_code();
        assert_eq!(c.len(), 6);
        assert!(c.chars().all(|ch| ch.is_ascii_digit()));
    }
}

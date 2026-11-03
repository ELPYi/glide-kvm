//! The two PCs disagreeing about who has the mouse: the sharing PC thinks
//! it's controlling the other one, which has let go on its own (it stopped
//! hearing the sharing PC for a moment, say). Left alone, the cursor would
//! be hidden on one PC and ignored on the other, with nothing to fix it.
//! Each side reports what it thinks, so either can notice and recover.

mod common;

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use common::*;
use glide::config::{ClientEntry, Config};
use glide::input::Side;
use glide::net::crypto;
use glide::net::protocol::{ClientMsg, ReliableEv, ServerMsg};
use glide::platform::{Captured, EdgeHit};
use glide::server::Server;

fn edge_right() -> Captured {
    Captured::Edge(EdgeHit {
        side: Side::Right,
        frac: 0.5,
    })
}

/// Stands in for a client: answers heartbeats (saying whether it's being
/// controlled: `active`) and acknowledges everything, like a real one.
fn fake_client(sock: UdpSocket, active: Arc<AtomicBool>, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        sock.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let mut buf = [0u8; 1500];
        let mut counter = 0;
        while !stop.load(Ordering::Relaxed) {
            let Ok((n, src)) = sock.recv_from(&mut buf) else {
                continue;
            };
            let Some(h) = crypto::parse_header(&buf[..n]) else {
                continue;
            };
            let cipher = crypto::session_cipher(SECRET, h.session);
            let Some(plain) = crypto::open(&cipher, &h, &buf[..n]) else {
                continue;
            };
            let reply = match ServerMsg::decode(&plain) {
                Some(ServerMsg::Heartbeat { sent_micros, .. }) => ClientMsg::Pong {
                    instance: 1,
                    echo_micros: sent_micros,
                    blocked: None,
                    active: Some(active.load(Ordering::Relaxed)),
                },
                Some(ServerMsg::Reliable { seq, .. }) => ClientMsg::Ack { seq },
                _ => continue,
            };
            counter += 1;
            let pkt = crypto::seal(&cipher, crypto::DIR_TO_SERVER, h.session, counter, &reply.encode());
            let _ = sock.send_to(&pkt, src);
        }
    })
}

#[test]
fn takes_control_back_when_the_other_pc_let_go() {
    let client = localhost();
    let client_port = client.local_addr().unwrap().port();
    let active = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let fake = fake_client(client, active.clone(), stop.clone());

    let cfg = Config {
        clients: vec![ClientEntry {
            name: "laptop".into(),
            address: format!("127.0.0.1:{client_port}"),
            side: Side::Right,
            muted: false,
        }],
        switching: classic(),
        ..Config::new_default_with_secret(SECRET)
    };
    let desk = Arc::new(MockDesktop::default());
    let server = Server::start(&cfg, desk.clone(), localhost()).unwrap();
    wait_for("connected", || server.status().peers[0].connected);

    // Crossing works while the other PC follows along...
    active.store(true, Ordering::Relaxed);
    server.on_captured(edge_right());
    assert_eq!(server.status().active, Some(0));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server.status().active, Some(0), "no false alarm while it's in control");

    // ...then it lets go on its own, while still connected.
    active.store(false, Ordering::Relaxed);
    wait_for("control back on this PC", || server.status().active.is_none());
    assert!(server.status().peers[0].connected, "it never went offline");
    assert!(
        !server.grab_flag().load(Ordering::Relaxed),
        "this PC's mouse works again"
    );
    assert_eq!(
        desk.cursor_hidden.lock().unwrap().last(),
        Some(&false),
        "and its cursor shows"
    );

    // And crossing again works (no leftover from the mix-up).
    active.store(true, Ordering::Relaxed);
    server.on_captured(edge_right());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(server.status().active, Some(0));

    stop.store(true, Ordering::Relaxed);
    fake.join().unwrap();
    server.stop();
}

/// Stands in for a server: keeps the client connected with heartbeats that
/// say whether it's controlling the client (`controlling`).
struct FakeServer {
    sock: UdpSocket,
    to: std::net::SocketAddr,
    cipher: crypto::Cipher,
    counter: u64,
}

const SESSION: u64 = 12345;

impl FakeServer {
    fn send(&mut self, msg: &ServerMsg) {
        self.counter += 1;
        let pkt = crypto::seal(
            &self.cipher,
            crypto::DIR_TO_CLIENT,
            SESSION,
            self.counter,
            &msg.encode(),
        );
        self.sock.send_to(&pkt, self.to).unwrap();
    }

    fn heartbeat(&mut self, controlling: bool) {
        self.send(&ServerMsg::Heartbeat {
            sent_micros: 1,
            wants_sound: false,
            controlling: Some(controlling),
        });
    }

    /// Heartbeats every 25 ms for `ms` (a real server's pace).
    fn heartbeats_for(&mut self, ms: u64, controlling: bool) {
        for _ in 0..ms / 25 {
            self.heartbeat(controlling);
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}

#[test]
fn lets_go_when_the_sharing_pc_stopped_controlling_it() {
    let client_sock = localhost();
    let to = client_sock.local_addr().unwrap();
    let desk = Arc::new(MockDesktop::default());
    *desk.cursor.lock().unwrap() = Some((10, 10));
    let client = start_client(client_sock, desk.clone(), None);
    let mut srv = FakeServer {
        sock: localhost(),
        to,
        cipher: crypto::session_cipher(SECRET, SESSION),
        counter: 0,
    };
    srv.heartbeat(false);
    wait_for("connected", || client.status().connected);

    srv.send(&ServerMsg::Reliable {
        seq: 1,
        ev: ReliableEv::Enter {
            side: Side::Left,
            frac: 0.5,
            feel: None,
        },
    });
    wait_for("being controlled", || client.status().active);
    srv.heartbeats_for(300, true);
    assert!(client.status().active, "no false alarm while it is in control");

    // The sharing PC took control back, but the Leave never got here.
    srv.heartbeats_for(300, false);
    assert!(!client.status().active, "let go");
    assert!(client.status().connected, "still connected");
    assert_eq!(
        desk.cursor_hidden.lock().unwrap().last(),
        Some(&true),
        "its cursor hides, as after a Leave"
    );
    client.stop();
}

//! A busy network drops mouse packets. The cursor on the other PC must still
//! travel as far as the hand did: each packet carries the running total, so
//! the next one to arrive makes up for any lost before it.

mod common;

use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use glide::input::Side;
use glide::net::crypto;
use glide::net::protocol::{MotionTally, ReliableEv, ServerMsg};

const SESSION: u64 = 777;

/// Stands in for a server, sending whatever the test hands it.
struct FakeServer {
    sock: UdpSocket,
    to: std::net::SocketAddr,
    cipher: crypto::Cipher,
    counter: u64,
}

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

    fn heartbeat(&mut self) {
        self.send(&ServerMsg::Heartbeat {
            sent_micros: 1,
            wants_sound: false,
            controlling: Some(true),
        });
    }

    fn enter(&mut self, tally: Option<MotionTally>) {
        self.send(&ServerMsg::Reliable {
            seq: 1,
            ev: ReliableEv::Enter {
                side: Side::Left,
                frac: 0.5,
                feel: None,
                tally,
            },
        });
    }
}

fn setup() -> (FakeServer, glide::client::ClientHandle, Arc<MockDesktop>) {
    let client_sock = localhost();
    let to = client_sock.local_addr().unwrap();
    let desk = Arc::new(MockDesktop::default());
    let client = start_client(client_sock, desk.clone(), None);
    let mut srv = FakeServer {
        sock: localhost(),
        to,
        cipher: crypto::session_cipher(SECRET, SESSION),
        counter: 0,
    };
    srv.heartbeat();
    wait_for("connected", || client.status().connected);
    (srv, client, desk)
}

#[test]
fn lost_motion_packets_are_made_up_for() {
    let (mut srv, client, desk) = setup();
    // Earlier visits already moved this PC's cursor: the tally doesn't start at 0.
    let mut tally = MotionTally {
        packets: 40,
        x: 500,
        y: -20,
    };
    srv.enter(Some(tally));
    wait_for("being controlled", || client.status().active);

    let mut sent = Vec::new();
    for i in 0..10 {
        tally.add(3, -1);
        sent.push(tally);
        // Every third packet is lost on the way.
        if i % 3 != 1 {
            srv.send(&ServerMsg::Motion {
                dx: 3,
                dy: -1,
                tally: Some(tally),
                sent_micros: None,
            });
        }
    }
    // One straggler turns up late: already counted, so it adds nothing.
    srv.send(&ServerMsg::Motion {
        dx: 3,
        dy: -1,
        tally: Some(sent[4]),
        sent_micros: None,
    });
    wait_for("all the movement", || desk.motion_sum() == (30, -10));
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(desk.motion_sum(), (30, -10), "exactly as far as the hand went");
    client.stop();
}

#[test]
fn movement_lost_before_the_switch_arrives_still_counts() {
    let (mut srv, client, desk) = setup();
    let start = MotionTally::default();
    srv.enter(Some(start));
    wait_for("being controlled", || client.status().active);

    // The first two packets of the visit are lost; the third gets through.
    let mut tally = start;
    for _ in 0..3 {
        tally.add(2, 2);
    }
    srv.send(&ServerMsg::Motion {
        dx: 2,
        dy: 2,
        tally: Some(tally),
        sent_micros: None,
    });
    wait_for("the whole movement", || desk.motion_sum() == (6, 6));
    client.stop();
}

#[test]
fn an_older_server_still_moves_the_cursor() {
    let (mut srv, client, desk) = setup();
    srv.enter(None);
    wait_for("being controlled", || client.status().active);
    for _ in 0..4 {
        srv.send(&ServerMsg::Motion {
            dx: -1,
            dy: 5,
            tally: None,
            sent_micros: None,
        });
    }
    wait_for("the movement", || desk.motion_sum() == (-4, 20));
    client.stop();
}

#[test]
fn smoothed_motion_all_arrives_and_lands_before_a_click() {
    let (mut srv, client, desk) = setup();
    let mut tally = MotionTally::default();
    srv.enter(Some(tally));
    wait_for("being controlled", || client.status().active);

    // Stamped as sent a millisecond apart but all arriving at once (a burst):
    // the client plays them out over time instead of in one jump.
    for i in 0..200u32 {
        tally.add(1, 0);
        srv.send(&ServerMsg::Motion {
            dx: 1,
            dy: 0,
            tally: Some(tally),
            sent_micros: Some(i * 1000),
        });
    }
    // A click right behind them: whatever is still held goes first.
    srv.send(&ServerMsg::Reliable {
        seq: 2,
        ev: ReliableEv::Input(glide::input::InputEvent::Button {
            button: glide::input::Button::Left,
            down: true,
        }),
    });
    wait_for("the click", || {
        desk.events()
            .iter()
            .any(|e| matches!(e, glide::input::InputEvent::Button { .. }))
    });
    let events = desk.events();
    let click = events
        .iter()
        .position(|e| matches!(e, glide::input::InputEvent::Button { .. }))
        .unwrap();
    let before: i32 = events[..click]
        .iter()
        .map(|e| match e {
            glide::input::InputEvent::Motion { dx, .. } => *dx,
            _ => 0,
        })
        .sum();
    assert_eq!(before, 200, "all the movement, before the click");
    client.stop();
}

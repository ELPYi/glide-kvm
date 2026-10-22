//! Pairing a new PC with a 6-digit code, through a real running server.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use glide::config::{ClientEntry, Config};
use glide::input::Side;
use glide::net::pairing;
use glide::platform::{Captured, EdgeHit};
use glide::server::Server;

fn empty_server(clients: Vec<ClientEntry>) -> (Arc<Server>, std::net::SocketAddr) {
    let cfg = Config {
        clients,
        ..Config::new_default_with_secret(SECRET)
    };
    let sock = localhost();
    let addr = sock.local_addr().unwrap();
    (
        Server::start(&cfg, Arc::new(MockDesktop::default()), sock).unwrap(),
        addr,
    )
}

#[test]
fn new_pc_pairs_and_is_immediately_usable() {
    let (server, server_addr) = empty_server(vec![]);
    let code = server.start_pairing(None).unwrap();

    // The new PC: pair on the socket it will then run on.
    let sock = localhost();
    let client_addr = sock.local_addr().unwrap();
    let got = pairing::pair_with(&sock, server_addr, &code, "laptop", Duration::from_secs(5)).unwrap();
    assert_eq!(got.secret, SECRET, "the real secret was transferred");

    let added = server.take_paired();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].name, "laptop");
    assert_eq!(added[0].side, Side::Right, "first free side");
    assert!(added[0].address.starts_with("127.0.0.1"));
    assert!(server.pairing_status().unwrap().done);
    assert!(server.take_paired().is_empty(), "reported once");

    // Run as a client with the received secret: it connects with no extra setup.
    let desk = Arc::new(MockDesktop::default());
    let client = start_client(sock, desk.clone(), None);
    wait_for("connected", || server.status().peers[0].connected);
    assert_eq!(server.status().peers[0].resolved, Some(client_addr));
    server.on_captured(Captured::Edge(EdgeHit {
        side: Side::Right,
        frac: 0.5,
    }));
    wait_for("in control", || client.status().active);
    server.stop();
}

#[test]
fn wrong_code_adds_nothing() {
    let (server, server_addr) = empty_server(vec![]);
    let code = server.start_pairing(None).unwrap();
    let wrong = format!("{:06}", (code.parse::<u32>().unwrap() + 7) % 1_000_000);
    let err = pairing::pair_with(&localhost(), server_addr, &wrong, "x", Duration::from_secs(5)).unwrap_err();
    assert_eq!(err.to_string(), "wrong code");
    assert!(server.take_paired().is_empty());
    let status = server.pairing_status().unwrap();
    assert!(!status.done);
    assert_eq!(status.code.replace(' ', ""), code, "one typo doesn't burn the code");
    server.stop();
}

#[test]
fn no_code_showing_means_no_pairing() {
    let (server, server_addr) = empty_server(vec![]);
    let err = pairing::pair_with(&localhost(), server_addr, "123456", "x", Duration::from_millis(500)).unwrap_err();
    assert!(err.to_string().contains("no answer"), "{err}");
    server.stop();
}

#[test]
fn refuses_when_every_side_is_taken() {
    let full = Side::ALL
        .iter()
        .map(|s| ClientEntry {
            name: format!("{s:?}"),
            address: "192.0.2.1".into(),
            side: *s,
            muted: false,
        })
        .collect();
    let (server, _) = empty_server(full);
    assert!(server.start_pairing(None).is_err());
    server.stop();
}

//! The "knock": a client that knows the server's address says hello first,
//! so its own firewall admits the server's replies (no inbound rule needed).

mod common;

use std::net::UdpSocket;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use glide::config::{ClientEntry, Config};
use glide::input::Side;
use glide::net::crypto;
use glide::net::protocol::{ClientMsg, ServerMsg};
use glide::server::Server;

/// A server whose only client is at `client_port`.
fn server_for(client_port: u16) -> (Arc<Server>, u16) {
    let cfg = Config {
        clients: vec![ClientEntry {
            name: "c".into(),
            address: format!("127.0.0.1:{client_port}"),
            side: Side::Right,
            muted: false,
            pc: None,
        }],
        ..Config::new_default_with_secret(SECRET)
    };
    let sock = localhost();
    let port = sock.local_addr().unwrap().port();
    (
        Server::start(&cfg, Arc::new(MockDesktop::default()), sock).unwrap(),
        port,
    )
}

#[test]
fn server_answers_a_knock_immediately() {
    let client = localhost();
    client.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let (server, server_port) = server_for(client.local_addr().unwrap().port());

    // Let DNS settle and drain the routine heartbeats.
    std::thread::sleep(Duration::from_millis(300));
    let mut buf = [0u8; 1500];
    while client.recv_from(&mut buf).is_ok() {}

    let knock_cipher = crypto::session_cipher(SECRET, crypto::KNOCK_SESSION);
    let pkt = crypto::seal(
        &knock_cipher,
        crypto::DIR_TO_SERVER,
        crypto::KNOCK_SESSION,
        1,
        &ClientMsg::Knock { name: None }.encode(),
    );
    let t0 = Instant::now();
    client.send_to(&pkt, ("127.0.0.1", server_port)).unwrap();

    // Generous wait, so a busy CI machine can't fail the read itself; the
    // timing is checked below.
    client.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let (n, _) = client.recv_from(&mut buf).expect("a reply to the knock");
    let elapsed = t0.elapsed();
    let h = crypto::parse_header(&buf[..n]).unwrap();
    let plain = crypto::open(&crypto::session_cipher(SECRET, h.session), &h, &buf[..n]).unwrap();
    assert!(matches!(ServerMsg::decode(&plain), Some(ServerMsg::Heartbeat { .. })));
    // Heartbeats go every 200 ms; a reply well inside that is the knock's.
    assert!(
        elapsed < Duration::from_millis(120),
        "answered in {elapsed:?}, not on the next heartbeat tick"
    );
    server.stop();
}

#[test]
fn forged_knock_is_ignored() {
    let client = localhost();
    client.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
    let (server, server_port) = server_for(client.local_addr().unwrap().port());
    std::thread::sleep(Duration::from_millis(300));
    let mut buf = [0u8; 1500];

    let wrong = crypto::session_cipher("not-the-real-secret-at-all", crypto::KNOCK_SESSION);
    let pkt = crypto::seal(
        &wrong,
        crypto::DIR_TO_SERVER,
        crypto::KNOCK_SESSION,
        1,
        &ClientMsg::Knock { name: None }.encode(),
    );
    while client.recv_from(&mut buf).is_ok() {}
    client.send_to(&pkt, ("127.0.0.1", server_port)).unwrap();
    // Only routine heartbeats (every 200 ms) may arrive; nothing within 20 ms.
    let early = client.recv_from(&mut buf).is_ok();
    assert!(!early, "server must not answer a knock it can't authenticate");
    server.stop();
}

#[test]
fn client_with_server_address_connects() {
    let client_sock = localhost();
    let client_port = client_sock.local_addr().unwrap().port();
    let (server, server_port) = server_for(client_port);
    let desk = Arc::new(MockDesktop::default());
    let client = start_client(client_sock, desk, Some(format!("127.0.0.1:{server_port}")));
    wait_for("connected", || {
        client.status().connected && server.status().peers[0].connected
    });
    client.stop();
    server.stop();
}

#[test]
fn ports_are_released_on_stop() {
    let sock = localhost();
    let port = sock.local_addr().unwrap().port();
    let (server, _) = server_for(1);
    let client = start_client(sock, Arc::new(MockDesktop::default()), None);
    client.stop();
    server.stop();
    UdpSocket::bind(("127.0.0.1", port)).expect("client port reusable after stop");
}

/// A server that knows its one client as `pc`, saved at an address it's no
/// longer at (it got a new one from DHCP, say).
fn server_with_stale_address(pc: &str) -> (Arc<Server>, u16) {
    let cfg = Config {
        clients: vec![ClientEntry {
            name: "My laptop".into(),
            address: "10.255.255.1".into(),
            side: Side::Right,
            muted: false,
            pc: Some(pc.into()),
        }],
        ..Config::new_default_with_secret(SECRET)
    };
    let sock = localhost();
    let port = sock.local_addr().unwrap().port();
    (
        Server::start(&cfg, Arc::new(MockDesktop::default()), sock).unwrap(),
        port,
    )
}

#[test]
fn a_paired_pc_at_a_new_address_reconnects_without_pairing() {
    let (server, server_port) = server_with_stale_address(&glide::net::addr::machine_name());
    let client_sock = localhost();
    let client_port = client_sock.local_addr().unwrap().port();
    let desk = Arc::new(MockDesktop::default());
    let client = start_client(client_sock, desk, Some(format!("127.0.0.1:{server_port}")));
    wait_for("connected at the new address", || {
        client.status().connected && server.status().peers[0].connected
    });
    // The new address is handed to the app to save, under the old one.
    let moved = server.take_moved();
    assert_eq!(moved.len(), 1, "{moved:?}");
    assert_eq!(moved[0].0, "10.255.255.1");
    assert_eq!(moved[0].1.address, format!("127.0.0.1:{client_port}"));
    assert_eq!(moved[0].1.name, "My laptop", "your name for it stays");
    client.stop();
    server.stop();
}

#[test]
fn a_stranger_at_a_new_address_is_not_taken_for_a_device() {
    let (server, server_port) = server_with_stale_address("WORK-LAPTOP");
    let sock = localhost();
    sock.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
    let knock_cipher = crypto::session_cipher(SECRET, crypto::KNOCK_SESSION);
    for (counter, name) in [(1, None), (2, Some("SOMEONE-ELSE".to_string()))] {
        let pkt = crypto::seal(
            &knock_cipher,
            crypto::DIR_TO_SERVER,
            crypto::KNOCK_SESSION,
            counter,
            &ClientMsg::Knock { name }.encode(),
        );
        sock.send_to(&pkt, ("127.0.0.1", server_port)).unwrap();
    }
    let mut buf = [0u8; 1500];
    assert!(sock.recv_from(&mut buf).is_err(), "no answer for an unknown PC");
    assert!(server.take_moved().is_empty());
    server.stop();
}

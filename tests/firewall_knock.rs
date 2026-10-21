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
        &ClientMsg::Knock.encode(),
    );
    let t0 = Instant::now();
    client.send_to(&pkt, ("127.0.0.1", server_port)).unwrap();

    let (n, _) = client.recv_from(&mut buf).expect("a reply to the knock");
    let elapsed = t0.elapsed();
    let h = crypto::parse_header(&buf[..n]).unwrap();
    let plain = crypto::open(&crypto::session_cipher(SECRET, h.session), &h, &buf[..n]).unwrap();
    assert!(matches!(ServerMsg::decode(&plain), Some(ServerMsg::Heartbeat { .. })));
    assert!(
        elapsed < Duration::from_millis(50),
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
        &ClientMsg::Knock.encode(),
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

//! The client the sign-in screen helper runs while no glide is running
//! (just restarted, nobody signed in): it connects like the normal one, and
//! gives the port back when stopped, so glide can take over.

mod common;

use std::net::UdpSocket;
use std::sync::Arc;

use common::*;
use glide::config::{ClientEntry, Config, Role};
use glide::engine::Engine;
use glide::input::Side;
use glide::server::Server;
use glide::status::EngineStatus;

fn free_port() -> u16 {
    UdpSocket::bind("0.0.0.0:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn connects_and_hands_the_port_back() {
    let client_port = free_port();
    let server_sock = localhost();
    let server_port = server_sock.local_addr().unwrap().port();
    let server_cfg = Config {
        clients: vec![ClientEntry {
            name: "laptop".into(),
            address: format!("127.0.0.1:{client_port}"),
            side: Side::Right,
            muted: false,
        }],
        ..Config::new_default_with_secret(SECRET)
    };
    let server = Server::start(&server_cfg, Arc::new(MockDesktop::default()), server_sock).unwrap();

    let client_cfg = Config {
        role: Some(Role::Client),
        port: client_port,
        server_address: Some(format!("127.0.0.1:{server_port}")),
        ..Config::new_default_with_secret(SECRET)
    };
    let engine = Engine::start_headless_client(&client_cfg).unwrap();
    wait_for(
        "the headless client to connect",
        || matches!(engine.status(), EngineStatus::Client(s) if s.connected),
    );
    if let EngineStatus::Client(s) = engine.status() {
        assert!(!s.sending_sound, "no sound from the sign-in screen");
    }

    engine.stop();
    assert!(
        UdpSocket::bind(("0.0.0.0", client_port)).is_ok(),
        "the port is free for glide again"
    );
    server.stop();
}

//! Shared test helpers: a mock desktop and a server+client pair on loopback.

#![allow(dead_code)] // each test file uses a different subset

use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glide::client::{self, ClientHandle, ClientOptions};
use glide::config::{ClientEntry, Config, SwitchSettings};
use glide::input::{InputBlock, InputEvent, PointerFeel, Side};
use glide::platform::{Desktop, EdgeHit};
use glide::server::Server;

pub const SECRET: &str = "an-adequately-long-test-secret";

/// Records what would have been injected / warped instead of touching the OS.
#[derive(Default)]
pub struct MockDesktop {
    pub injected: Mutex<Vec<(Instant, InputEvent)>>,
    pub warps: Mutex<Vec<(Side, f32)>>,
    /// What `edge_hit` reports (lets a test "push against an edge").
    pub edge: Mutex<Option<EdgeHit>>,
    /// Simulates a busy OS: every inject takes this long.
    pub inject_delay: Mutex<Duration>,
    /// What `pointer_feel` reports (this "PC's" mouse settings).
    pub feel: Mutex<Option<PointerFeel>>,
    /// Every `set_pointer_feel` call, in order.
    pub feel_calls: Mutex<Vec<Option<PointerFeel>>>,
    /// What `input_block` reports (simulates a lock screen / admin window).
    pub block: Mutex<Option<InputBlock>>,
    /// Every on-screen notice shown.
    pub notices: Mutex<Vec<String>>,
    /// Every `set_cursor_hidden` call, in order.
    pub cursor_hidden: Mutex<Vec<bool>>,
    /// What `cursor_position` reports (move it to simulate local mouse use).
    pub cursor: Mutex<Option<(i32, i32)>>,
    /// What `foreground` reports (the focused app).
    pub foreground: Mutex<Option<glide::game::Foreground>>,
}

impl MockDesktop {
    pub fn events(&self) -> Vec<InputEvent> {
        self.injected.lock().unwrap().iter().map(|e| e.1).collect()
    }
    pub fn clear(&self) {
        self.injected.lock().unwrap().clear();
    }
    /// Total injected motion.
    pub fn motion_sum(&self) -> (i32, i32) {
        self.events().iter().fold((0, 0), |(x, y), e| match e {
            InputEvent::Motion { dx, dy } => (x + dx, y + dy),
            _ => (x, y),
        })
    }
}

impl Desktop for MockDesktop {
    fn inject(&self, ev: &InputEvent) {
        let delay = *self.inject_delay.lock().unwrap();
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
        self.injected.lock().unwrap().push((Instant::now(), *ev));
    }
    fn warp_to_edge(&self, side: Side, frac: f32) {
        self.warps.lock().unwrap().push((side, frac));
    }
    fn edge_hit(&self, _dx: i32, _dy: i32) -> Option<EdgeHit> {
        *self.edge.lock().unwrap()
    }
    fn pointer_feel(&self) -> Option<PointerFeel> {
        *self.feel.lock().unwrap()
    }
    fn notify(&self, text: &str) {
        self.notices.lock().unwrap().push(text.to_string());
    }
    fn input_block(&self) -> Option<InputBlock> {
        *self.block.lock().unwrap()
    }
    fn set_pointer_feel(&self, feel: Option<PointerFeel>) {
        self.feel_calls.lock().unwrap().push(feel);
    }
    fn set_cursor_hidden(&self, hidden: bool) {
        self.cursor_hidden.lock().unwrap().push(hidden);
    }
    fn cursor_position(&self) -> Option<(i32, i32)> {
        *self.cursor.lock().unwrap()
    }
    fn foreground(&self) -> Option<glide::game::Foreground> {
        self.foreground.lock().unwrap().clone()
    }
}

pub fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::yield_now();
    }
}

pub struct Pair {
    pub server: Arc<Server>,
    pub server_desk: Arc<MockDesktop>,
    pub server_port: u16,
    pub client: Option<ClientHandle>,
    pub client_desk: Arc<MockDesktop>,
    pub client_port: u16,
}

impl Drop for Pair {
    fn drop(&mut self) {
        self.server.stop();
    }
}

pub fn localhost() -> UdpSocket {
    UdpSocket::bind("127.0.0.1:0").unwrap()
}

pub fn start_client(sock: UdpSocket, desk: Arc<MockDesktop>, server_address: Option<String>) -> ClientHandle {
    let opts = ClientOptions {
        secret: SECRET.into(),
        server_address,
        server_port: 0,
        games: Default::default(),
    };
    client::start(sock, desk, opts).unwrap()
}

/// Server with one client on its right, both on loopback, connected.
pub fn connected_pair(switching: SwitchSettings) -> Pair {
    let client_sock = localhost();
    let client_port = client_sock.local_addr().unwrap().port();
    let client_desk = Arc::new(MockDesktop::default());
    let client = start_client(client_sock, client_desk.clone(), None);

    let cfg = Config {
        clients: vec![ClientEntry {
            name: "test-client".into(),
            address: format!("127.0.0.1:{client_port}"),
            side: Side::Right,
            muted: false,
        }],
        switching,
        ..Config::new_default_with_secret(SECRET)
    };
    let server_desk = Arc::new(MockDesktop::default());
    let server_sock = localhost();
    let server_port = server_sock.local_addr().unwrap().port();
    let server = Server::start(&cfg, server_desk.clone(), server_sock).unwrap();
    wait_for("client to connect", || server.status().peers[0].connected);
    Pair {
        server,
        server_desk,
        server_port,
        client: Some(client),
        client_desk,
        client_port,
    }
}

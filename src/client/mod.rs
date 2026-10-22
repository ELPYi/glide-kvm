//! Client: receives input from the server and injects it locally.
//!
//! One thread, one socket, no locks on the hot path: packet in -> decrypt ->
//! SendInput out. The UI reads a status snapshot updated as things change.
//!
//! **Staying snappy under load.** Each wake-up drains *every* packet already
//! waiting and merges their motion into one injected move. If injecting is
//! slow for a moment (Windows is busy dragging a window, say), the next
//! wake-up catches up in one step instead of replaying a growing backlog
//! move by move, which is what turns a hiccup into seconds of lag.
//!
//! This PC's sound is sent by a separate thread (`crate::audio::share`),
//! through an [`AudioLink`]: the session details it needs, kept current by
//! this thread. It never touches this loop.
//!
//! If the server's address is configured, the client also *knocks* (sends a
//! small authenticated packet) until connected. Because the client spoke
//! first, its firewall treats the server's packets as replies and lets them
//! in, so no inbound firewall rule is needed.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::audio::Uplink;
use crate::game::{self, GameSettings};
use crate::input::{Button, InputBlock, InputEvent, Side};
use crate::log;
use crate::net::addr::resolve;
use crate::net::crypto::{self, Cipher, ReplayWindow};
use crate::net::protocol::{ClientMsg, ReliableEv, ServerMsg};
use crate::net::reliable::Inbox;
use crate::platform::{self, Desktop};
use crate::status::ClientStatus;

const SERVER_TIMEOUT: Duration = Duration::from_millis(1000);
const KNOCK_EVERY: Duration = Duration::from_millis(500);
const RESOLVE_EVERY: Duration = Duration::from_secs(5);
const INBOX_CAPACITY: usize = 4096;
/// Max packets merged per wake-up (bounds the time between injections).
const MAX_DRAIN: usize = 512;
/// While in control, keep polling the socket this long after the last
/// packet instead of sleeping. A sleeping thread takes 20-60 µs to wake;
/// polling picks the next mouse packet up within ~1 µs. Only runs while the
/// mouse is actually moving here, so it costs nothing at rest.
const HOT_POLL_FOR: Duration = Duration::from_millis(3);
/// How often to check whether this PC can accept input (lock screen, admin
/// window). Reported with every pong so the server never strands the cursor.
const BLOCK_CHECK_EVERY: Duration = Duration::from_millis(100);
/// Socket read timeout: how often the loop wakes when idle.
const POLL: Duration = Duration::from_millis(100);
/// How often to check for a game in focus while this PC is being controlled.
const GAME_CHECK_EVERY: Duration = Duration::from_millis(250);

pub struct ClientOptions {
    pub secret: String,
    /// Where to knock. `None` = wait to be contacted (needs a firewall rule).
    pub server_address: Option<String>,
    /// Port assumed when `server_address` doesn't name one.
    pub server_port: u16,
    /// A game in focus here keeps control here (the edge back doesn't switch).
    pub games: GameSettings,
}

/// What the sound sender needs to reach the server, kept current by the
/// client thread.
#[derive(Default)]
pub struct AudioLink {
    /// Where and how to send: `None` while not connected.
    target: Mutex<Option<AudioTarget>>,
    /// The server asked for sound (in its latest heartbeat).
    wanted: AtomicBool,
    /// Packet counter shared with the client thread, so the two never
    /// reuse a nonce. Never reset (a new session doesn't need it to be).
    counter: AtomicU64,
}

struct AudioTarget {
    session: u64,
    cipher: Cipher,
    server: SocketAddr,
}

/// The sound sender's way out: an [`Uplink`] over the client's socket.
pub struct AudioSender {
    link: Arc<AudioLink>,
    sock: UdpSocket,
}

impl Uplink for AudioSender {
    fn ready(&self) -> bool {
        self.link.wanted.load(Ordering::Relaxed) && self.link.target.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    fn send(&self, stream: u32, seq: u32, opus: &[u8]) {
        let guard = self.link.target.lock().unwrap_or_else(|e| e.into_inner());
        let Some(t) = guard.as_ref() else { return };
        let ctr = self.link.counter.fetch_add(1, Ordering::Relaxed) + 1;
        let msg = ClientMsg::Audio {
            stream,
            seq,
            opus: opus.to_vec(),
        };
        let pkt = crypto::seal(&t.cipher, crypto::DIR_TO_SERVER, t.session, ctr, &msg.encode());
        let server = t.server;
        drop(guard);
        let _ = self.sock.send_to(&pkt, server);
    }
}

/// A running client. Stops when [`ClientHandle::stop`] is called or dropped.
pub struct ClientHandle {
    status: Arc<Mutex<ClientStatus>>,
    games: Arc<Mutex<GameSettings>>,
    audio: Arc<AudioLink>,
    sock: UdpSocket,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ClientHandle {
    pub fn status(&self) -> ClientStatus {
        self.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    /// Change the game rules live.
    pub fn set_games(&self, games: GameSettings) {
        *self.games.lock().unwrap_or_else(|e| e.into_inner()) = games;
    }

    /// A way for the sound sender to reach the server.
    pub fn audio_sender(&self) -> Result<AudioSender> {
        Ok(AudioSender {
            link: self.audio.clone(),
            sock: self.sock.try_clone()?,
        })
    }

    fn shutdown(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for ClientHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn start(sock: UdpSocket, desktop: Arc<dyn Desktop>, opts: ClientOptions) -> Result<ClientHandle> {
    sock.set_read_timeout(Some(POLL))?;
    let status = Arc::new(Mutex::new(ClientStatus {
        port: sock.local_addr()?.port(),
        ..Default::default()
    }));
    let running = Arc::new(AtomicBool::new(true));
    let games = Arc::new(Mutex::new(opts.games.clone()));
    let audio = Arc::new(AudioLink::default());
    let handle_sock = sock.try_clone()?;
    let mut client = Client::new(sock, desktop, opts, status.clone(), games.clone(), audio.clone());
    let r = running.clone();
    let thread = std::thread::Builder::new().name("glide-client".into()).spawn(move || {
        platform::boost_latency_thread();
        client.run(&r);
    })?;
    Ok(ClientHandle {
        status,
        games,
        audio,
        sock: handle_sock,
        running,
        thread: Some(thread),
    })
}

struct Client {
    sock: UdpSocket,
    desktop: Arc<dyn Desktop>,
    opts: ClientOptions,
    status: Arc<Mutex<ClientStatus>>,
    instance: u64,

    session: u64,
    cipher: Option<Cipher>,
    knock_cipher: Cipher,
    replay: ReplayWindow,
    /// Shared with the sound sender (see [`AudioLink`]).
    audio: Arc<AudioLink>,
    server: Option<SocketAddr>,
    last_rx: Option<Instant>,

    knock_target: Option<SocketAddr>,
    last_resolve: Option<Instant>,
    last_knock: Option<Instant>,

    inbox: Inbox<ReliableEv>,
    /// Motion received but not yet injected (merged across a drain).
    pending_motion: (i32, i32),
    /// When mouse motion last arrived (drives hot polling).
    last_motion_rx: Option<Instant>,
    /// Latest "can't accept input" state, and when it was checked.
    block: Option<InputBlock>,
    block_checked: Option<Instant>,
    active: bool,
    /// The edge we entered through; pushing back out of it returns to the server.
    entry_side: Side,
    /// Where the cursor was hidden (control went back to the server). It's
    /// shown again as soon as it moves from there, i.e. this PC's own mouse
    /// or touchpad is used.
    hidden_at: Option<(i32, i32)>,
    games: Arc<Mutex<GameSettings>>,
    /// The game in focus here while being controlled, and when last checked.
    game: Option<String>,
    game_checked: Option<Instant>,
    keys_down: HashSet<u16>,
    buttons_down: HashSet<Button>,
}

impl Client {
    fn new(
        sock: UdpSocket,
        desktop: Arc<dyn Desktop>,
        opts: ClientOptions,
        status: Arc<Mutex<ClientStatus>>,
        games: Arc<Mutex<GameSettings>>,
        audio: Arc<AudioLink>,
    ) -> Self {
        Client {
            knock_cipher: crypto::session_cipher(&opts.secret, crypto::KNOCK_SESSION),
            sock,
            desktop,
            opts,
            status,
            instance: crate::now_micros(),
            session: 0,
            cipher: None,
            replay: ReplayWindow::default(),
            audio,
            server: None,
            last_rx: None,
            knock_target: None,
            last_resolve: None,
            last_knock: None,
            inbox: Inbox::new(INBOX_CAPACITY),
            pending_motion: (0, 0),
            last_motion_rx: None,
            block: None,
            block_checked: None,
            active: false,
            entry_side: Side::Left,
            hidden_at: None,
            games,
            game: None,
            game_checked: None,
            keys_down: HashSet::new(),
            buttons_down: HashSet::new(),
        }
    }

    fn connected(&self) -> bool {
        self.last_rx.is_some_and(|t| t.elapsed() < SERVER_TIMEOUT)
    }

    fn run(&mut self, running: &AtomicBool) {
        let mut buf = [0u8; 1500];
        let mut was_connected = false;
        while running.load(Ordering::Relaxed) {
            if self.active && self.last_motion_rx.is_some_and(|t| t.elapsed() < HOT_POLL_FOR) {
                // Mouse is moving here: poll instead of sleeping.
                if !self.drain(&mut buf) {
                    std::thread::yield_now();
                }
            } else {
                match self.sock.recv_from(&mut buf) {
                    Ok((n, src)) => {
                        self.handle_packet(&buf[..n], src);
                        self.drain(&mut buf);
                    }
                    Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
                    Err(_) => {} // e.g. ICMP unreachable echoes on Windows
                }
            }
            self.flush_motion();
            self.check_block();
            self.check_local_use();
            self.check_game();

            let connected = self.connected();
            if !connected {
                self.knock();
                if self.active {
                    log!("server went quiet; releasing everything");
                    self.deactivate();
                }
                self.show_cursor();
            }
            if connected != was_connected {
                was_connected = connected;
                self.sync_audio_link();
                if !connected {
                    log!("disconnected from server");
                }
                self.publish_status();
            }
        }
        self.deactivate();
        self.show_cursor();
        self.server = None;
        self.sync_audio_link();
        self.publish_status();
    }

    /// Tell the sound sender where the server is now (or that it's gone).
    fn sync_audio_link(&self) {
        let target = match (self.connected(), self.server, &self.cipher) {
            (true, Some(server), Some(cipher)) => Some(AudioTarget {
                session: self.session,
                cipher: cipher.clone(),
                server,
            }),
            _ => None,
        };
        *self.audio.target.lock().unwrap_or_else(|e| e.into_inner()) = target;
    }

    fn next_counter(&self) -> u64 {
        self.audio.counter.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Control went back to the server: hide the cursor here, so the only
    /// one you see is the one you're moving.
    fn hide_cursor(&mut self) {
        if let Some(pos) = self.desktop.cursor_position() {
            self.desktop.set_cursor_hidden(true);
            self.hidden_at = Some(pos);
        }
    }

    fn show_cursor(&mut self) {
        if self.hidden_at.take().is_some() {
            self.desktop.set_cursor_hidden(false);
        }
    }

    /// While being controlled, notice a game in focus here: the edge back to
    /// the sharing PC then doesn't switch.
    fn check_game(&mut self) {
        let game = if !self.active {
            None
        } else if self.game_checked.is_some_and(|t| t.elapsed() < GAME_CHECK_EVERY) {
            return;
        } else {
            self.game_checked = Some(Instant::now());
            let rules = self.games.lock().unwrap_or_else(|e| e.into_inner()).clone();
            rules.game_in_focus(self.desktop.foreground().as_ref())
        };
        if game != self.game {
            match &game {
                Some(g) => log!("{} is in focus: staying on this PC", game::display_name(g)),
                None if self.active => log!("no game in focus: edge switching is back"),
                None => {}
            }
            self.game = game;
            self.publish_status();
        }
    }

    /// Someone is using this PC's own mouse: show its cursor again.
    fn check_local_use(&mut self) {
        if self.hidden_at.is_some() && self.desktop.cursor_position() != self.hidden_at {
            self.show_cursor();
        }
    }

    /// Handle every packet that's already waiting, without blocking.
    /// Returns whether there were any.
    fn drain(&mut self, buf: &mut [u8]) -> bool {
        if self.sock.set_nonblocking(true).is_err() {
            return false;
        }
        let mut got = false;
        for _ in 0..MAX_DRAIN {
            match self.sock.recv_from(buf) {
                Ok((n, src)) => {
                    self.handle_packet(&buf[..n], src);
                    got = true;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(_) => {}
            }
        }
        let _ = self.sock.set_nonblocking(false);
        got
    }

    /// Re-check whether this PC can accept input. The server hears about it
    /// with the next pong (within ~25 ms) and takes control back if needed.
    fn check_block(&mut self) {
        if self.block_checked.is_some_and(|t| t.elapsed() < BLOCK_CHECK_EVERY) {
            return;
        }
        self.block_checked = Some(Instant::now());
        let block = self.desktop.input_block();
        if block != self.block {
            match block {
                Some(b) => log!("can't accept input: {}", b.label().to_lowercase()),
                None => log!("can accept input again"),
            }
            self.block = block;
        }
    }

    /// Back to idle: release everything and restore this PC's pointer settings.
    fn deactivate(&mut self) {
        self.pending_motion = (0, 0);
        self.release_all();
        if self.active {
            self.desktop.set_pointer_feel(None);
        }
        self.active = false;
    }

    fn publish_status(&self) {
        let mut s = self.status.lock().unwrap_or_else(|e| e.into_inner());
        s.server = self.server;
        s.connected = self.connected();
        s.active = self.active;
        s.game = self.game.clone();
    }

    /// Say hello to the configured server (see module docs for why).
    fn knock(&mut self) {
        let Some(address) = self.opts.server_address.as_deref().filter(|a| !a.trim().is_empty()) else {
            return;
        };
        if self.knock_target.is_none() && self.last_resolve.is_none_or(|t| t.elapsed() >= RESOLVE_EVERY) {
            self.last_resolve = Some(Instant::now());
            self.knock_target = resolve(address, self.opts.server_port);
            if self.knock_target.is_none() {
                log!("couldn't find the server at {address}");
            }
        }
        let Some(target) = self.knock_target else { return };
        if self.last_knock.is_none_or(|t| t.elapsed() >= KNOCK_EVERY) {
            self.last_knock = Some(Instant::now());
            let pkt = crypto::seal(
                &self.knock_cipher,
                crypto::DIR_TO_SERVER,
                crypto::KNOCK_SESSION,
                self.next_counter(),
                &ClientMsg::Knock.encode(),
            );
            let _ = self.sock.send_to(&pkt, target);
        }
    }

    fn handle_packet(&mut self, pkt: &[u8], src: SocketAddr) {
        let Some(h) = crypto::parse_header(pkt) else { return };
        if h.dir != crypto::DIR_TO_CLIENT || h.session < self.session || h.session == crypto::KNOCK_SESSION {
            return;
        }
        let adopted = h.session != self.session;
        let plain = if !adopted {
            match self.cipher.as_ref().and_then(|c| crypto::open(c, &h, pkt)) {
                Some(p) => p,
                None => return,
            }
        } else {
            // A newer server session. Adopt it only once a packet authenticates.
            let cipher = crypto::session_cipher(&self.opts.secret, h.session);
            let Some(p) = crypto::open(&cipher, &h, pkt) else {
                return;
            };
            self.adopt_session(h.session, cipher, src);
            p
        };
        if !self.replay.accept(h.counter) {
            return;
        }
        let Some(msg) = ServerMsg::decode(&plain) else { return };
        let moved = self.server != Some(src);
        if moved && platform::prioritize_traffic(&self.sock, src) {
            log!("network priority on");
        }
        self.server = Some(src);
        let reconnected = !self.connected();
        self.last_rx = Some(Instant::now());
        if reconnected || moved || adopted {
            self.sync_audio_link();
        }
        if reconnected {
            self.publish_status();
        }

        match msg {
            ServerMsg::Heartbeat {
                sent_micros,
                wants_sound,
            } => {
                self.audio.wanted.store(wants_sound, Ordering::Relaxed);
                self.send(&ClientMsg::Pong {
                    instance: self.instance,
                    echo_micros: sent_micros,
                    blocked: self.block,
                });
            }
            ServerMsg::Motion { dx, dy } => self.on_motion(dx, dy),
            ServerMsg::Reliable { seq, ev } => {
                // Motion first, so a click lands where the cursor actually is.
                self.flush_motion();
                for ev in self.inbox.accept(seq, ev) {
                    self.apply(ev);
                }
                self.send(&ClientMsg::Ack {
                    seq: self.inbox.ack_point(),
                });
            }
        }
    }

    fn adopt_session(&mut self, session: u64, cipher: Cipher, src: SocketAddr) {
        if self.session != 0 {
            log!("server restarted");
        }
        log!("connected to server {src}");
        self.deactivate();
        self.session = session;
        self.cipher = Some(cipher);
        self.replay = ReplayWindow::default();
        self.inbox.reset();
    }

    fn on_motion(&mut self, dx: i32, dy: i32) {
        if self.active {
            self.last_motion_rx = Some(Instant::now());
            self.pending_motion.0 = self.pending_motion.0.saturating_add(dx);
            self.pending_motion.1 = self.pending_motion.1.saturating_add(dy);
        }
    }

    /// Inject the merged motion (or hand control back at the entry edge).
    fn flush_motion(&mut self) {
        let (dx, dy) = std::mem::take(&mut self.pending_motion);
        if !self.active || (dx == 0 && dy == 0) {
            return;
        }
        if let Some(hit) = self.desktop.edge_hit(dx, dy) {
            // Mid-game, the edge is just the edge of the game.
            if hit.side == self.entry_side && self.game.is_none() {
                self.send(&ClientMsg::EdgeLeave {
                    side: hit.side,
                    frac: hit.frac,
                });
                return;
            }
        }
        self.desktop.inject(&InputEvent::Motion { dx, dy });
    }

    fn apply(&mut self, ev: ReliableEv) {
        match ev {
            ReliableEv::Enter { side, frac, feel } => {
                self.release_all();
                self.active = true;
                self.entry_side = side;
                // Feel exactly like the sharing PC's mouse (or keep our own).
                self.desktop.set_pointer_feel(feel);
                self.desktop.warp_to_edge(side, frac);
                self.show_cursor();
                log!("you're on this machine");
                self.publish_status();
            }
            ReliableEv::Leave => {
                self.deactivate();
                self.hide_cursor();
                log!("back to the server");
                self.publish_status();
            }
            ReliableEv::Notice(n) => {
                if self.active {
                    self.desktop.notify(n.text());
                }
            }
            ReliableEv::Input(ev) => {
                if !self.active {
                    return;
                }
                match ev {
                    InputEvent::Key { code, down: true } => {
                        self.keys_down.insert(code);
                    }
                    InputEvent::Key { code, down: false } => {
                        self.keys_down.remove(&code);
                    }
                    InputEvent::Button { button, down: true } => {
                        self.buttons_down.insert(button);
                    }
                    InputEvent::Button { button, down: false } => {
                        self.buttons_down.remove(&button);
                    }
                    _ => {}
                }
                self.desktop.inject(&ev);
            }
        }
    }

    /// Never leave a key or button stuck down on this machine.
    fn release_all(&mut self) {
        for code in std::mem::take(&mut self.keys_down) {
            self.desktop.inject(&InputEvent::Key { code, down: false });
        }
        for button in std::mem::take(&mut self.buttons_down) {
            self.desktop.inject(&InputEvent::Button { button, down: false });
        }
    }

    fn send(&mut self, msg: &ClientMsg) {
        let (Some(addr), Some(cipher)) = (self.server, self.cipher.as_ref()) else {
            return;
        };
        let ctr = self.next_counter();
        let pkt = crypto::seal(cipher, crypto::DIR_TO_SERVER, self.session, ctr, &msg.encode());
        let _ = self.sock.send_to(&pkt, addr);
    }
}

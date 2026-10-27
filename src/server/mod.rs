//! Server: the machine with the physical keyboard and mouse.
//!
//! [`Server`] owns the socket and the per-client [`peer::Peer`] state, and
//! carries out what the pure [`switcher::Switcher`] decides.
//!
//! Threads:
//! - capture (owned by the engine): OS hooks call [`Server::on_captured`] for
//!   every input event. Keys/clicks are sealed and sent right there; motion
//!   is summed and paced (see [`pacer`]).
//! - `glide-srv-pacer`: sends motion that arrived too soon after the last send.
//! - `glide-srv-recv`: pongs, acks, knocks, edge-leaves and pairing requests.
//! - `glide-srv-timer`: heartbeats, retransmits, liveness, pairing upkeep.
//! - `glide-srv-dns`: resolving client addresses, kept off the hot path.
//! - `glide-srv-game`: notices a game in focus (edges then don't switch).
//! - sound from the other PCs is played by an [`audio::play::Player`]
//!   (its own thread); `glide-srv-recv` only hands packets over.

pub mod pacer;
pub mod peer;
pub mod switcher;

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};

use crate::audio::play::Player;
use crate::audio::AudioSettings;
use crate::config::{ClientEntry, Config, SwitchSettings};
use crate::game::{self, GameSettings};
use crate::hotkey::{Hotkey, Hotkeys};
use crate::input::Side;
use crate::log;
use crate::net::addr::resolve;
use crate::net::crypto::{self, Cipher};
use crate::net::pairing::{self, ServerPairing};
use crate::net::protocol::{ClientMsg, Notice, ReliableEv, ServerMsg};
use crate::platform::{self, Captured, Desktop};
use crate::status::{PairingStatus, ServerStatus};
use pacer::{MotionBuf, FAST_INTERVAL, MOTION_INTERVAL};
use peer::Peer;
use switcher::{Decision, Forward, Switch, Switcher};

/// Heartbeats to devices that aren't answering yet (discovery).
const HEARTBEAT_EVERY: Duration = Duration::from_millis(200);
/// Heartbeats to connected devices. Frequent enough that neither PC's Wi-Fi
/// radio dozes off (power saving adds 10-100 ms to the next packet), and it
/// gives a steady stream of round-trip samples. ~40 tiny packets a second.
const KEEP_WARM_EVERY: Duration = Duration::from_millis(25);
const RETRANSMIT_AFTER: Duration = Duration::from_millis(30);
const RESOLVE_EVERY: Duration = Duration::from_secs(5);
/// The timer never sleeps longer than this (liveness checks, new devices).
const IDLE_TICK: Duration = Duration::from_millis(250);
const PAIRING_TICK: Duration = Duration::from_millis(100);
const MIN_TICK: Duration = Duration::from_millis(1);
/// Waits longer than this use the ordinary (coarse, cheaper) OS sleep.
const COARSE_SLEEP_OK: Duration = Duration::from_millis(20);
const POKE_EVERY: Duration = Duration::from_millis(500);
/// How often to check whether a game is in focus.
pub const GAME_CHECK_EVERY: Duration = Duration::from_millis(250);
/// How often blocked threads wake to notice a stop request.
const POLL: Duration = Duration::from_millis(100);
/// Order in which pairing fills free sides.
const SIDE_PREFERENCE: [Side; 4] = [Side::Right, Side::Left, Side::Bottom, Side::Top];

struct State {
    peers: Vec<Peer>,
    switcher: Switcher,
    motion: MotionBuf,
    /// The game in focus here, if any.
    game: Option<String>,
}

/// A pairing code being shown on this PC.
struct PairingSession {
    machine: ServerPairing,
    /// Optional "other PC's address": we send to it so our firewall lets its reply in.
    poke: Option<SocketAddr>,
    last_poke: Option<Instant>,
}

pub struct Server {
    sock: UdpSocket,
    secret: String,
    cipher: Cipher,
    knock_cipher: Cipher,
    session: u64,
    counter: AtomicU64,
    /// Port we're bound to (shown in the UI).
    port: u16,
    /// Port assumed for client addresses that don't name one.
    client_port: u16,
    state: Mutex<State>,
    /// Wakes the pacer thread when motion is waiting.
    pacer_wake: Condvar,
    pairing: Mutex<Option<PairingSession>>,
    /// Devices added by pairing, waiting for the app to save them.
    newly_paired: Mutex<Vec<ClientEntry>>,
    /// Result of "record a shortcut", waiting for the app (None inside = cancelled).
    recorded_hotkey: Mutex<Option<Option<Hotkey>>>,
    /// Shared with capture: while set, mouse motion is swallowed locally.
    grab: Arc<AtomicBool>,
    games: Mutex<GameSettings>,
    /// Plays the other PCs' sound (set by the engine; tests run without).
    player: Mutex<Option<Player>>,
    desktop: Arc<dyn Desktop>,
    running: AtomicBool,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl Server {
    /// Starts the network threads. Hook capture up to [`Server::on_captured`]
    /// separately (the engine does this), which keeps the server testable.
    pub fn start(cfg: &Config, desktop: Arc<dyn Desktop>, sock: UdpSocket) -> Result<Arc<Server>> {
        sock.set_read_timeout(Some(POLL))?;
        let port = sock.local_addr()?.port();
        // Microseconds since the epoch: a fresh, ever-increasing id per run.
        let session = crate::now_micros();
        let srv = Arc::new(Server {
            sock,
            secret: cfg.secret.clone(),
            cipher: crypto::session_cipher(&cfg.secret, session),
            knock_cipher: crypto::session_cipher(&cfg.secret, crypto::KNOCK_SESSION),
            session,
            counter: AtomicU64::new(0),
            port,
            client_port: cfg.port,
            state: Mutex::new(State {
                peers: cfg.clients.iter().cloned().map(Peer::new).collect(),
                switcher: Switcher::new(cfg.switching, cfg.hotkeys),
                motion: MotionBuf::default(),
                game: None,
            }),
            pacer_wake: Condvar::new(),
            pairing: Mutex::new(None),
            newly_paired: Mutex::new(Vec::new()),
            recorded_hotkey: Mutex::new(None),
            grab: Arc::new(AtomicBool::new(false)),
            games: Mutex::new(cfg.games.clone()),
            player: Mutex::new(None),
            desktop,
            running: AtomicBool::new(true),
            threads: Mutex::new(Vec::new()),
        });

        let spawn = |name: &str, f: fn(&Server)| {
            let s = srv.clone();
            std::thread::Builder::new().name(name.into()).spawn(move || f(&s))
        };
        let threads = vec![
            spawn("glide-srv-pacer", Server::pacer_loop)?,
            spawn("glide-srv-recv", Server::recv_loop)?,
            spawn("glide-srv-timer", Server::timer_loop)?,
            spawn("glide-srv-dns", Server::resolve_loop)?,
            spawn("glide-srv-game", Server::game_loop)?,
        ];
        *srv.threads.lock().unwrap() = threads;
        Ok(srv)
    }

    /// Stops the threads and hands input back to this machine.
    pub fn stop(&self) {
        *self.player() = None;
        {
            let mut st = self.lock();
            if let Some(switch) = st.switcher.force_leave() {
                self.apply_switch(&mut st, switch);
            }
        }
        self.running.store(false, Ordering::Relaxed);
        self.pacer_wake.notify_all();
        for t in self.threads.lock().unwrap().drain(..) {
            let _ = t.join();
        }
    }

    pub fn grab_flag(&self) -> Arc<AtomicBool> {
        self.grab.clone()
    }

    pub fn status(&self) -> ServerStatus {
        let st = self.lock();
        let mut peers: Vec<_> = st.peers.iter().map(Peer::status).collect();
        if let Some(player) = self.player().as_ref() {
            for (p, s) in st.peers.iter().zip(peers.iter_mut()) {
                s.sound = s.connected && p.addr.is_some_and(|a| player.playing_from(a));
            }
        }
        ServerStatus {
            port: self.port,
            peers,
            active: st.switcher.active(),
            locked: st.switcher.locked(),
            game: st.game.clone(),
            pairing: self.pairing_status(),
            recording_hotkey: st.switcher.recording(),
        }
    }

    pub fn set_locked(&self, locked: bool) {
        let mut st = self.lock();
        if st.switcher.locked() != locked {
            st.switcher.set_locked(locked);
            self.announce_lock(&mut st, locked);
        }
    }

    /// Confirm a lock change on screen, on the PC you're actually using
    /// (shortcuts otherwise act invisibly).
    fn announce_lock(&self, st: &mut State, locked: bool) {
        log!("edge switching {}", if locked { "locked" } else { "unlocked" });
        let notice = if locked { Notice::Locked } else { Notice::Unlocked };
        match st.switcher.active() {
            Some(i) => self.send_reliable(&mut st.peers[i], ReliableEv::Notice(notice)),
            None => self.desktop.notify(notice.text()),
        }
    }

    pub fn set_switching(&self, settings: SwitchSettings) {
        let mut st = self.lock();
        st.switcher.set_settings(settings);
        if let Some(i) = st.switcher.active() {
            let interval = motion_interval(settings.mouse_rate_hz, st.peers[i].fast_link());
            st.motion.set_interval(interval);
        }
    }

    /// Change the game rules live.
    pub fn set_games(&self, games: GameSettings) {
        *self.games.lock().unwrap() = games;
    }

    /// Play the other PCs' sound with `player` (`None`: don't ask for it).
    pub fn set_player(&self, player: Option<Player>) {
        *self.player() = player;
    }

    /// Change sound settings live (play on/off, volume).
    pub fn set_audio(&self, settings: AudioSettings) {
        if let Some(p) = self.player().as_ref() {
            p.set(settings);
        }
    }

    /// Mute (or unmute) device `index`'s sound here.
    pub fn set_device_muted(&self, index: usize, muted: bool) {
        let mut st = self.lock();
        if let Some(p) = st.peers.get_mut(index) {
            p.entry.muted = muted;
        }
        self.sync_muted(&st);
    }

    /// Tell the player which devices are muted, by their current address.
    fn sync_muted(&self, st: &State) {
        if let Some(player) = self.player().as_ref() {
            let ips = st
                .peers
                .iter()
                .filter(|p| p.entry.muted)
                .filter_map(|p| p.addr.map(|a| a.ip()))
                .collect();
            player.set_muted(ips);
        }
    }

    fn player(&self) -> MutexGuard<'_, Option<Player>> {
        self.player.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn wants_sound(&self) -> bool {
        self.player().as_ref().is_some_and(Player::wants_sound)
    }

    pub fn set_hotkeys(&self, hotkeys: Hotkeys) {
        self.lock().switcher.set_hotkeys(hotkeys);
    }

    /// Start (or cancel) capturing the next key combination as a shortcut.
    pub fn record_hotkey(&self, on: bool) {
        self.lock().switcher.record_hotkey(on);
        *self.recorded_hotkey.lock().unwrap() = None;
    }

    /// The recorded combination once there is one: `Some(Some(key))`, or
    /// `Some(None)` if cancelled with Esc.
    pub fn take_recorded_hotkey(&self) -> Option<Option<Hotkey>> {
        self.recorded_hotkey.lock().unwrap().take()
    }

    /// Re-arrange devices live: `sides[i]` is device `i`'s new side.
    pub fn set_layout(&self, sides: &[Side]) -> Result<()> {
        let mut st = self.lock();
        if sides.len() != st.peers.len() {
            bail!("layout has {} sides for {} devices", sides.len(), st.peers.len());
        }
        if (1..sides.len()).any(|i| sides[..i].contains(&sides[i])) {
            bail!("two devices on the same side");
        }
        for (p, side) in st.peers.iter_mut().zip(sides) {
            p.entry.side = *side;
        }
        Ok(())
    }

    // ------------------------------------------------------------ pairing

    /// Show a pairing code. `poke`: the new PC's address, if pairing can't get
    /// through this PC's firewall otherwise. Returns the code.
    pub fn start_pairing(&self, poke: Option<&str>) -> Result<String> {
        if free_side(&self.lock().peers).is_none() {
            bail!("all four sides already have a device; remove one first");
        }
        let poke = poke
            .filter(|a| !a.trim().is_empty())
            .and_then(|a| resolve(a, self.client_port));
        let machine = ServerPairing::new(&self.secret, self.client_port, Instant::now());
        let code = machine.code().to_string();
        *self.pairing.lock().unwrap() = Some(PairingSession {
            machine,
            poke,
            last_poke: None,
        });
        log!("pairing code {} is ready", pairing::pretty_code(&code));
        Ok(code)
    }

    pub fn cancel_pairing(&self) {
        *self.pairing.lock().unwrap() = None;
    }

    pub fn pairing_status(&self) -> Option<PairingStatus> {
        let now = Instant::now();
        self.pairing.lock().unwrap().as_ref().map(|s| PairingStatus {
            code: pairing::pretty_code(s.machine.code()),
            expires_in: s.machine.expires_in(now),
            failed_attempts: s.machine.failed_attempts(),
            checking: s.machine.is_checking(),
            done: s.machine.is_done(),
        })
    }

    /// Devices paired since the last call (for the app to save).
    pub fn take_paired(&self) -> Vec<ClientEntry> {
        std::mem::take(&mut *self.newly_paired.lock().unwrap())
    }

    fn handle_pairing(&self, pkt: &[u8], src: SocketAddr) {
        let step = {
            let mut guard = self.pairing.lock().unwrap();
            let Some(session) = guard.as_mut() else { return };
            session.machine.handle(pkt, src, Instant::now())
        };
        // Record the device before acknowledging, so it exists by the time the new PC hears back.
        if let Some(name) = step.paired {
            self.add_paired(name, src);
        }
        if let Some(reply) = step.reply {
            let _ = self.sock.send_to(&reply, src);
        }
    }

    /// A PC proved it knew the code: add it (or refresh it if it's re-pairing).
    fn add_paired(&self, name: String, src: SocketAddr) {
        let mut st = self.lock();
        if let Some(p) = st.peers.iter_mut().find(|p| p.addr.is_some_and(|a| a.ip() == src.ip())) {
            p.addr = Some(src);
            log!("{} paired again", p.entry.name);
            return;
        }
        let Some(side) = free_side(&st.peers) else {
            log!("{name} paired, but every side is taken");
            return;
        };
        let address = if src.port() == self.client_port {
            src.ip().to_string()
        } else {
            src.to_string()
        };
        let entry = ClientEntry {
            name,
            address,
            side,
            muted: false,
        };
        log!(
            "paired with {} ({}), placed {}",
            entry.name,
            entry.address,
            side.label().to_lowercase()
        );
        let mut peer = Peer::new(entry.clone());
        peer.addr = Some(src);
        st.peers.push(peer);
        self.newly_paired.lock().unwrap().push(entry);
    }

    // ------------------------------------------------------------ input path

    /// Runs inside the OS input hook. Returns true to swallow the event locally.
    pub fn on_captured(&self, ev: Captured) -> bool {
        let mut st = self.lock();
        let decision = {
            let State { peers, switcher, .. } = &mut *st;
            // Never cross into a PC that can't accept input (limbo).
            let target = |side: Side| {
                peers
                    .iter()
                    .position(|p| p.entry.side == side && p.alive() && p.blocked.is_none())
            };
            switcher.handle(ev, Instant::now(), target)
        };
        self.apply(&mut st, decision)
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn apply(&self, st: &mut State, d: Decision) -> bool {
        if let Some(locked) = d.lock_changed {
            self.announce_lock(st, locked);
        }
        if let Some(recorded) = d.recorded {
            *self.recorded_hotkey.lock().unwrap() = Some(recorded);
        }
        if let Some(switch) = d.switch {
            self.apply_switch(st, switch);
        }
        for (to, ev) in d.before {
            match to {
                None => self.desktop.inject(&ev),
                Some(i) => self.send_input(st, i, ev),
            }
        }
        match d.forward {
            Some(Forward::Motion { dx, dy }) => {
                let now = Instant::now();
                st.motion.add(dx, dy);
                if st.motion.ready(now) {
                    self.flush_motion(st, now);
                } else {
                    self.pacer_wake.notify_one();
                }
            }
            Some(Forward::Reliable(i, ev)) => self.send_input(st, i, ev),
            None => {}
        }
        d.block
    }

    fn send_input(&self, st: &mut State, peer: usize, ev: crate::input::InputEvent) {
        if st.switcher.active() == Some(peer) {
            // Motion first, so a click lands where the cursor actually is.
            self.flush_motion(st, Instant::now());
        }
        if let Some(p) = st.peers.get_mut(peer) {
            self.send_reliable(p, ReliableEv::Input(ev));
        }
    }

    fn flush_motion(&self, st: &mut State, now: Instant) {
        let addr = st.switcher.active().and_then(|i| st.peers[i].addr);
        if let (Some((dx, dy)), Some(addr)) = (st.motion.take(now), addr) {
            self.send(addr, &ServerMsg::Motion { dx, dy });
        }
    }

    fn apply_switch(&self, st: &mut State, switch: Switch) {
        st.motion.clear();
        match switch {
            Switch::Enter { peer, frac } => {
                self.grab.store(true, Ordering::Relaxed);
                let interval = motion_interval(st.switcher.settings().mouse_rate_hz, st.peers[peer].fast_link());
                st.motion.set_interval(interval);
                let feel = st
                    .switcher
                    .settings()
                    .match_pointer_feel
                    .then(|| self.desktop.pointer_feel())
                    .flatten();
                let p = &mut st.peers[peer];
                let side = p.entry.side.opposite();
                self.send_reliable(p, ReliableEv::Enter { side, frac, feel });
                // The cursor is "over there" now, so don't leave it parked here.
                self.desktop.set_cursor_hidden(true);
                log!("-> {}", p.entry.name);
            }
            Switch::Leave { peer, warp } => {
                let p = &mut st.peers[peer];
                self.send_reliable(p, ReliableEv::Leave);
                if let Some(frac) = warp {
                    self.desktop.warp_to_edge(p.entry.side, frac);
                }
                self.desktop.set_cursor_hidden(false);
                self.grab.store(false, Ordering::Relaxed);
                log!("<- back from {}", p.entry.name);
            }
        }
    }

    fn send(&self, addr: SocketAddr, msg: &ServerMsg) {
        let ctr = self.counter.fetch_add(1, Ordering::Relaxed);
        let pkt = crypto::seal(&self.cipher, crypto::DIR_TO_CLIENT, self.session, ctr, &msg.encode());
        let _ = self.sock.send_to(&pkt, addr);
    }

    fn send_reliable(&self, peer: &mut Peer, ev: ReliableEv) {
        let seq = peer.outbox.push(ev.clone(), Instant::now());
        if let Some(addr) = peer.addr {
            self.send(addr, &ServerMsg::Reliable { seq, ev });
        }
    }

    // ------------------------------------------------------------ threads

    fn pacer_loop(&self) {
        platform::boost_latency_thread();
        while self.running.load(Ordering::Relaxed) {
            let st = self.lock();
            if !st.motion.pending() {
                let _ = self.pacer_wake.wait_timeout(st, POLL);
                continue;
            }
            let wait = st
                .motion
                .due_at()
                .map(|t| t.saturating_duration_since(Instant::now()))
                .unwrap_or_default();
            drop(st);
            // A high-resolution timer, not a spin: a packet a fraction of
            // a millisecond later is lost in the network's own jitter.
            platform::precise_sleep(wait);
            let mut st = self.lock();
            self.flush_motion(&mut st, Instant::now());
        }
    }

    fn recv_loop(&self) {
        platform::boost_latency_thread();
        let mut buf = [0u8; 1500];
        while self.running.load(Ordering::Relaxed) {
            // Errors are timeouts (stop polling), or Windows reporting ICMP
            // "port unreachable" from a send to an offline client. Harmless.
            let Ok((n, src)) = self.sock.recv_from(&mut buf) else {
                continue;
            };
            if buf[0] == pairing::MAGIC {
                self.handle_pairing(&buf[..n], src);
            } else {
                self.handle_packet(&buf[..n], src);
            }
        }
    }

    fn handle_packet(&self, pkt: &[u8], src: SocketAddr) {
        let Some(h) = crypto::parse_header(pkt) else { return };
        if h.dir != crypto::DIR_TO_SERVER {
            return;
        }
        if h.session == crypto::KNOCK_SESSION {
            self.handle_knock(&h, pkt, src);
            return;
        }
        if h.session != self.session {
            return;
        }
        let Some(plain) = crypto::open(&self.cipher, &h, pkt) else {
            return;
        };
        let Some(msg) = ClientMsg::decode(&plain) else { return };
        // Sound goes straight to the player, without taking the state lock
        // the input hook uses. (It's authenticated; the player's per-sender
        // sequence numbers drop replays and duplicates.)
        if let ClientMsg::Audio { stream, seq, opus } = &msg {
            if let Some(p) = self.player().as_ref() {
                p.push(src, *stream, *seq, opus);
            }
            return;
        }

        let mut st = self.lock();
        let Some(i) = st.peers.iter().position(|p| p.addr.is_some_and(|a| a.ip() == src.ip())) else {
            return;
        };
        if let ClientMsg::Pong { instance, .. } = msg {
            if instance != st.peers[i].instance {
                st.peers[i].reset_session(instance);
                if st.switcher.active() == Some(i) {
                    if let Some(switch) = st.switcher.force_leave() {
                        self.apply_switch(&mut st, switch);
                    }
                }
            }
        }
        if !st.peers[i].replay.accept(h.counter) {
            return;
        }
        st.peers[i].addr = Some(src);
        match msg {
            ClientMsg::Pong {
                echo_micros, blocked, ..
            } => {
                let p = &mut st.peers[i];
                if blocked != p.blocked {
                    match blocked {
                        Some(b) => log!("{} can't take input: {}", p.entry.name, b.label().to_lowercase()),
                        None => log!("{} can take input again", p.entry.name),
                    }
                }
                p.blocked = blocked;
                p.last_pong = Some(Instant::now());
                // In control of a PC that just became unable to accept input?
                // Take it back right away instead of leaving the cursor in limbo.
                if blocked.is_some() && st.switcher.active() == Some(i) {
                    if let Some(switch) = st.switcher.force_leave() {
                        self.apply_switch(&mut st, switch);
                    }
                }
                let p = &mut st.peers[i];
                let rtt_us = crate::now_micros().saturating_sub(echo_micros);
                p.record_rtt(rtt_us as f32 / 1000.0);
            }
            ClientMsg::Ack { seq } => st.peers[i].outbox.ack(seq),
            ClientMsg::EdgeLeave { side, frac } => {
                let facing_us = side == st.peers[i].entry.side.opposite();
                if let Some(switch) = st.switcher.client_left(i, facing_us, frac, Instant::now()) {
                    self.apply_switch(&mut st, switch);
                }
            }
            ClientMsg::Knock | ClientMsg::Audio { .. } => {}
        }
    }

    /// A client saying hello before it knows our session. Answer at once
    /// (no waiting for the next heartbeat) to the address it came from.
    fn handle_knock(&self, h: &crypto::Header, pkt: &[u8], src: SocketAddr) {
        if crypto::open(&self.knock_cipher, h, pkt).is_none() {
            return;
        }
        let wants_sound = self.wants_sound();
        let mut st = self.lock();
        let Some(p) = st.peers.iter_mut().find(|p| p.addr.is_some_and(|a| a.ip() == src.ip())) else {
            return;
        };
        p.addr = Some(src);
        p.last_heartbeat = Some(Instant::now());
        self.send(
            src,
            &ServerMsg::Heartbeat {
                sent_micros: crate::now_micros(),
                wants_sound,
            },
        );
    }

    /// Sleeps until the next job is due (a heartbeat, a retransmission, a
    /// liveness check) instead of waking on a fixed fast tick: an idle glide
    /// wakes a few times a second, not hundreds.
    fn timer_loop(&self) {
        while self.running.load(Ordering::Relaxed) {
            let next = self.timer_tick(Instant::now());
            let wait = next.saturating_duration_since(Instant::now()).max(MIN_TICK);
            if wait < COARSE_SLEEP_OK {
                platform::precise_sleep(wait);
            } else {
                std::thread::sleep(wait);
            }
        }
    }

    /// Does whatever is due; returns when to run again.
    fn timer_tick(&self, now: Instant) -> Instant {
        let mut next = now + IDLE_TICK;
        if self.pairing_upkeep(now) {
            next = next.min(now + PAIRING_TICK);
        }
        let wants_sound = self.wants_sound();
        {
            let mut st = self.lock();
            for p in st.peers.iter_mut() {
                let Some(addr) = p.addr else { continue };
                if p.prioritized != Some(addr) {
                    p.prioritized = Some(addr);
                    if platform::prioritize_traffic(&self.sock, addr) {
                        log!("{}: network priority on", p.entry.name);
                    }
                }
                let every = if p.alive() { KEEP_WARM_EVERY } else { HEARTBEAT_EVERY };
                if p.last_heartbeat.is_none_or(|t| now - t >= every) {
                    p.last_heartbeat = Some(now);
                    self.send(
                        addr,
                        &ServerMsg::Heartbeat {
                            sent_micros: crate::now_micros(),
                            wants_sound,
                        },
                    );
                }
                for (seq, ev) in p.outbox.due(now, RETRANSMIT_AFTER) {
                    self.send(addr, &ServerMsg::Reliable { seq, ev });
                }
                if let Some(t) = p.last_heartbeat {
                    next = next.min(t + every);
                }
                if let Some(t) = p.outbox.next_due(RETRANSMIT_AFTER) {
                    next = next.min(t);
                }
                let alive = p.alive();
                if alive != p.was_alive {
                    p.was_alive = alive;
                    if alive {
                        log!("{} connected ({addr})", p.entry.name);
                    } else {
                        log!("{} disconnected", p.entry.name);
                    }
                }
            }
            // Addresses can change (DHCP, roaming); keep mutes pointed right.
            self.sync_muted(&st);
            // The PC typing goes to is gone or locked: type here again.
            if let Some(i) = st.switcher.keyboard().filter(|&i| st.switcher.active() != Some(i)) {
                if st.peers.get(i).is_none_or(|p| !p.alive() || p.blocked.is_some()) {
                    log!("the PC you were typing on can't take keys; typing here again");
                    st.switcher.keyboard_lost();
                }
            }
            if st.switcher.active().is_some_and(|i| !st.peers[i].alive()) {
                log!("lost the active device; taking input back");
                if let Some(switch) = st.switcher.force_leave() {
                    self.apply_switch(&mut st, switch);
                }
            }
        }
        next
    }

    /// Returns whether a pairing code is showing (it needs regular upkeep).
    fn pairing_upkeep(&self, now: Instant) -> bool {
        let mut guard = self.pairing.lock().unwrap();
        let Some(s) = guard.as_mut() else { return false };
        s.machine.tick(now);
        if let Some(addr) = s.poke {
            if !s.machine.is_done() && s.last_poke.is_none_or(|t| now - t >= POKE_EVERY) {
                s.last_poke = Some(now);
                let _ = self.sock.send_to(&pairing::poke_packet(), addr);
            }
        }
        true
    }

    /// Watch for a game in focus; while one is, the edges don't switch.
    fn game_loop(&self) {
        while self.running.load(Ordering::Relaxed) {
            let rules = self.games.lock().unwrap().clone();
            let game = rules.game_in_focus(self.desktop.foreground().as_ref());
            let mut st = self.lock();
            if st.game != game {
                match &game {
                    Some(g) => log!("{} is in focus: staying on this PC", game::display_name(g)),
                    None => log!("no game in focus: edge switching is back"),
                }
                st.switcher.set_in_game(game.is_some());
                st.game = game;
            }
            drop(st);
            std::thread::sleep(GAME_CHECK_EVERY);
        }
    }

    fn resolve_loop(&self) {
        let mut last_try: Vec<Option<Instant>> = Vec::new();
        while self.running.load(Ordering::Relaxed) {
            let todo: Vec<(usize, String)> = {
                let st = self.lock();
                last_try.resize(st.peers.len(), None);
                st.peers
                    .iter()
                    .enumerate()
                    .filter(|(i, p)| !p.alive() && last_try[*i].is_none_or(|t| t.elapsed() >= RESOLVE_EVERY))
                    .map(|(i, p)| (i, p.entry.address.clone()))
                    .collect()
            };
            for (i, address) in todo {
                last_try[i] = Some(Instant::now());
                // DNS can block for seconds; the state lock is not held here.
                match resolve(&address, self.client_port) {
                    Some(addr) => {
                        let mut st = self.lock();
                        let p = &mut st.peers[i];
                        if p.addr.is_none_or(|a| a.ip() != addr.ip()) {
                            log!("{} is at {addr}", p.entry.name);
                        }
                        p.addr = Some(addr);
                    }
                    None => log!("couldn't find {address} on the network"),
                }
            }
            std::thread::sleep(POLL);
        }
    }
}

/// Pacing interval for a mouse rate setting (0 = automatic by link speed).
fn motion_interval(rate_hz: u32, fast_link: bool) -> Duration {
    match rate_hz {
        0 if fast_link => FAST_INTERVAL,
        0 => MOTION_INTERVAL,
        hz => Duration::from_secs(1) / hz,
    }
}

fn free_side(peers: &[Peer]) -> Option<Side> {
    SIDE_PREFERENCE
        .into_iter()
        .find(|s| !peers.iter().any(|p| p.entry.side == *s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_rate_setting_picks_the_interval() {
        assert_eq!(motion_interval(0, true), FAST_INTERVAL);
        assert_eq!(motion_interval(0, false), MOTION_INTERVAL);
        assert_eq!(motion_interval(2000, false), Duration::from_micros(500));
        assert_eq!(motion_interval(8000, false), Duration::from_micros(125));
    }
}

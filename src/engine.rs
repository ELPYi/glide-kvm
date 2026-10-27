//! The one entry point for running glide: wires config + platform + network
//! into a running server or client, and offers start / stop / status.
//! The desktop app and the CLI are both thin layers over this.

use std::net::UdpSocket;
use std::sync::Arc;

use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::audio::play::Player;
use crate::audio::share::Sharer;
use crate::audio::AudioSettings;
use crate::client::{self, ClientHandle, ClientOptions};
use crate::clip::sync::ClipSync;
use crate::config::{ClientEntry, Config, Role, SwitchSettings};
use crate::hotkey::{Hotkey, Hotkeys};
use crate::input::Side;
use crate::log;
use crate::platform::{self, CaptureHandle};
use crate::server::{Server, ServerClipNet};
use crate::status::EngineStatus;

pub struct Engine {
    inner: Inner,
}

enum Inner {
    Server {
        server: Arc<Server>,
        capture: Option<CaptureHandle>,
        clip: Option<ClipSync>,
    },
    Client {
        handle: Option<ClientHandle>,
        /// Sends this PC's sound (stopped before the client).
        sharer: Option<Sharer>,
        clip: Option<ClipSync>,
    },
}

impl Engine {
    /// Validate the config, bind the port and start running as `role`.
    pub fn start(cfg: &Config, role: Role) -> Result<Engine> {
        cfg.validate(role)?;
        platform::raise_process_priority();
        let desktop = platform::desktop()?;
        let sock = bind_with_retry(cfg.port)?;

        let inner = match role {
            Role::Server => {
                let server = Server::start(cfg, desktop, sock)?;
                let s = server.clone();
                let capture = match platform::start_capture(server.grab_flag(), Box::new(move |ev| s.on_captured(ev))) {
                    Ok(c) => c,
                    Err(e) => {
                        server.stop();
                        return Err(e.context("starting input capture"));
                    }
                };
                server.set_player(Some(Player::start(cfg.audio, Box::new(platform::open_speaker))));
                let clip = ClipSync::start(
                    cfg.clipboard,
                    Box::new(ServerClipNet::new(&server)),
                    Box::new(platform::open_clipboard),
                );
                server.set_clip_inbox(Some(clip.inbox()));
                log!("sharing this keyboard and mouse on UDP {}", cfg.port);
                for c in &cfg.clients {
                    log!("  {} ({}) is {}", c.name, c.address, side_phrase(c.side));
                }
                log!("{} = lock / unlock switching", cfg.hotkeys.lock);
                Inner::Server {
                    server,
                    capture: Some(capture),
                    clip: Some(clip),
                }
            }
            Role::Client => {
                let opts = ClientOptions {
                    secret: cfg.secret.clone(),
                    server_address: cfg.server_address.clone(),
                    server_port: cfg.port,
                    games: cfg.games.clone(),
                };
                let handle = client::start(sock, desktop, opts)?;
                let sharer = match handle.audio_sender() {
                    Ok(uplink) => Some(Sharer::start(
                        cfg.audio,
                        Box::new(uplink),
                        Box::new(platform::open_loopback),
                    )),
                    Err(e) => {
                        log!("can't send this PC's sound: {e:#}");
                        None
                    }
                };
                let clip = handle.audio_sender().ok().map(|link| {
                    let clip = ClipSync::start(cfg.clipboard, Box::new(link), Box::new(platform::open_clipboard));
                    handle.set_clip_inbox(Some(clip.inbox()));
                    clip
                });
                match cfg.server_address.as_deref().filter(|a| !a.trim().is_empty()) {
                    Some(a) => log!("waiting for the server at {a} (UDP {})", cfg.port),
                    None => log!("waiting for the server on UDP {}", cfg.port),
                }
                Inner::Client {
                    handle: Some(handle),
                    sharer,
                    clip,
                }
            }
        };
        Ok(Engine { inner })
    }

    pub fn status(&self) -> EngineStatus {
        match &self.inner {
            Inner::Server { server, .. } => EngineStatus::Server(server.status()),
            Inner::Client {
                handle: Some(c),
                sharer,
                ..
            } => {
                let mut s = c.status();
                s.sending_sound = s.connected && sharer.as_ref().is_some_and(Sharer::sending);
                EngineStatus::Client(s)
            }
            Inner::Client { handle: None, .. } => EngineStatus::Stopped,
        }
    }

    /// Lock/unlock edge switching (server only; no-op on a client).
    pub fn set_locked(&self, locked: bool) {
        if let Inner::Server { server, .. } = &self.inner {
            server.set_locked(locked);
        }
    }

    /// Apply switching settings without a restart (server only).
    pub fn set_switching(&self, settings: SwitchSettings) {
        if let Inner::Server { server, .. } = &self.inner {
            server.set_switching(settings);
        }
    }

    /// Change the game rules live (both roles: a game counts on whichever PC
    /// it runs).
    pub fn set_games(&self, games: crate::game::GameSettings) {
        match &self.inner {
            Inner::Server { server, .. } => server.set_games(games),
            Inner::Client { handle: Some(c), .. } => c.set_games(games),
            Inner::Client { handle: None, .. } => {}
        }
    }

    /// Change sound settings live: playing them here (server), sending
    /// this PC's (client).
    /// Turn clipboard sharing on or off live.
    pub fn set_clipboard(&self, on: bool) {
        match &self.inner {
            Inner::Server { clip: Some(c), .. } | Inner::Client { clip: Some(c), .. } => c.set_enabled(on),
            _ => {}
        }
    }

    pub fn set_audio(&self, settings: AudioSettings) {
        match &self.inner {
            Inner::Server { server, .. } => server.set_audio(settings),
            Inner::Client { sharer: Some(s), .. } => s.set(settings),
            Inner::Client { .. } => {}
        }
    }

    /// Mute or unmute one device's sound on this PC (server only).
    pub fn set_device_muted(&self, index: usize, muted: bool) {
        if let Inner::Server { server, .. } = &self.inner {
            server.set_device_muted(index, muted);
        }
    }

    /// Change shortcuts live (server only).
    pub fn set_hotkeys(&self, hotkeys: Hotkeys) {
        if let Inner::Server { server, .. } = &self.inner {
            server.set_hotkeys(hotkeys);
        }
    }

    /// Capture the next key combination pressed on this PC (server only).
    pub fn record_hotkey(&self, on: bool) -> Result<()> {
        match &self.inner {
            Inner::Server { server, .. } => {
                server.record_hotkey(on);
                Ok(())
            }
            _ => bail!("shortcuts are recorded on the sharing PC"),
        }
    }

    pub fn take_recorded_hotkey(&self) -> Option<Option<Hotkey>> {
        match &self.inner {
            Inner::Server { server, .. } => server.take_recorded_hotkey(),
            _ => None,
        }
    }

    /// Re-arrange devices live; `sides[i]` is device `i`'s side (server only).
    pub fn set_layout(&self, sides: &[Side]) -> Result<()> {
        match &self.inner {
            Inner::Server { server, .. } => server.set_layout(sides),
            _ => Ok(()),
        }
    }

    /// Show a pairing code (server only). See [`crate::net::pairing`].
    pub fn start_pairing(&self, poke_address: Option<&str>) -> Result<String> {
        match &self.inner {
            Inner::Server { server, .. } => server.start_pairing(poke_address),
            _ => bail!("only the sharing PC can show a pairing code"),
        }
    }

    pub fn cancel_pairing(&self) {
        if let Inner::Server { server, .. } = &self.inner {
            server.cancel_pairing();
        }
    }

    /// Devices added by pairing since the last call; the caller saves them.
    pub fn take_paired(&self) -> Vec<ClientEntry> {
        match &self.inner {
            Inner::Server { server, .. } => server.take_paired(),
            _ => Vec::new(),
        }
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        match &mut self.inner {
            Inner::Server { server, capture, clip } => {
                server.set_clip_inbox(None);
                drop(clip.take());
                if let Some(c) = capture.take() {
                    c.stop(); // restore normal local input first
                    server.stop();
                    log!("stopped");
                }
            }
            Inner::Client { handle, sharer, clip } => {
                drop(sharer.take());
                drop(clip.take());
                if let Some(h) = handle.take() {
                    h.stop();
                    log!("stopped");
                }
            }
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn side_phrase(side: crate::input::Side) -> &'static str {
    use crate::input::Side::*;
    match side {
        Left => "to the left",
        Right => "to the right",
        Top => "above",
        Bottom => "below",
    }
}

/// Bind glide's port, waiting briefly if a previous glide is still closing
/// (glide restarts itself when switching to administrator mode).
fn bind_with_retry(port: u16) -> Result<UdpSocket> {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match UdpSocket::bind(("0.0.0.0", port)) {
            Ok(s) => return Ok(s),
            Err(_) if std::time::Instant::now() < deadline => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e).with_context(|| format!("UDP port {port} is in use (is glide already running?)")),
        }
    }
}

/// On a PC being set up: pair with the sharing PC at `server_address` using
/// the code it shows. Binds glide's `port` (stop the engine first) and blocks
/// for up to 15 s. On success, save the returned secret and port.
pub fn pair_with_server(port: u16, server_address: &str, code: &str) -> Result<crate::net::pairing::Paired> {
    use crate::net::{addr, pairing};
    let server = addr::resolve(server_address, port)
        .with_context(|| format!("couldn't find {server_address} on the network"))?;
    let sock = UdpSocket::bind(("0.0.0.0", port)).with_context(|| format!("UDP port {port} is in use"))?;
    let result = pairing::pair_with(&sock, server, code, &addr::machine_name(), Duration::from_secs(15));
    match &result {
        Ok(_) => log!("paired with {server_address}"),
        Err(e) => log!("pairing with {server_address} failed: {e}"),
    }
    result
}

//! # glide
//!
//! A low-latency software KVM: one keyboard and mouse shared across several
//! computers. Push the cursor off a screen edge and it continues on the next
//! machine.
//!
//! ## Module map
//!
//! | Module       | Responsibility                                                    |
//! |--------------|-------------------------------------------------------------------|
//! | [`config`]   | `glide.toml`: load, validate, save                                |
//! | [`engine`]   | Start/stop facade over server or client; what the UI and CLI use  |
//! | [`server`]   | Machine with the physical keyboard/mouse: capture, switch, send   |
//! | [`client`]   | Machine being controlled: receive, inject                         |
//! | [`net`]      | Wire protocol, encryption, reliable delivery                      |
//! | [`input`]    | OS-neutral input events (the "language" everything speaks)       |
//! | [`hotkey`]   | Customizable shortcuts: parse, display, match                     |
//! | [`platform`] | Everything OS-specific: hooks, injection, autostart, firewall     |
//! | [`status`]   | Read-only snapshots of engine state for display                   |
//! | [`gui`]      | The desktop app                                                   |
//! | [`logging`]  | `log!` macro + in-memory log the app displays                     |
//! | [`update`]   | Over-the-air updates from GitHub Releases                         |
//!
//! See `docs/ARCHITECTURE.md` for the threads, data flow and protocol.

pub mod client;
pub mod config;
pub mod engine;
pub mod game;
pub mod gui;
pub mod hotkey;
pub mod input;
pub mod logging;
pub mod net;
pub mod platform;
pub mod server;
pub mod status;
pub mod update;

/// This build's version: `0.4.17` for releases, `0.4.0-dev` for local builds.
pub const VERSION: &str = env!("GLIDE_VERSION");
/// GitHub `owner/repo` this build updates from; empty for local builds.
pub const UPDATE_REPO: &str = env!("GLIDE_UPDATE_REPO");

/// Built by CI as a release (updates itself; manages launch at sign-in).
pub fn is_release_build() -> bool {
    !UPDATE_REPO.is_empty() && !VERSION.ends_with("-dev")
}

/// Microseconds since the Unix epoch. Used for session ids and RTT stamps.
pub fn now_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

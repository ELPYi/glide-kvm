//! Read-only snapshots of engine state, produced on demand for the UI.
//! Plain data: cloning one never touches locks held by the hot path for long.

use std::net::SocketAddr;
use std::time::Duration;

use crate::input::{InputBlock, Side};

#[derive(Clone, Debug, Default)]
pub enum EngineStatus {
    #[default]
    Stopped,
    Server(ServerStatus),
    Client(ClientStatus),
}

#[derive(Clone, Debug, Default)]
pub struct ServerStatus {
    pub port: u16,
    pub peers: Vec<PeerStatus>,
    /// Index into `peers` of the machine currently receiving input.
    pub active: Option<usize>,
    pub locked: bool,
    /// The game in focus here (edges don't switch while it is), if any.
    pub game: Option<String>,
    /// A pairing code is being shown.
    pub pairing: Option<PairingStatus>,
    /// Waiting for a key combination to record as a shortcut.
    pub recording_hotkey: bool,
}

#[derive(Clone, Debug)]
pub struct PairingStatus {
    /// "482 913"
    pub code: String,
    pub expires_in: Duration,
    /// Wrong tries on this code so far.
    pub failed_attempts: u8,
    /// Someone is entering the code right now.
    pub checking: bool,
    /// A device paired with this code.
    pub done: bool,
}

#[derive(Clone, Debug)]
pub struct PeerStatus {
    pub name: String,
    /// As written in the config.
    pub address: String,
    pub side: Side,
    /// Where packets are actually going, once resolved.
    pub resolved: Option<SocketAddr>,
    pub connected: bool,
    /// Typical (median) round-trip time.
    pub rtt_ms: Option<f32>,
    /// Spikes: the 95th-percentile round trip.
    pub rtt_worst_ms: Option<f32>,
    /// It can't accept input right now (locked, admin window in front).
    pub blocked: Option<InputBlock>,
    /// Its sound is playing here right now.
    pub sound: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ClientStatus {
    pub port: u16,
    pub server: Option<SocketAddr>,
    pub connected: bool,
    /// This machine is currently receiving the keyboard and mouse.
    pub active: bool,
    /// The game in focus here (the edge back doesn't switch while it is).
    pub game: Option<String>,
    /// The sharing PC wants this PC's sound (and sharing is on here).
    pub sending_sound: bool,
}

//! Screens. Each `show` function draws from a read-only [`Model`] and pushes
//! [`Action`](super::actions::Action)s; none of them change app state directly
//! (the settings form edits its own draft, which is only applied on Save).

pub mod devices;
pub mod header;
pub mod logs;
pub mod settings;
pub mod welcome;

use super::actions::{HotkeyAction, Tab};
use super::theme::Tone;
use crate::config::Config;
use crate::net::addr::LocalAddr;
use crate::status::EngineStatus;

/// A short message shown under the header ("Saved", "Firewall rule added").
pub struct Notice {
    pub text: String,
    pub tone: Tone,
}

/// This PC's name and addresses (refreshed every few seconds).
pub struct ThisPc {
    pub name: String,
    pub addrs: Vec<LocalAddr>,
}

impl ThisPc {
    /// The address to type on another PC.
    pub fn main_address(&self) -> Option<String> {
        self.addrs.first().map(|a| a.ip.to_string())
    }
}

/// Everything a view may read.
pub struct Model<'a> {
    pub status: &'a EngineStatus,
    pub running: bool,
    /// The config on disk (what's running), as opposed to the settings draft.
    pub saved: &'a Config,
    /// Why the engine failed to start, if it did.
    pub error: Option<&'a str>,
    pub autostart: bool,
    pub firewall_busy: bool,
    pub tab: Tab,
    pub notice: Option<&'a Notice>,
    pub this_pc: &'a ThisPc,
    /// A "pair with server" attempt is running (client side).
    pub pairing_busy: bool,
    /// Which shortcut is waiting for a key combination, if any.
    pub recording_hotkey: Option<HotkeyAction>,
    /// This copy of glide has administrator rights.
    pub elevated: bool,
    /// An administrator-mode change is in progress (UAC prompt etc.).
    pub admin_busy: bool,
    /// What the updater is doing.
    pub update: crate::update::UpdateState,
}

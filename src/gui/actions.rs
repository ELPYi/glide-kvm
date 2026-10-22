//! Everything the user can ask the app to do. Views push these; only
//! [`App`](super::app::App) acts on them.

use crate::audio::AudioSettings;
use crate::config::Role;
use crate::hotkey::Hotkey;
use crate::input::Side;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Devices,
    Settings,
    Logs,
}

/// Which shortcut is being changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HotkeyAction {
    Lock,
}

#[derive(Debug, PartialEq)]
pub enum Action {
    Start,
    Stop,
    /// First run: pick what this machine is, save, and start.
    ChooseRole(Role),
    /// Write the settings draft to disk and apply it.
    SaveSettings,
    DiscardSettings,
    GenerateSecret,
    SetAutostart(bool),
    /// Add the inbound firewall rule (UAC prompt).
    OpenFirewall,
    ClearLogs,
    GoTo(Tab),
    /// Sharing PC: show a pairing code. `poke` = the new PC's address, only
    /// needed when this PC's firewall blocks the first contact.
    StartPairing {
        poke: Option<String>,
    },
    CancelPairing,
    /// New PC: pair with the sharing PC using the code it shows.
    PairWithServer {
        address: String,
        code: String,
    },
    /// Mute / unmute a device's sound on this PC (applies right away).
    SetDeviceMuted {
        index: usize,
        muted: bool,
    },
    /// Dragged on the layout map. Swaps with whatever was on `side`.
    MoveDevice {
        index: usize,
        side: Side,
    },
    /// Record the next key combination for this shortcut; `None` cancels.
    RecordHotkey(Option<HotkeyAction>),
    /// Set a shortcut right away (no Save needed): reset, turn off, or a
    /// freshly recorded combination.
    SetHotkey(HotkeyAction, Hotkey),
    /// Turn administrator mode on/off (controls Task Manager etc.).
    SetRunAsAdmin(bool),
    /// Turn automatic updates on/off (applies right away).
    SetAutoUpdate(bool),
    /// Sound settings, applied right away. `save`: also write them to disk
    /// (not on every step of a volume drag).
    SetAudio {
        settings: AudioSettings,
        save: bool,
    },
    CheckForUpdates,
}

/// Collects actions emitted while drawing a frame.
#[derive(Default)]
pub struct Actions(Vec<Action>);

impl Actions {
    pub fn push(&mut self, a: Action) {
        self.0.push(a);
    }

    pub fn drain(&mut self) -> std::vec::Drain<'_, Action> {
        self.0.drain(..)
    }
}

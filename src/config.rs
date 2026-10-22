//! `glide.toml`: the one settings file, shared by the app and the CLI.
//!
//! It lives next to the executable, so a glide folder is self-contained and
//! can be copied between machines.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::audio::AudioSettings;
use crate::game::GameSettings;
use crate::hotkey::Hotkeys;
use crate::input::Side;

pub const DEFAULT_PORT: u16 = 24801;
pub const FILE_NAME: &str = "glide.toml";
/// Written by a normal launch for the administrator copy it hands off to
/// ("normal" or "minimized"). See `bin/glide.rs`.
pub const LAUNCH_FILE: &str = "glide.launch";
const MIN_SECRET_LEN: usize = 16;

/// What this machine does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Has the physical keyboard and mouse; shares them.
    Server,
    /// Is controlled by a server.
    Client,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Unset until chosen in the app (or given on the CLI).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// Shared secret; must match on every machine. Generate with `glide-cli genkey`.
    pub secret: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Client only: the server's address. Optional, but lets the client reach
    /// out first, which means no firewall rule is needed on the client.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_address: Option<String>,
    /// Start sharing as soon as the app opens.
    #[serde(default = "yes")]
    pub start_on_open: bool,
    /// Run with administrator rights (via a scheduled task, no repeated UAC
    /// prompts) so glide can control admin windows such as Task Manager.
    #[serde(default)]
    pub run_as_admin: bool,
    /// The window's close button hides glide to the tray instead of quitting.
    #[serde(default = "yes")]
    pub close_to_tray: bool,
    /// Release builds: download and install new versions automatically.
    #[serde(default = "yes")]
    pub auto_update: bool,
    /// Set once a release build has turned on launching at sign-in for the
    /// first time; after that the Settings toggle is the user's call.
    #[serde(default)]
    pub sign_in_set_up: bool,
    #[serde(default)]
    pub switching: SwitchSettings,
    /// Keyboard shortcuts (pressed on the sharing PC).
    #[serde(default)]
    pub hotkeys: Hotkeys,
    /// Don't switch PCs while a game is in focus (on whichever PC it runs).
    #[serde(default)]
    pub games: GameSettings,
    /// The other PCs' sound, played on the sharing PC.
    #[serde(default)]
    pub audio: AudioSettings,
    /// Server only: the machines it can hand input to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub clients: Vec<ClientEntry>,
}

/// How and when the cursor crosses to another machine. Live-adjustable.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SwitchSettings {
    /// Don't cross while a mouse button is held, so dragging a window to the
    /// edge snaps it instead of throwing the cursor to another machine.
    pub block_while_dragging: bool,
    /// How long the cursor must keep pushing against an edge before crossing.
    /// 0 = instant. A small delay (100-250 ms) prevents accidental crossings.
    pub edge_delay_ms: u32,
    /// While controlling another PC, give it this PC's pointer speed and
    /// acceleration, so the mouse feels identical everywhere.
    pub match_pointer_feel: bool,
    /// Mouse updates sent per second while controlling another PC.
    /// 0 = automatic (4000 on a wired-class link, 1000 on Wi-Fi).
    /// Higher = less delay but more packets; 8000 effectively sends every
    /// report from a high-polling-rate mouse.
    pub mouse_rate_hz: u32,
}

/// Choices offered for [`SwitchSettings::mouse_rate_hz`] (0 = automatic).
pub const MOUSE_RATES: [u32; 5] = [0, 1000, 2000, 4000, 8000];

impl Default for SwitchSettings {
    fn default() -> Self {
        SwitchSettings {
            block_while_dragging: true,
            edge_delay_ms: 0,
            match_pointer_feel: true,
            mouse_rate_hz: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientEntry {
    pub name: String,
    /// IP, hostname, or Tailscale name. Optional ":port" overrides `port`.
    pub address: String,
    /// Which edge of the server's screens this client sits beyond.
    pub side: Side,
}

fn default_port() -> u16 {
    DEFAULT_PORT
}

fn yes() -> bool {
    true
}

impl Config {
    /// A fresh config with a newly generated secret.
    pub fn new_default() -> Config {
        Config::new_default_with_secret(&crate::net::crypto::generate_secret())
    }

    pub fn new_default_with_secret(secret: &str) -> Config {
        Config {
            role: None,
            secret: secret.to_string(),
            port: DEFAULT_PORT,
            server_address: None,
            start_on_open: true,
            run_as_admin: false,
            close_to_tray: true,
            auto_update: true,
            sign_in_set_up: false,
            switching: SwitchSettings::default(),
            hotkeys: Hotkeys::default(),
            games: GameSettings::default(),
            audio: AudioSettings::default(),
            clients: Vec::new(),
        }
    }

    pub fn parse(text: &str) -> Result<Config> {
        let mut cfg: Config = toml::from_str(text)?;
        // Scroll Lock was the lock shortcut before 0.4 (and many keyboards
        // lack the key); move such configs to the new default.
        if cfg.hotkeys.lock.to_string() == crate::hotkey::OLD_DEFAULT_LOCK {
            cfg.hotkeys = Hotkeys::default();
        }
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Config::parse(&text).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config always serializes")
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.to_toml()).with_context(|| format!("writing {}", path.display()))
    }

    /// Every problem that would stop `role` from running, as user-facing text.
    pub fn problems(&self, role: Role) -> Vec<String> {
        let mut out = Vec::new();
        if self.secret.len() < MIN_SECRET_LEN {
            out.push(format!("The secret must be at least {MIN_SECRET_LEN} characters."));
        }
        if self.port == 0 {
            out.push("The port can't be 0.".into());
        }
        // A server with no devices yet is fine: pairing adds them while running.
        if role == Role::Server {
            if !MOUSE_RATES.contains(&self.switching.mouse_rate_hz) {
                out.push("Mouse update rate must be automatic, 1000, 2000, 4000 or 8000.".into());
            }
            for (i, c) in self.clients.iter().enumerate() {
                if c.name.trim().is_empty() {
                    out.push(format!("Device #{} needs a name.", i + 1));
                }
                if c.address.trim().is_empty() {
                    out.push(format!("{} needs an address.", display_name(c, i)));
                }
                if self.clients[..i].iter().any(|b| b.side == c.side) {
                    out.push(format!(
                        "Two devices are set to the {} side.",
                        c.side.label().to_lowercase()
                    ));
                }
            }
        }
        out
    }

    /// `problems` as an error, for the CLI.
    pub fn validate(&self, role: Role) -> Result<()> {
        let p = self.problems(role);
        if !p.is_empty() {
            bail!("{}", p.join("\n"));
        }
        Ok(())
    }
}

fn display_name(c: &ClientEntry, i: usize) -> String {
    if c.name.trim().is_empty() {
        format!("Device #{}", i + 1)
    } else {
        c.name.clone()
    }
}

/// `glide.toml` next to the running executable.
pub fn default_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(FILE_NAME)))
        .unwrap_or_else(|| PathBuf::from(FILE_NAME))
}

/// Explicit path, else next to the executable, else the working directory.
pub fn find(explicit: Option<&str>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(PathBuf::from(p));
    }
    let candidates = [default_path(), PathBuf::from(FILE_NAME)];
    candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .with_context(|| format!("no config found (looked for {candidates:?}); pass -c <path>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(name: &str, side: Side) -> ClientEntry {
        ClientEntry {
            name: name.into(),
            address: "10.0.0.2".into(),
            side,
        }
    }

    #[test]
    fn minimal_file_gets_defaults() {
        let c = Config::parse(r#"secret = "0123456789abcdef""#).unwrap();
        assert_eq!(c.port, DEFAULT_PORT);
        assert_eq!(c.role, None);
        assert!(c.start_on_open);
        assert_eq!(c.switching, SwitchSettings::default());
        assert!(c.switching.block_while_dragging);
    }

    #[test]
    fn v1_config_still_loads() {
        // The format before the app existed.
        let c = Config::parse(
            r#"
            secret = "0123456789abcdef0123"
            port = 24801
            [[clients]]
            name = "laptop"
            address = "192.168.1.18"
            side = "right"
            "#,
        )
        .unwrap();
        assert_eq!(
            c.clients,
            vec![ClientEntry {
                name: "laptop".into(),
                address: "192.168.1.18".into(),
                side: Side::Right
            }]
        );
    }

    #[test]
    fn save_load_roundtrip() {
        let mut c = Config::new_default();
        c.role = Some(Role::Server);
        c.switching.edge_delay_ms = 150;
        c.clients.push(client("laptop", Side::Right));
        c.clients.push(client("mini", Side::Left));
        let back = Config::parse(&c.to_toml()).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn old_scroll_lock_shortcut_moves_to_ctrl_shift_l() {
        let c = Config::parse("secret = \"0123456789abcdef\"\n[hotkeys]\nlock = \"ScrollLock\"\n").unwrap();
        assert_eq!(c.hotkeys.lock.to_string(), "Ctrl+Shift+L");
        let custom = Config::parse("secret = \"0123456789abcdef\"\n[hotkeys]\nlock = \"F13\"\n").unwrap();
        assert_eq!(custom.hotkeys.lock.to_string(), "F13", "a custom choice is kept");
    }

    #[test]
    fn the_example_file_parses() {
        let c = Config::parse(include_str!("../glide.example.toml")).unwrap();
        assert_eq!(c.audio, crate::audio::AudioSettings::default());
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(Config::parse("secret = \"0123456789abcdef\"\ntypo = 1").is_err());
    }

    #[test]
    fn validation() {
        let mut c = Config::new_default();
        assert!(c.problems(Role::Client).is_empty());
        assert!(
            c.problems(Role::Server).is_empty(),
            "no devices yet is fine (pair them)"
        );

        c.clients.push(client("a", Side::Right));
        c.clients.push(client("b", Side::Right));
        assert_eq!(c.problems(Role::Server), vec!["Two devices are set to the right side."]);

        c.clients[1].side = Side::Left;
        c.clients[1].address = " ".into();
        c.secret = "short".into();
        let p = c.problems(Role::Server);
        assert_eq!(p.len(), 2, "{p:?}");
        assert!(c.validate(Role::Server).is_err());
    }
}

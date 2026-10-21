//! Keyboard shortcuts ("Ctrl+Shift+L"), stored as text in `glide.toml`
//! and matched against the neutral evdev key codes from capture.
//!
//! A shortcut is a set of modifiers plus one main key. Matching is exact:
//! Ctrl+Home does not fire while Ctrl+Shift+Home is held, so shortcuts never
//! collide with longer combinations other apps use.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Which modifiers must be held. Left and right versions count the same.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Windows / Super / Command key.
    pub win: bool,
}

const CTRL: [u16; 2] = [29, 97];
const ALT: [u16; 2] = [56, 100];
const SHIFT: [u16; 2] = [42, 54];
const WIN: [u16; 2] = [125, 126];

impl Mods {
    /// The modifiers currently held, from a set of pressed key codes.
    pub fn from_held(held: &HashSet<u16>) -> Mods {
        let any = |pair: [u16; 2]| pair.iter().any(|c| held.contains(c));
        Mods {
            ctrl: any(CTRL),
            alt: any(ALT),
            shift: any(SHIFT),
            win: any(WIN),
        }
    }
}

impl Mods {
    /// Key codes of the modifiers *not* in this set (both left and right).
    pub fn released_codes(&self) -> impl Iterator<Item = u16> {
        [(self.ctrl, CTRL), (self.alt, ALT), (self.shift, SHIFT), (self.win, WIN)]
            .into_iter()
            .filter(|(held, _)| !held)
            .flat_map(|(_, codes)| codes)
    }
}

pub fn is_modifier(code: u16) -> bool {
    [CTRL, ALT, SHIFT, WIN].iter().any(|pair| pair.contains(&code))
}

/// A shortcut, or "none" (disabled).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub mods: Mods,
    /// evdev code of the main key; 0 = disabled.
    pub key: u16,
}

impl Hotkey {
    pub const NONE: Hotkey = Hotkey {
        mods: Mods {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
        },
        key: 0,
    };

    pub fn new(mods: Mods, key: u16) -> Hotkey {
        Hotkey { mods, key }
    }

    pub fn is_none(&self) -> bool {
        self.key == 0
    }

    /// Does pressing `code` (with `held` keys down) trigger this shortcut?
    pub fn matches(&self, code: u16, held: &HashSet<u16>) -> bool {
        !self.is_none() && code == self.key && Mods::from_held(held) == self.mods
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.is_none() {
            return f.write_str("None");
        }
        let m = self.mods;
        for (on, name) in [(m.ctrl, "Ctrl"), (m.alt, "Alt"), (m.shift, "Shift"), (m.win, "Win")] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&key_name(self.key))
    }
}

impl FromStr for Hotkey {
    type Err = String;

    fn from_str(s: &str) -> Result<Hotkey, String> {
        let s = s.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("none") {
            return Ok(Hotkey::NONE);
        }
        let parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let Some((key_part, mod_parts)) = parts.split_last() else {
            return Err("empty shortcut".into());
        };
        let mut mods = Mods::default();
        for m in mod_parts {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods.ctrl = true,
                "alt" => mods.alt = true,
                "shift" => mods.shift = true,
                "win" | "super" | "meta" | "cmd" => mods.win = true,
                other => return Err(format!("\"{other}\" isn't a modifier (use Ctrl, Alt, Shift or Win)")),
            }
        }
        let key = key_code(key_part).ok_or_else(|| format!("unknown key \"{key_part}\""))?;
        if is_modifier(key) {
            return Err("a shortcut needs a main key besides the modifiers".into());
        }
        Ok(Hotkey { mods, key })
    }
}

impl Serialize for Hotkey {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Hotkey {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Hotkey, D::Error> {
        String::deserialize(d)?.parse().map_err(serde::de::Error::custom)
    }
}

/// The app's shortcuts, as stored under `[hotkeys]` in `glide.toml`.
/// (Unknown keys are ignored: older versions also had a `go_home` shortcut.)
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    /// Lock / unlock switching.
    pub lock: Hotkey,
}

/// The lock shortcut until 0.4; configs still using it move to the new default.
pub const OLD_DEFAULT_LOCK: &str = "ScrollLock";

impl Default for Hotkeys {
    fn default() -> Self {
        Hotkeys {
            lock: "Ctrl+Shift+L".parse().unwrap(),
        }
    }
}

/// (name, evdev code). The first name for a code is how it's displayed.
const KEYS: &[(&str, u16)] = &[
    ("Esc", 1),
    ("Escape", 1),
    ("1", 2),
    ("2", 3),
    ("3", 4),
    ("4", 5),
    ("5", 6),
    ("6", 7),
    ("7", 8),
    ("8", 9),
    ("9", 10),
    ("0", 11),
    ("-", 12),
    ("=", 13),
    ("Backspace", 14),
    ("Tab", 15),
    ("Q", 16),
    ("W", 17),
    ("E", 18),
    ("R", 19),
    ("T", 20),
    ("Y", 21),
    ("U", 22),
    ("I", 23),
    ("O", 24),
    ("P", 25),
    ("[", 26),
    ("]", 27),
    ("Enter", 28),
    ("Return", 28),
    ("A", 30),
    ("S", 31),
    ("D", 32),
    ("F", 33),
    ("G", 34),
    ("H", 35),
    ("J", 36),
    ("K", 37),
    ("L", 38),
    (";", 39),
    ("'", 40),
    ("`", 41),
    ("\\", 43),
    ("Z", 44),
    ("X", 45),
    ("C", 46),
    ("V", 47),
    ("B", 48),
    ("N", 49),
    ("M", 50),
    (",", 51),
    (".", 52),
    ("/", 53),
    ("Space", 57),
    ("CapsLock", 58),
    ("F1", 59),
    ("F2", 60),
    ("F3", 61),
    ("F4", 62),
    ("F5", 63),
    ("F6", 64),
    ("F7", 65),
    ("F8", 66),
    ("F9", 67),
    ("F10", 68),
    ("F11", 87),
    ("F12", 88),
    ("F13", 183),
    ("F14", 184),
    ("F15", 185),
    ("F16", 186),
    ("F17", 187),
    ("F18", 188),
    ("F19", 189),
    ("F20", 190),
    ("F21", 191),
    ("F22", 192),
    ("F23", 193),
    ("F24", 194),
    ("NumLock", 69),
    ("ScrollLock", 70),
    ("Pause", 119),
    ("Break", 119),
    ("PrintScreen", 99),
    ("PrtSc", 99),
    ("Num0", 82),
    ("Num1", 79),
    ("Num2", 80),
    ("Num3", 81),
    ("Num4", 75),
    ("Num5", 76),
    ("Num6", 77),
    ("Num7", 71),
    ("Num8", 72),
    ("Num9", 73),
    ("Num*", 55),
    ("Num-", 74),
    ("Num+", 78),
    ("Num.", 83),
    ("Num/", 98),
    ("NumEnter", 96),
    ("Home", 102),
    ("End", 107),
    ("PageUp", 104),
    ("PgUp", 104),
    ("PageDown", 109),
    ("PgDn", 109),
    ("Insert", 110),
    ("Ins", 110),
    ("Delete", 111),
    ("Del", 111),
    ("Up", 103),
    ("Down", 108),
    ("Left", 105),
    ("Right", 106),
    ("Menu", 127),
    ("Mute", 113),
    ("VolumeDown", 114),
    ("VolumeUp", 115),
    ("PlayPause", 164),
    ("NextTrack", 163),
    ("PrevTrack", 165),
    ("Stop", 166),
    ("Ctrl", 29),
    ("RightCtrl", 97),
    ("Alt", 56),
    ("AltGr", 100),
    ("Shift", 42),
    ("RightShift", 54),
    ("Win", 125),
    ("RightWin", 126),
];

pub fn key_code(name: &str) -> Option<u16> {
    KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, c)| *c)
}

/// Display name for an evdev code ("Key 123" if unnamed, still round-trips).
pub fn key_name(code: u16) -> String {
    match KEYS.iter().find(|(_, c)| *c == code) {
        Some((n, _)) => (*n).to_string(),
        None => format!("Key{code}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(codes: &[u16]) -> HashSet<u16> {
        codes.iter().copied().collect()
    }

    #[test]
    fn parse_and_display_roundtrip() {
        for s in [
            "ScrollLock",
            "Ctrl+Alt+Home",
            "Ctrl+Shift+F12",
            "Win+Num5",
            "Alt+`",
            "Ctrl+Pause",
            "None",
        ] {
            let h: Hotkey = s.parse().unwrap();
            assert_eq!(h.to_string(), s);
        }
        assert_eq!(
            "ctrl + alt + del".parse::<Hotkey>().unwrap().to_string(),
            "Ctrl+Alt+Delete"
        );
        assert_eq!("".parse::<Hotkey>().unwrap(), Hotkey::NONE);
    }

    #[test]
    fn bad_shortcuts_are_explained() {
        assert!("Ctrl+Banana".parse::<Hotkey>().unwrap_err().contains("unknown key"));
        assert!("Hyper+A".parse::<Hotkey>().unwrap_err().contains("isn't a modifier"));
        assert!("Ctrl+Shift".parse::<Hotkey>().unwrap_err().contains("main key"));
    }

    #[test]
    fn matching_is_exact() {
        let h: Hotkey = "Ctrl+Alt+Home".parse().unwrap();
        assert!(h.matches(102, &held(&[29, 56, 102])));
        assert!(h.matches(102, &held(&[97, 100, 102])), "right-hand modifiers count");
        assert!(!h.matches(102, &held(&[29, 102])), "missing Alt");
        assert!(!h.matches(102, &held(&[29, 56, 42, 102])), "extra Shift");
        assert!(!h.matches(107, &held(&[29, 56, 107])), "other key");
        assert!(!Hotkey::NONE.matches(0, &held(&[])));
    }

    #[test]
    fn unnamed_codes_still_roundtrip() {
        let h = Hotkey::new(Mods::default(), 250);
        assert_eq!(h.to_string(), "Key250");
        assert!("Key250".parse::<Hotkey>().is_err(), "only named keys can be typed");
    }

    #[test]
    fn toml_form() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct W {
            hotkeys: Hotkeys,
        }
        let w = W {
            hotkeys: Hotkeys::default(),
        };
        let text = toml::to_string(&w).unwrap();
        assert!(text.contains("lock = \"Ctrl+Shift+L\""), "{text}");
        assert_eq!(toml::from_str::<W>(&text).unwrap(), w);
        // Older configs with the removed jump-back shortcut still load.
        let old = "[hotkeys]\nlock = \"Ctrl+Shift+L\"\ngo_home = \"Ctrl+Alt+Home\"\n";
        assert_eq!(toml::from_str::<W>(old).unwrap(), w);
    }
}

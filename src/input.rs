//! OS-neutral input types. Everything that crosses the network uses these,
//! so a Windows server can drive a Linux or macOS client and vice versa.

/// Which edge of a screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Left, Side::Right, Side::Top, Side::Bottom];

    pub fn label(self) -> &'static str {
        match self {
            Side::Left => "Left",
            Side::Right => "Right",
            Side::Top => "Above",
            Side::Bottom => "Below",
        }
    }

    pub fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        }
    }

    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Option<Side> {
        Some(match v {
            0 => Side::Left,
            1 => Side::Right,
            2 => Side::Top,
            3 => Side::Bottom,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl Button {
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(v: u8) -> Option<Button> {
        Some(match v {
            0 => Button::Left,
            1 => Button::Right,
            2 => Button::Middle,
            3 => Button::Back,
            4 => Button::Forward,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// Raw (unaccelerated) relative motion in device counts.
    Motion {
        dx: i32,
        dy: i32,
    },
    Button {
        button: Button,
        down: bool,
    },
    /// 120 units per wheel notch. Positive dy = scroll up, positive dx = scroll right.
    Wheel {
        dx: i32,
        dy: i32,
    },
    /// `code` is a Linux evdev key code (KEY_*), used as the neutral wire format.
    Key {
        code: u16,
        down: bool,
    },
}

/// How the pointer responds to mouse movement (Windows' "pointer speed" and
/// "Enhance pointer precision"). Sent so a controlled PC can temporarily feel
/// exactly like the sharing PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PointerFeel {
    /// 1..=20, Windows' scale (10 = 1:1).
    pub speed: u8,
    /// Acceleration ("Enhance pointer precision").
    pub acceleration: bool,
}

/// Why a PC can't accept input right now. Windows silently discards injected
/// input in these states, so without knowing, the sharing PC would keep
/// "controlling" a PC that ignores it: the cursor stuck in limbo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputBlock {
    /// Lock screen, sign-in, UAC prompt or Ctrl+Alt+Del screen (Windows'
    /// protected "secure desktop").
    SecureScreen,
    /// An administrator window is in front and glide isn't running as admin.
    AdminWindow,
}

impl InputBlock {
    pub fn to_u8(self) -> u8 {
        match self {
            InputBlock::SecureScreen => 1,
            InputBlock::AdminWindow => 2,
        }
    }

    pub fn from_u8(v: u8) -> Option<InputBlock> {
        match v {
            1 => Some(InputBlock::SecureScreen),
            2 => Some(InputBlock::AdminWindow),
            _ => None,
        }
    }

    /// Short label for status pills.
    pub fn label(self) -> &'static str {
        match self {
            InputBlock::SecureScreen => "Locked",
            InputBlock::AdminWindow => "Admin window",
        }
    }

    /// What to do about it.
    pub fn advice(self) -> &'static str {
        match self {
            InputBlock::SecureScreen => {
                "Its lock screen (or a UAC prompt) is showing. Windows doesn't let glide control that; unlock it with its own keyboard."
            }
            InputBlock::AdminWindow => {
                "An admin window (like Task Manager) is in front there. Turn on Settings, Startup, \"Run glide as administrator\" on that PC to control it."
            }
        }
    }
}

/// The handful of evdev codes the app itself cares about (hotkeys).
pub mod keys {
    pub const KEY_LEFTCTRL: u16 = 29;
    pub const KEY_RIGHTCTRL: u16 = 97;
    pub const KEY_LEFTALT: u16 = 56;
    pub const KEY_RIGHTALT: u16 = 100;
    pub const KEY_SCROLLLOCK: u16 = 70;
    pub const KEY_HOME: u16 = 102;
}

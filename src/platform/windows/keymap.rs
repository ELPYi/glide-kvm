//! Windows scan codes (set 1) <-> Linux evdev key codes.
//!
//! For plain (non-E0) scan codes 0x01..=0x58 the two are identical, so only
//! extended keys and a few oddballs need a table.

/// E0-prefixed scan code -> evdev.
const EXTENDED: &[(u16, u16)] = &[
    (0x1C, 96),  // KP Enter
    (0x1D, 97),  // Right Ctrl
    (0x35, 98),  // KP /
    (0x37, 99),  // Print Screen
    (0x38, 100), // Right Alt / AltGr
    (0x47, 102), // Home
    (0x48, 103), // Up
    (0x49, 104), // Page Up
    (0x4B, 105), // Left
    (0x4D, 106), // Right
    (0x4F, 107), // End
    (0x50, 108), // Down
    (0x51, 109), // Page Down
    (0x52, 110), // Insert
    (0x53, 111), // Delete
    (0x5B, 125), // Left Win
    (0x5C, 126), // Right Win
    (0x5D, 127), // Menu
    (0x5E, 116), // Power
    (0x5F, 142), // Sleep
    (0x63, 143), // Wake
    (0x10, 165), // Previous track
    (0x19, 163), // Next track
    (0x20, 113), // Mute
    (0x21, 140), // Calculator
    (0x22, 164), // Play/Pause
    (0x24, 166), // Stop
    (0x2E, 114), // Volume down
    (0x30, 115), // Volume up
    (0x32, 172), // Browser home
    (0x65, 217), // Search
    (0x66, 156), // Favorites
    (0x67, 173), // Refresh
    (0x68, 128), // Browser stop
    (0x69, 159), // Forward
    (0x6A, 158), // Back
    (0x6B, 157), // My Computer
    (0x6C, 155), // Mail
    (0x6D, 226), // Media select
];

/// Non-extended scan codes above 0x58 -> evdev.
const PLAIN_HIGH: &[(u16, u16)] = &[
    (0x59, 117), // KP =
    (0x64, 183), // F13
    (0x65, 184),
    (0x66, 185),
    (0x67, 186),
    (0x68, 187),
    (0x69, 188),
    (0x6A, 189),
    (0x6B, 190),
    (0x6C, 191),
    (0x6D, 192),
    (0x6E, 193), // F23
    (0x76, 194), // F24
    (0x70, 93),  // Katakana/Hiragana
    (0x73, 89),  // Ro
    (0x79, 92),  // Henkan
    (0x7B, 94),  // Muhenkan
    (0x7D, 124), // Yen
];

/// Keys that are better identified (and injected) by virtual-key code,
/// because their scan codes are ambiguous or missing on some keyboards.
/// (vk, evdev, inject with KEYEVENTF_EXTENDEDKEY)
const BY_VK: &[(u32, u16, bool)] = &[
    (0x13, 119, false), // VK_PAUSE (shares scan 0x45 with NumLock)
    (0x90, 69, true),   // VK_NUMLOCK
    (0xA1, 54, false),  // VK_RSHIFT (sometimes reported as extended)
    (0x2C, 99, true),   // VK_SNAPSHOT (Alt+PrtSc reports scan 0x54)
    (0xAD, 113, true),  // VK_VOLUME_MUTE
    (0xAE, 114, true),  // VK_VOLUME_DOWN
    (0xAF, 115, true),  // VK_VOLUME_UP
    (0xB0, 163, true),  // VK_MEDIA_NEXT_TRACK
    (0xB1, 165, true),  // VK_MEDIA_PREV_TRACK
    (0xB2, 166, true),  // VK_MEDIA_STOP
    (0xB3, 164, true),  // VK_MEDIA_PLAY_PAUSE
];

pub fn from_windows(vk: u32, scan: u32, extended: bool) -> Option<u16> {
    if let Some(&(_, code, _)) = BY_VK.iter().find(|e| e.0 == vk) {
        return Some(code);
    }
    let scan = scan as u16;
    if scan == 0 {
        return None;
    }
    if extended {
        EXTENDED.iter().find(|e| e.0 == scan).map(|e| e.1)
    } else if scan <= 0x58 {
        Some(scan)
    } else {
        PLAIN_HIGH.iter().find(|e| e.0 == scan).map(|e| e.1)
    }
}

pub enum WinKey {
    Scan { scan: u16, extended: bool },
    Vk { vk: u16, extended: bool },
}

pub fn to_windows(code: u16) -> Option<WinKey> {
    if let Some(&(vk, _, extended)) = BY_VK.iter().find(|e| e.1 == code) {
        return Some(WinKey::Vk {
            vk: vk as u16,
            extended,
        });
    }
    if (1..=0x58).contains(&code) {
        return Some(WinKey::Scan {
            scan: code,
            extended: false,
        });
    }
    if let Some(e) = EXTENDED.iter().find(|e| e.1 == code) {
        return Some(WinKey::Scan {
            scan: e.0,
            extended: true,
        });
    }
    PLAIN_HIGH.iter().find(|e| e.1 == code).map(|e| WinKey::Scan {
        scan: e.0,
        extended: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_scancodes() {
        for (scan, ext) in (1..=0x58u16)
            .map(|s| (s, false))
            .chain(EXTENDED.iter().map(|e| (e.0, true)))
        {
            let code = from_windows(0, scan as u32, ext).unwrap();
            match to_windows(code).unwrap() {
                WinKey::Scan { scan: s, extended } => assert_eq!((s, extended), (scan, ext)),
                WinKey::Vk { .. } => {} // media/pause etc. deliberately go through VK
            }
        }
        assert_eq!(from_windows(0x41, 0x1E, false), Some(30)); // A
        assert_eq!(from_windows(0x25, 0x4B, true), Some(105)); // Left arrow
        assert_eq!(from_windows(0x13, 0x45, false), Some(119)); // Pause
        assert_eq!(from_windows(0x90, 0x45, true), Some(69)); // NumLock
    }
}

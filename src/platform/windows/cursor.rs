//! Hiding the cursor while it's "on another PC", so your eye follows it
//! across instead of finding it parked at the edge of this screen.
//!
//! Windows has no "hide the cursor everywhere" call (ShowCursor only
//! affects our own windows), so each standard system cursor is swapped for a
//! fully transparent one, and `SPI_SETCURSORS` reloads the real ones from the
//! user's settings. That reload is also run whenever glide starts, so a copy
//! that was killed while the cursor was hidden can't leave it invisible.

use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

/// Every cursor shape apps commonly show (arrow, text, resize, hand, ...).
const SHAPES: [SYSTEM_CURSOR_ID; 14] = [
    OCR_NORMAL,
    OCR_IBEAM,
    OCR_WAIT,
    OCR_CROSS,
    OCR_UP,
    OCR_SIZENWSE,
    OCR_SIZENESW,
    OCR_SIZEWE,
    OCR_SIZENS,
    OCR_SIZEALL,
    OCR_NO,
    OCR_HAND,
    OCR_APPSTARTING,
    OCR_HELP,
];

static HIDDEN: AtomicBool = AtomicBool::new(false);

/// Hide (or show again) the cursor on this PC. Cheap to call repeatedly.
pub fn set_hidden(hidden: bool) {
    if HIDDEN.swap(hidden, Ordering::SeqCst) == hidden {
        return;
    }
    if hidden {
        for shape in SHAPES {
            // SetSystemCursor takes ownership of (and destroys) the cursor it's
            // given, so each shape gets its own copy.
            let blank = blank_cursor();
            if !blank.is_null() {
                unsafe { SetSystemCursor(blank, shape) };
            }
        }
        refresh();
    } else {
        restore();
    }
}

/// Reload the user's own cursors (undoes any hiding, even by an earlier run).
pub fn restore() {
    unsafe { SystemParametersInfoW(SPI_SETCURSORS, 0, null_mut(), 0) };
    HIDDEN.store(false, Ordering::SeqCst);
    refresh();
}

/// The cursor's position on the desktop.
pub fn position() -> Option<(i32, i32)> {
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then_some((p.x, p.y))
}

/// Windows redraws the cursor shape on the next mouse move; "move" it to
/// where it already is so the change shows right away.
fn refresh() {
    if let Some((x, y)) = position() {
        unsafe { SetCursorPos(x, y) };
    }
}

/// A 32x32 cursor with every pixel transparent (AND mask all 1s, XOR all 0s).
fn blank_cursor() -> HCURSOR {
    const BYTES: usize = 32 * 32 / 8;
    let and_mask = [0xFFu8; BYTES];
    let xor_mask = [0u8; BYTES];
    unsafe {
        CreateCursor(
            null_mut(),
            0,
            0,
            32,
            32,
            and_mask.as_ptr().cast(),
            xor_mask.as_ptr().cast(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Graphics::Gdi::{DeleteObject, GetBitmapBits};

    /// The arrow cursor's monochrome mask (AND half then XOR half), if it has one.
    fn arrow_mask() -> Vec<u8> {
        unsafe {
            let arrow = LoadCursorW(null_mut(), IDC_ARROW);
            let mut info: ICONINFO = std::mem::zeroed();
            assert!(GetIconInfo(arrow, &mut info) != 0);
            let mut bits = vec![0u8; 32 * 64 / 8 * 4];
            let n = GetBitmapBits(info.hbmMask, bits.len() as i32, bits.as_mut_ptr().cast());
            bits.truncate(n.max(0) as usize);
            DeleteObject(info.hbmMask);
            if !info.hbmColor.is_null() {
                DeleteObject(info.hbmColor);
            }
            bits
        }
    }

    fn is_blank(mask: &[u8]) -> bool {
        let (and, xor) = mask.split_at(mask.len() / 2);
        and.iter().all(|b| *b == 0xFF) && xor.iter().all(|b| *b == 0)
    }

    /// Touches the real cursor (blinks it off for a moment); run with --ignored.
    #[test]
    #[ignore]
    fn hides_and_restores_the_real_cursor() {
        restore();
        assert!(!is_blank(&arrow_mask()), "the normal arrow has visible pixels");
        set_hidden(true);
        let hidden = arrow_mask();
        restore();
        assert!(is_blank(&hidden), "while hidden, the arrow is fully transparent");
        assert!(!is_blank(&arrow_mask()), "restored afterwards");
    }
}

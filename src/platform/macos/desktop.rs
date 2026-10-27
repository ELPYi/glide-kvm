//! Injection and cursor geometry.
//!
//! macOS has no relative mouse injection: each move is posted at an
//! absolute position (kept on the screens here), carrying the deltas too so
//! games still see motion. It also doesn't work out double clicks or which
//! modifiers are held for posted events, so both are tracked here and set
//! on every event.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use objc2::{class, msg_send};

use super::ffi::*;
use super::keymap;
use crate::input::{Button, InputEvent, PointerFeel, Side};
use crate::platform::geometry::{Rect, ScreenLayout};
use crate::platform::{Desktop, EdgeHit};

/// How long a cached display layout is trusted (displays rarely change).
const LAYOUT_TTL: Duration = Duration::from_secs(2);
/// Marks events glide posted, so capture can tell them from real input.
pub(super) const INJECTED: i64 = 0x676c_6964; // "glid"
/// Trust our own idea of where the cursor is for this long after a move:
/// the OS updates its position a moment after a posted event.
const TRACK_FOR: Duration = Duration::from_millis(100);
/// Pixels scrolled per wheel notch (Windows' 120 units).
const PIXELS_PER_NOTCH: i32 = 48;
/// A second click this close to the first (in points) can be a double click.
const DOUBLE_CLICK_SLOP: f64 = 4.0;

pub(super) const PERMISSION: &str = "glide needs Accessibility access to use the mouse and keyboard. \
     In System Settings, Privacy & Security, Accessibility, turn on glide, then quit and reopen glide.";

/// Asks for the Accessibility permission (macOS shows its prompt once).
pub(super) fn ensure_permission() -> Result<()> {
    unsafe {
        if !CGPreflightPostEventAccess() {
            CGRequestPostEventAccess();
            bail!(PERMISSION);
        }
    }
    Ok(())
}

struct Click {
    at: Instant,
    button: Button,
    pos: CGPoint,
    count: i64,
}

#[derive(Default)]
struct State {
    /// Where we last put the cursor, and when.
    pos: Option<(CGPoint, Instant)>,
    /// Buttons held through glide (moves become drags).
    buttons: [bool; 5],
    /// Modifiers held through glide (left/right device bits).
    mods: u64,
    last_click: Option<Click>,
    wheel: (i32, i32),
    /// Pointer speed from the sharing PC (1.0 = one point per count).
    speed: f64,
}

struct MacDesktop {
    source: usize,
    layout: Mutex<(Instant, ScreenLayout)>,
    state: Mutex<State>,
    double_click: Duration,
    hidden: AtomicBool,
}

pub fn desktop() -> Result<Arc<dyn Desktop>> {
    ensure_permission()?;
    let source = unsafe { CGEventSourceCreate(kCGEventSourceStateHIDSystemState) };
    let interval: f64 = unsafe { msg_send![class!(NSEvent), doubleClickInterval] };
    Ok(Arc::new(MacDesktop {
        source: source as usize,
        layout: Mutex::new((Instant::now(), screen_layout())),
        state: Mutex::new(State {
            speed: 1.0,
            ..State::default()
        }),
        double_click: Duration::from_secs_f64(interval.clamp(0.1, 2.0)),
        hidden: AtomicBool::new(false),
    }))
}

/// The displays, in points (CoreGraphics' global coordinates: top-left origin).
pub(super) fn screen_layout() -> ScreenLayout {
    let mut ids = [0u32; 16];
    let mut count = 0u32;
    unsafe {
        CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut count);
    }
    let monitors = ids[..count as usize]
        .iter()
        .map(|&id| {
            let b = unsafe { CGDisplayBounds(id) };
            Rect {
                left: b.origin.x.round() as i32,
                top: b.origin.y.round() as i32,
                right: (b.origin.x + b.size.width).round() as i32,
                bottom: (b.origin.y + b.size.height).round() as i32,
            }
        })
        .collect();
    ScreenLayout::new(monitors)
}

/// Device bits and the overall flag for a modifier key (evdev code).
fn modifier(code: u16) -> Option<(u64, u64)> {
    Some(match code {
        29 => (NX_DEVICELCTLKEYMASK, kCGEventFlagMaskControl),
        97 => (NX_DEVICERCTLKEYMASK, kCGEventFlagMaskControl),
        42 => (NX_DEVICELSHIFTKEYMASK, kCGEventFlagMaskShift),
        54 => (NX_DEVICERSHIFTKEYMASK, kCGEventFlagMaskShift),
        56 => (NX_DEVICELALTKEYMASK, kCGEventFlagMaskAlternate),
        100 => (NX_DEVICERALTKEYMASK, kCGEventFlagMaskAlternate),
        125 => (NX_DEVICELCMDKEYMASK, kCGEventFlagMaskCommand),
        126 => (NX_DEVICERCMDKEYMASK, kCGEventFlagMaskCommand),
        _ => return None,
    })
}

/// Event flags for the held modifiers.
fn flags(mods: u64) -> u64 {
    let mut f = mods;
    for code in [29, 97, 42, 54, 56, 100, 125, 126] {
        if let Some((device, overall)) = modifier(code) {
            if mods & device != 0 {
                f |= overall;
            }
        }
    }
    f
}

fn button_number(b: Button) -> u32 {
    match b {
        Button::Left => 0,
        Button::Right => 1,
        Button::Middle => 2,
        Button::Back => 3,
        Button::Forward => 4,
    }
}

impl MacDesktop {
    fn layout(&self) -> ScreenLayout {
        let mut cached = self.layout.lock().unwrap_or_else(|e| e.into_inner());
        if cached.0.elapsed() > LAYOUT_TTL {
            *cached = (Instant::now(), screen_layout());
        }
        cached.1.clone()
    }

    fn source(&self) -> CGEventSourceRef {
        self.source as CGEventSourceRef
    }

    fn post(&self, ev: CGEventRef, st: &State) {
        let Some(ev) = Cf::new(ev) else { return };
        unsafe {
            CGEventSetFlags(ev.0, flags(st.mods));
            CGEventSetIntegerValueField(ev.0, kCGEventSourceUserData, INJECTED);
            CGEventPost(kCGHIDEventTap, ev.0);
        }
    }

    fn position(&self, st: &State) -> CGPoint {
        match st.pos {
            Some((p, at)) if at.elapsed() < TRACK_FOR => p,
            _ => cursor_location().unwrap_or_default(),
        }
    }

    fn mouse(&self, ty: u32, pos: CGPoint, button: u32) -> CGEventRef {
        unsafe { CGEventCreateMouseEvent(self.source(), ty, pos, button) }
    }

    fn motion(&self, st: &mut State, dx: i32, dy: i32) {
        let from = self.position(st);
        let (nx, ny) = (from.x + dx as f64 * st.speed, from.y + dy as f64 * st.speed);
        let (fx, fy) = (from.x.floor() as i32, from.y.floor() as i32);
        let (cx, cy) = self
            .layout()
            .clamp_move(fx, fy, nx.floor() as i32 - fx, ny.floor() as i32 - fy);
        let to = CGPoint {
            x: if cx == nx.floor() as i32 { nx } else { cx as f64 },
            y: if cy == ny.floor() as i32 { ny } else { cy as f64 },
        };
        let (ty, button) = if st.buttons[0] {
            (kCGEventLeftMouseDragged, kCGMouseButtonLeft)
        } else if st.buttons[1] {
            (kCGEventRightMouseDragged, kCGMouseButtonRight)
        } else if let Some(n) = (2..5).find(|&n| st.buttons[n]) {
            (kCGEventOtherMouseDragged, n as u32)
        } else {
            (kCGEventMouseMoved, kCGMouseButtonLeft)
        };
        let ev = self.mouse(ty, to, button);
        if !ev.is_null() {
            unsafe {
                CGEventSetIntegerValueField(ev, kCGMouseEventDeltaX, dx as i64);
                CGEventSetIntegerValueField(ev, kCGMouseEventDeltaY, dy as i64);
            }
        }
        self.post(ev, st);
        st.pos = Some((to, Instant::now()));
    }

    fn button(&self, st: &mut State, button: Button, down: bool) {
        let pos = self.position(st);
        let n = button_number(button);
        st.buttons[n as usize] = down;
        let ty = match (button, down) {
            (Button::Left, true) => kCGEventLeftMouseDown,
            (Button::Left, false) => kCGEventLeftMouseUp,
            (Button::Right, true) => kCGEventRightMouseDown,
            (Button::Right, false) => kCGEventRightMouseUp,
            (_, true) => kCGEventOtherMouseDown,
            (_, false) => kCGEventOtherMouseUp,
        };
        // Double (and triple) clicks: macOS leaves counting to whoever posts.
        let count = if down {
            let count = match &st.last_click {
                Some(c)
                    if c.button == button
                        && c.at.elapsed() <= self.double_click
                        && (c.pos.x - pos.x).abs() <= DOUBLE_CLICK_SLOP
                        && (c.pos.y - pos.y).abs() <= DOUBLE_CLICK_SLOP =>
                {
                    c.count + 1
                }
                _ => 1,
            };
            st.last_click = Some(Click {
                at: Instant::now(),
                button,
                pos,
                count,
            });
            count
        } else {
            st.last_click
                .as_ref()
                .filter(|c| c.button == button)
                .map_or(1, |c| c.count)
        };
        let ev = self.mouse(ty, pos, n);
        if !ev.is_null() {
            unsafe {
                CGEventSetIntegerValueField(ev, kCGMouseEventClickState, count);
                CGEventSetIntegerValueField(ev, kCGMouseEventButtonNumber, n as i64);
            }
        }
        self.post(ev, st);
    }

    fn wheel(&self, st: &mut State, dx: i32, dy: i32) {
        // Windows: 120 per notch, positive = up / right. macOS: pixels,
        // positive = up / left.
        st.wheel.0 += dx * PIXELS_PER_NOTCH;
        st.wheel.1 += dy * PIXELS_PER_NOTCH;
        let (px, py) = (st.wheel.0 / 120, st.wheel.1 / 120);
        st.wheel.0 -= px * 120;
        st.wheel.1 -= py * 120;
        if px == 0 && py == 0 {
            return;
        }
        let ev = unsafe { CGEventCreateScrollWheelEvent2(self.source(), kCGScrollEventUnitPixel, 2, py, -px, 0) };
        self.post(ev, st);
    }

    fn key(&self, st: &mut State, code: u16, down: bool) {
        if let Some((device, _)) = modifier(code) {
            if down {
                st.mods |= device;
            } else {
                st.mods &= !device;
            }
        }
        let Some(kvk) = keymap::to_mac(code) else { return };
        let ev = unsafe { CGEventCreateKeyboardEvent(self.source(), kvk, down) };
        self.post(ev, st);
    }
}

// The event source is only used through CoreGraphics calls, which are
// thread-safe.
unsafe impl Send for MacDesktop {}
unsafe impl Sync for MacDesktop {}

impl Desktop for MacDesktop {
    fn inject(&self, ev: &InputEvent) {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match *ev {
            InputEvent::Motion { dx, dy } => self.motion(&mut st, dx, dy),
            InputEvent::Button { button, down } => self.button(&mut st, button, down),
            InputEvent::Wheel { dx, dy } => self.wheel(&mut st, dx, dy),
            InputEvent::Key { code, down } => self.key(&mut st, code, down),
        }
    }

    fn warp_to_edge(&self, side: Side, frac: f32) {
        let (x, y) = self.layout().entry_point(side, frac);
        let to = CGPoint {
            x: x as f64,
            y: y as f64,
        };
        unsafe {
            CGWarpMouseCursorPosition(to);
            // Without this, macOS ignores the mouse for a moment after a warp.
            CGAssociateMouseAndMouseCursorPosition(1);
        }
        self.state.lock().unwrap_or_else(|e| e.into_inner()).pos = Some((to, Instant::now()));
    }

    fn edge_hit(&self, dx: i32, dy: i32) -> Option<EdgeHit> {
        let p = self.position(&self.state.lock().unwrap_or_else(|e| e.into_inner()));
        self.layout().edge_hit(p.x.floor() as i32, p.y.floor() as i32, dx, dy)
    }

    fn pointer_feel(&self) -> Option<PointerFeel> {
        // Motion captured here already has the Mac's acceleration applied:
        // have the other PC move one pixel per count.
        Some(PointerFeel {
            speed: 10,
            acceleration: false,
        })
    }

    fn set_pointer_feel(&self, feel: Option<PointerFeel>) {
        // Windows' pointer speed slider, as a multiplier (10 = 1:1).
        const SPEEDS: [f64; 20] = [
            0.03125, 0.0625, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75,
            3.0, 3.25, 3.5,
        ];
        let speed = feel.map_or(1.0, |f| SPEEDS[f.speed.clamp(1, 20) as usize - 1]);
        self.state.lock().unwrap_or_else(|e| e.into_inner()).speed = speed;
    }

    fn set_cursor_hidden(&self, hidden: bool) {
        if self.hidden.swap(hidden, Ordering::Relaxed) == hidden {
            return;
        }
        super::cursor::allow_hiding_in_background();
        unsafe {
            if hidden {
                CGDisplayHideCursor(CGMainDisplayID());
            } else {
                CGDisplayShowCursor(CGMainDisplayID());
            }
        }
    }

    fn cursor_position(&self) -> Option<(i32, i32)> {
        cursor_location().map(|p| (p.x.floor() as i32, p.y.floor() as i32))
    }
}

impl Drop for MacDesktop {
    fn drop(&mut self) {
        // Hiding must never outlive glide.
        self.set_cursor_hidden(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_flags() {
        assert_eq!(flags(0), 0);
        let left_cmd_right_shift = NX_DEVICELCMDKEYMASK | NX_DEVICERSHIFTKEYMASK;
        assert_eq!(
            flags(left_cmd_right_shift),
            left_cmd_right_shift | kCGEventFlagMaskCommand | kCGEventFlagMaskShift
        );
    }
}

//! Input capture: a CoreGraphics event tap sees (and can swallow) every
//! key, button, scroll and move before apps do.
//!
//! While another PC is being controlled, the cursor is detached from the
//! mouse (`CGAssociateMouseAndMouseCursorPosition`), so it stays put here
//! while moves keep arriving as deltas, like a game's mouse look.
//!
//! The tap runs on a dedicated thread's run loop. macOS switches a tap off
//! if it's ever slow to answer; it's switched straight back on.

#![allow(non_upper_case_globals)] // CoreGraphics' names, matched on

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};

use super::desktop::{ensure_permission, screen_layout, INJECTED, PERMISSION};
use super::ffi::*;
use super::keymap;
use crate::hotkey::Mods;
use crate::input::{Button, InputEvent};
use crate::platform::geometry::ScreenLayout;
use crate::platform::{CaptureHandle, Captured, Handler};

const LAYOUT_TTL: Duration = Duration::from_secs(2);

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: CFStringRef;
    fn CFRunLoopRunInMode(mode: CFStringRef, seconds: f64, return_after_source: u8) -> i32;
}

struct Ctx {
    grab: Arc<AtomicBool>,
    handler: Handler,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
    static LAYOUT: RefCell<(Option<Instant>, ScreenLayout)> = RefCell::new((None, ScreenLayout::default()));
    static TAP: Cell<CFMachPortRef> = const { Cell::new(std::ptr::null_mut()) };
    /// The cursor is detached from the mouse (while grabbed).
    static DETACHED: Cell<bool> = const { Cell::new(false) };
    /// Scroll left over from fractions of a Windows wheel unit.
    static WHEEL: Cell<(f64, f64)> = const { Cell::new((0.0, 0.0)) };
}

fn grabbed() -> bool {
    CTX.with(|c| c.borrow().as_ref().is_some_and(|c| c.grab.load(Ordering::Relaxed)))
}

fn dispatch(ev: Captured) -> bool {
    CTX.with(|c| match c.try_borrow_mut() {
        Ok(mut c) => c.as_mut().is_some_and(|c| (c.handler)(ev)),
        Err(_) => false,
    })
}

/// Detach the cursor while grabbed, reattach otherwise.
fn sync_cursor() {
    let want = grabbed();
    DETACHED.with(|d| {
        if d.get() != want {
            unsafe {
                CGAssociateMouseAndMouseCursorPosition(u32::from(!want));
            }
            d.set(want);
        }
    });
}

fn layout_edge_hit(x: f64, y: f64, dx: i32, dy: i32) -> Option<crate::platform::EdgeHit> {
    LAYOUT.with(|l| {
        let mut l = l.borrow_mut();
        if l.0.is_none_or(|at| at.elapsed() > LAYOUT_TTL) {
            *l = (Some(Instant::now()), screen_layout());
        }
        l.1.edge_hit(x.floor() as i32, y.floor() as i32, dx, dy)
    })
}

fn mods(flags: u64) -> Mods {
    Mods {
        ctrl: flags & kCGEventFlagMaskControl != 0,
        alt: flags & kCGEventFlagMaskAlternate != 0,
        shift: flags & kCGEventFlagMaskShift != 0,
        win: flags & kCGEventFlagMaskCommand != 0,
    }
}

/// For a modifier's FlagsChanged event: is it now down?
fn modifier_down(kvk: u16, flags: u64) -> Option<bool> {
    let bit = match kvk {
        0x3B => NX_DEVICELCTLKEYMASK,
        0x3E => NX_DEVICERCTLKEYMASK,
        0x38 => NX_DEVICELSHIFTKEYMASK,
        0x3C => NX_DEVICERSHIFTKEYMASK,
        0x3A => NX_DEVICELALTKEYMASK,
        0x3D => NX_DEVICERALTKEYMASK,
        0x37 => NX_DEVICELCMDKEYMASK,
        0x36 => NX_DEVICERCMDKEYMASK,
        _ => return None,
    };
    Some(flags & bit != 0)
}

const CAPS_LOCK: u16 = 0x39;

unsafe fn field(ev: CGEventRef, f: u32) -> i64 {
    CGEventGetIntegerValueField(ev, f)
}

/// Returns whether to swallow the event.
unsafe fn handle(ty: u32, ev: CGEventRef) -> bool {
    let key = |code: Option<u16>, down: bool| match code {
        Some(code) => dispatch(Captured::Input(InputEvent::Key { code, down })),
        None => grabbed(),
    };
    let button = |button: Option<Button>, down: bool| match button {
        Some(button) => dispatch(Captured::Input(InputEvent::Button { button, down })),
        None => grabbed(),
    };
    match ty {
        kCGEventKeyDown | kCGEventKeyUp => {
            let down = ty == kCGEventKeyDown;
            if down && !grabbed() {
                // Nothing is being blocked, so the OS's view of the
                // modifiers is accurate: share it.
                dispatch(Captured::Modifiers(mods(CGEventGetFlags(ev))));
            }
            key(keymap::from_mac(field(ev, kCGKeyboardEventKeycode) as u16), down)
        }
        kCGEventFlagsChanged => {
            let kvk = field(ev, kCGKeyboardEventKeycode) as u16;
            let flags = CGEventGetFlags(ev);
            if kvk == CAPS_LOCK {
                // Reported once per press (as a toggle): send a full press.
                let code = keymap::from_mac(kvk);
                key(code, true) | key(code, false)
            } else {
                match modifier_down(kvk, flags) {
                    Some(down) => key(keymap::from_mac(kvk), down),
                    None => grabbed(), // Fn
                }
            }
        }
        kCGEventMouseMoved | kCGEventLeftMouseDragged | kCGEventRightMouseDragged | kCGEventOtherMouseDragged => {
            let (dx, dy) = (
                field(ev, kCGMouseEventDeltaX) as i32,
                field(ev, kCGMouseEventDeltaY) as i32,
            );
            if dx == 0 && dy == 0 {
                return grabbed();
            }
            if !grabbed() {
                let p = CGEventGetLocation(ev);
                if let Some(hit) = layout_edge_hit(p.x, p.y, dx, dy) {
                    dispatch(Captured::Edge(hit));
                }
            }
            if grabbed() {
                dispatch(Captured::Input(InputEvent::Motion { dx, dy }));
            }
            grabbed()
        }
        kCGEventLeftMouseDown | kCGEventLeftMouseUp => button(Some(Button::Left), ty == kCGEventLeftMouseDown),
        kCGEventRightMouseDown | kCGEventRightMouseUp => button(Some(Button::Right), ty == kCGEventRightMouseDown),
        kCGEventOtherMouseDown | kCGEventOtherMouseUp => {
            let b = match field(ev, kCGMouseEventButtonNumber) {
                2 => Some(Button::Middle),
                3 => Some(Button::Back),
                4 => Some(Button::Forward),
                _ => None,
            };
            button(b, ty == kCGEventOtherMouseDown)
        }
        kCGEventScrollWheel => {
            // Lines, in 16.16 fixed point (fractions from trackpads), to
            // Windows' 120 per notch. macOS: positive = up / left.
            let lines = |f| field(ev, f) as f64 / 65536.0 * 120.0;
            let (mut x, mut y) = WHEEL.with(Cell::get);
            x -= lines(kCGScrollWheelEventFixedPtDeltaAxis2);
            y += lines(kCGScrollWheelEventFixedPtDeltaAxis1);
            let (dx, dy) = (x.trunc() as i32, y.trunc() as i32);
            WHEEL.with(|w| w.set((x - dx as f64, y - dy as f64)));
            if dx == 0 && dy == 0 {
                return grabbed();
            }
            dispatch(Captured::Input(InputEvent::Wheel { dx, dy }))
        }
        _ => false,
    }
}

unsafe extern "C" fn tap_callback(_proxy: *mut c_void, ty: u32, ev: CGEventRef, _user: *mut c_void) -> CGEventRef {
    if ty == kCGEventTapDisabledByTimeout || ty == kCGEventTapDisabledByUserInput {
        crate::log!("macOS paused input capture; resuming it");
        TAP.with(|t| CGEventTapEnable(t.get(), true));
        return ev;
    }
    if field(ev, kCGEventSourceUserData) == INJECTED {
        return ev;
    }
    let block = handle(ty, ev);
    sync_cursor(); // an edge or a shortcut may have switched
    if block {
        std::ptr::null_mut()
    } else {
        ev
    }
}

fn mask(types: &[u32]) -> u64 {
    types.iter().fold(0, |m, &t| m | (1u64 << t))
}

/// Starts capture on its own thread. Returns once the tap is live.
pub fn start_capture(grab: Arc<AtomicBool>, handler: Handler) -> Result<CaptureHandle> {
    ensure_permission()?;
    let running = Arc::new(AtomicBool::new(true));
    let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
    let still = running.clone();
    let thread = std::thread::Builder::new()
        .name("glide-capture".into())
        .spawn(move || unsafe {
            super::boost_latency_thread();
            CTX.with(|c| *c.borrow_mut() = Some(Ctx { grab, handler }));
            let events = mask(&[
                kCGEventKeyDown,
                kCGEventKeyUp,
                kCGEventFlagsChanged,
                kCGEventMouseMoved,
                kCGEventLeftMouseDown,
                kCGEventLeftMouseUp,
                kCGEventRightMouseDown,
                kCGEventRightMouseUp,
                kCGEventLeftMouseDragged,
                kCGEventRightMouseDragged,
                kCGEventOtherMouseDown,
                kCGEventOtherMouseUp,
                kCGEventOtherMouseDragged,
                kCGEventScrollWheel,
            ]);
            let tap = CGEventTapCreate(
                kCGHIDEventTap,
                kCGHeadInsertEventTap,
                kCGEventTapOptionDefault,
                events,
                tap_callback,
                std::ptr::null_mut(),
            );
            if tap.is_null() {
                let _ = ready_tx.send(Err(anyhow!(PERMISSION)));
                return;
            }
            TAP.with(|t| t.set(tap));
            let source = CFMachPortCreateRunLoopSource(std::ptr::null(), tap, 0);
            CFRunLoopAddSource(CFRunLoopGetCurrent(), source, kCFRunLoopDefaultMode);
            CGEventTapEnable(tap, true);
            let _ = ready_tx.send(Ok(()));
            while still.load(Ordering::Relaxed) {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.25, 0);
                sync_cursor(); // control may have come back over the network
            }
            CGEventTapEnable(tap, false);
            CFMachPortInvalidate(tap);
            CFRelease(source);
            CFRelease(tap);
            TAP.with(|t| t.set(std::ptr::null_mut()));
            CTX.with(|c| *c.borrow_mut() = None);
            sync_cursor(); // never leave the cursor detached
        })?;

    ready_rx
        .recv()
        .map_err(|_| anyhow!("capture thread died during startup"))??;
    Ok(CaptureHandle::new(move || {
        running.store(false, Ordering::Relaxed);
        let _ = thread.join();
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers_from_flags() {
        let f = kCGEventFlagMaskCommand | NX_DEVICERCMDKEYMASK | kCGEventFlagMaskShift | NX_DEVICELSHIFTKEYMASK;
        assert_eq!(
            mods(f),
            Mods {
                ctrl: false,
                alt: false,
                shift: true,
                win: true
            }
        );
        assert_eq!(modifier_down(0x36, f), Some(true), "right command");
        assert_eq!(modifier_down(0x37, f), Some(false), "left command is up");
        assert_eq!(modifier_down(0x00, f), None, "A isn't a modifier");
    }
}

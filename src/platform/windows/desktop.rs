//! Injection and cursor geometry.
//!
//! Keys are injected by scan code (games read scan codes), the mouse with
//! relative moves so the receiving machine's own pointer acceleration applies.

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use windows_sys::Win32::Foundation::{BOOL, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos, XBUTTON1, XBUTTON2};

use super::keymap::{self, WinKey};
use crate::input::{Button, InputBlock, InputEvent, PointerFeel, Side};
use crate::platform::geometry::{Rect, ScreenLayout};
use crate::platform::{Desktop, EdgeHit};

/// How long a cached monitor layout is trusted (displays rarely change).
const LAYOUT_TTL: Duration = Duration::from_secs(2);

pub(super) struct WinDesktop {
    layout: Mutex<(Instant, ScreenLayout)>,
    /// Running as administrator (admin windows don't block us then).
    elevated: bool,
}

impl WinDesktop {
    pub(super) fn new() -> Self {
        WinDesktop {
            layout: Mutex::new((Instant::now(), screen_layout())),
            elevated: super::admin::is_elevated(),
        }
    }

    fn layout(&self) -> ScreenLayout {
        let mut cached = self.layout.lock().unwrap_or_else(|e| e.into_inner());
        if cached.0.elapsed() > LAYOUT_TTL {
            *cached = (Instant::now(), screen_layout());
        }
        cached.1.clone()
    }
}

pub fn desktop() -> Result<Arc<dyn Desktop>> {
    super::init_dpi();
    // In case an earlier run was killed while the cursor was hidden.
    super::cursor::restore();
    Ok(Arc::new(WinDesktop::new()))
}

/// Snapshot of the monitor layout (one OS call; cheap to keep and query).
pub(super) fn screen_layout() -> ScreenLayout {
    unsafe extern "system" fn collect(mon: HMONITOR, _dc: HDC, _r: *mut RECT, data: LPARAM) -> BOOL {
        let mut mi: MONITORINFO = zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(mon, &mut mi) != 0 {
            let r = mi.rcMonitor;
            (*(data as *mut Vec<Rect>)).push(Rect {
                left: r.left,
                top: r.top,
                right: r.right,
                bottom: r.bottom,
            });
        }
        1
    }
    let mut monitors: Vec<Rect> = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(collect),
            &mut monitors as *mut Vec<Rect> as LPARAM,
        );
    }
    ScreenLayout::new(monitors)
}

pub(super) fn cursor_pos() -> Option<POINT> {
    let mut p = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut p) } != 0).then_some(p)
}

fn send(input: INPUT) {
    unsafe {
        SendInput(1, &input, size_of::<INPUT>() as i32);
    }
}

fn mouse_input(dx: i32, dy: i32, data: i32, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as _,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn key_input(vk: u16, scan: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn button_flags(button: Button, down: bool) -> (u32, i32) {
    match (button, down) {
        (Button::Left, true) => (MOUSEEVENTF_LEFTDOWN, 0),
        (Button::Left, false) => (MOUSEEVENTF_LEFTUP, 0),
        (Button::Right, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
        (Button::Right, false) => (MOUSEEVENTF_RIGHTUP, 0),
        (Button::Middle, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
        (Button::Middle, false) => (MOUSEEVENTF_MIDDLEUP, 0),
        (Button::Back, true) => (MOUSEEVENTF_XDOWN, XBUTTON1 as i32),
        (Button::Back, false) => (MOUSEEVENTF_XUP, XBUTTON1 as i32),
        (Button::Forward, true) => (MOUSEEVENTF_XDOWN, XBUTTON2 as i32),
        (Button::Forward, false) => (MOUSEEVENTF_XUP, XBUTTON2 as i32),
    }
}

impl Desktop for WinDesktop {
    fn inject(&self, ev: &InputEvent) {
        match *ev {
            InputEvent::Motion { dx, dy } => send(mouse_input(dx, dy, 0, MOUSEEVENTF_MOVE)),
            InputEvent::Button { button, down } => {
                let (flags, data) = button_flags(button, down);
                send(mouse_input(0, 0, data, flags));
            }
            InputEvent::Wheel { dx, dy } => {
                if dy != 0 {
                    send(mouse_input(0, 0, dy, MOUSEEVENTF_WHEEL));
                }
                if dx != 0 {
                    send(mouse_input(0, 0, dx, MOUSEEVENTF_HWHEEL));
                }
            }
            InputEvent::Key { code, down } => {
                let up = if down { 0 } else { KEYEVENTF_KEYUP };
                let ext = |e: bool| if e { KEYEVENTF_EXTENDEDKEY } else { 0 };
                match keymap::to_windows(code) {
                    Some(WinKey::Scan { scan, extended }) => {
                        send(key_input(0, scan, KEYEVENTF_SCANCODE | ext(extended) | up))
                    }
                    Some(WinKey::Vk { vk, extended }) => send(key_input(vk, 0, ext(extended) | up)),
                    None => {}
                }
            }
        }
    }

    fn warp_to_edge(&self, side: Side, frac: f32) {
        let (x, y) = self.layout().entry_point(side, frac);
        unsafe {
            SetCursorPos(x, y);
        }
    }

    fn edge_hit(&self, dx: i32, dy: i32) -> Option<EdgeHit> {
        let p = cursor_pos()?;
        self.layout().edge_hit(p.x, p.y, dx, dy)
    }

    fn notify(&self, text: &str) {
        super::osd::show(text);
    }

    fn set_cursor_hidden(&self, hidden: bool) {
        super::cursor::set_hidden(hidden);
    }

    fn cursor_position(&self) -> Option<(i32, i32)> {
        super::cursor::position()
    }

    fn input_block(&self) -> Option<InputBlock> {
        super::blocking::input_block(self.elevated)
    }

    fn pointer_feel(&self) -> Option<PointerFeel> {
        super::pointer::current()
    }

    fn set_pointer_feel(&self, feel: Option<PointerFeel>) {
        super::pointer::set(feel);
    }
}

//! Input capture.
//!
//! WH_KEYBOARD_LL / WH_MOUSE_LL hooks see (and can swallow) every physical
//! key, button and wheel event. Mouse *motion* comes from Raw Input instead,
//! so it is unaccelerated and keeps arriving even while the cursor is frozen.
//!
//! The mouse hook is only installed while another PC is being controlled
//! (the only time mouse input must be swallowed). Windows waits for a
//! mouse hook on every report before moving the cursor, so with a gaming
//! mouse it was most of glide's CPU (3.7% of a core at 8 kHz vs 0.4%
//! without) and sat on the path of the local cursor. Locally, Raw Input
//! supplies the buttons and `GetCursorPos` the position for edge checks
//! (which can lag one report, i.e. ≤1 ms: an edge push is many reports).
//!
//! Hooks run on the thread that installed them, and only while that thread
//! pumps messages, so capture owns a dedicated high-priority thread.

use std::cell::{Cell, RefCell};
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use anyhow::{anyhow, Result};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
};
use windows_sys::Win32::UI::Input::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::desktop::screen_layout;
use super::{keymap, wanted, wide, SELFTEST};
use crate::hotkey::Mods;
use crate::input::{Button, InputEvent};
use crate::platform::geometry::ScreenLayout;
use crate::platform::{CaptureHandle, Captured, Handler};

const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;
/// Timer that re-checks our Raw Input registration (see [`ensure_raw_input`]).
const RAW_INPUT_CHECK_TIMER: usize = 1;
const RAW_INPUT_CHECK_MS: u32 = 1000;
/// Fake shift scan codes some keyboards emit around navigation keys.
const FAKE_SHIFTS: [u32; 2] = [0x2A, 0x36];

struct Ctx {
    grab: Arc<AtomicBool>,
    handler: Handler,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
    /// Monitor layout, refreshed when displays change. Keeps the per-event
    /// edge check to a few comparisons (a gaming mouse sends 1000s/second).
    static LAYOUT: RefCell<ScreenLayout> = RefCell::new(ScreenLayout::default());
    /// The mouse hook, while installed (only while grabbed).
    static MOUSE_HOOK: Cell<HHOOK> = const { Cell::new(null_mut()) };
}

/// Install the mouse hook while grabbed, remove it otherwise. Runs on the
/// capture thread after anything that can switch. Entering another PC is
/// always decided on this thread (an edge or a shortcut), so the hook is in
/// place before the next mouse report; leaving can come from the network,
/// and then a lingering hook just passes input through until the next check.
fn sync_mouse_hook() {
    let want = grabbed();
    MOUSE_HOOK.with(|h| unsafe {
        if want && h.get().is_null() {
            h.set(SetWindowsHookExW(
                WH_MOUSE_LL,
                Some(mouse_proc),
                GetModuleHandleW(null()),
                0,
            ));
            if h.get().is_null() {
                crate::log!("couldn't install the mouse hook; this PC's mouse isn't blocked");
            }
        } else if !want && !h.get().is_null() {
            UnhookWindowsHookEx(h.get());
            h.set(null_mut());
        }
    });
}

fn mouse_hook_installed() -> bool {
    MOUSE_HOOK.with(|h| !h.get().is_null())
}

fn refresh_layout() {
    LAYOUT.with(|l| *l.borrow_mut() = screen_layout());
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

/// Selftest only: keep the user's real input out of the measurement.
fn selftest_blocks_real_input() -> bool {
    SELFTEST.load(Ordering::Relaxed) && grabbed()
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let k = &*(lparam as *const KBDLLHOOKSTRUCT);
        if !wanted(k.flags & LLKHF_INJECTED != 0) {
            if selftest_blocks_real_input() {
                return 1;
            }
        } else {
            let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            if down && !grabbed() {
                // Nothing is being blocked right now, so the OS's view of
                // which modifiers are held is accurate: share it.
                dispatch(Captured::Modifiers(os_modifiers()));
            }
            let extended = k.flags & LLKHF_EXTENDED != 0;
            let fake_shift = extended && FAKE_SHIFTS.contains(&k.scanCode);
            let block = match keymap::from_windows(k.vkCode, k.scanCode, extended) {
                Some(code) if !fake_shift => dispatch(Captured::Input(InputEvent::Key { code, down })),
                _ => grabbed(),
            };
            sync_mouse_hook(); // a shortcut may have switched
            if block {
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// Which modifiers Windows currently considers held.
fn os_modifiers() -> Mods {
    let down = |vk: VIRTUAL_KEY| unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000 != 0;
    Mods {
        ctrl: down(VK_CONTROL),
        alt: down(VK_MENU),
        shift: down(VK_SHIFT),
        win: down(VK_LWIN) || down(VK_RWIN),
    }
}

fn mouse_event(msg: u32, mouse_data: u32) -> Option<InputEvent> {
    let hi = (mouse_data >> 16) as u16;
    let xbutton = if hi == XBUTTON1 { Button::Back } else { Button::Forward };
    let button = |button, down| Some(InputEvent::Button { button, down });
    match msg {
        WM_LBUTTONDOWN => button(Button::Left, true),
        WM_LBUTTONUP => button(Button::Left, false),
        WM_RBUTTONDOWN => button(Button::Right, true),
        WM_RBUTTONUP => button(Button::Right, false),
        WM_MBUTTONDOWN => button(Button::Middle, true),
        WM_MBUTTONUP => button(Button::Middle, false),
        WM_XBUTTONDOWN => button(xbutton, true),
        WM_XBUTTONUP => button(xbutton, false),
        WM_MOUSEWHEEL => Some(InputEvent::Wheel {
            dx: 0,
            dy: hi as i16 as i32,
        }),
        WM_MOUSEHWHEEL => Some(InputEvent::Wheel {
            dx: hi as i16 as i32,
            dy: 0,
        }),
        _ => None, // WM_MOUSEMOVE: motion is taken from Raw Input
    }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let m = &*(lparam as *const MSLLHOOKSTRUCT);
        if !wanted(m.flags & LLMHF_INJECTED != 0) {
            if selftest_blocks_real_input() {
                return 1;
            }
        } else {
            let block = match mouse_event(wparam as u32, m.mouseData) {
                Some(ev) => dispatch(Captured::Input(ev)),
                None => grabbed(),
            };
            if block {
                return 1;
            }
        }
    }
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_DISPLAYCHANGE {
        refresh_layout();
    }
    if msg == WM_TIMER && wparam == RAW_INPUT_CHECK_TIMER {
        refresh_layout(); // also catches DPI/arrangement changes without WM_DISPLAYCHANGE
        if ensure_raw_input(hwnd) {
            crate::log!("re-claimed mouse Raw Input from another part of the app");
        }
        return 0;
    }
    if msg == WM_INPUT {
        let mut raw: RAWINPUT = zeroed();
        let mut size = size_of::<RAWINPUT>() as u32;
        let n = GetRawInputData(
            lparam as HRAWINPUT,
            RID_INPUT,
            &mut raw as *mut RAWINPUT as *mut _,
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        );
        if n != u32::MAX && raw.header.dwType == RIM_TYPEMOUSE {
            let m = raw.data.mouse;
            // hDevice is null for injected input.
            let accepted = wanted(raw.header.hDevice.is_null());
            // Without the hook, buttons come from here (the switcher tracks
            // held buttons to not switch mid-drag). Nothing to block locally.
            if accepted && !mouse_hook_installed() {
                for ev in raw_buttons(u32::from(m.Anonymous.Anonymous.usButtonFlags)) {
                    dispatch(Captured::Input(ev));
                }
            }
            let relative = m.usFlags & MOUSE_MOVE_ABSOLUTE == 0;
            let (dx, dy) = (m.lLastX, m.lLastY);
            if accepted && relative && (dx != 0 || dy != 0) {
                if !grabbed() {
                    // Local use: the only question is whether we're pushing
                    // against an outer edge. Motion itself isn't needed.
                    let Some(p) = super::desktop::cursor_pos() else {
                        return DefWindowProcW(hwnd, msg, wparam, lparam);
                    };
                    if let Some(hit) = LAYOUT.with(|l| l.borrow().edge_hit(p.x, p.y, dx, dy)) {
                        dispatch(Captured::Edge(hit));
                    }
                }
                if grabbed() {
                    dispatch(Captured::Input(InputEvent::Motion { dx, dy }));
                }
            }
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

/// Button presses and releases in a Raw Input mouse report.
fn raw_buttons(flags: u32) -> impl Iterator<Item = InputEvent> {
    const BUTTONS: [(u32, u32, Button); 5] = [
        (RI_MOUSE_LEFT_BUTTON_DOWN, RI_MOUSE_LEFT_BUTTON_UP, Button::Left),
        (RI_MOUSE_RIGHT_BUTTON_DOWN, RI_MOUSE_RIGHT_BUTTON_UP, Button::Right),
        (RI_MOUSE_MIDDLE_BUTTON_DOWN, RI_MOUSE_MIDDLE_BUTTON_UP, Button::Middle),
        (RI_MOUSE_BUTTON_4_DOWN, RI_MOUSE_BUTTON_4_UP, Button::Back),
        (RI_MOUSE_BUTTON_5_DOWN, RI_MOUSE_BUTTON_5_UP, Button::Forward),
    ];
    BUTTONS.into_iter().flat_map(move |(down, up, button)| {
        let pressed = (flags & down != 0).then_some(InputEvent::Button { button, down: true });
        let released = (flags & up != 0).then_some(InputEvent::Button { button, down: false });
        pressed.into_iter().chain(released)
    })
}

const MOUSE_USAGE_PAGE: u16 = 0x01; // generic desktop
const MOUSE_USAGE: u16 = 0x02;

unsafe fn register_raw_input(hwnd: HWND) -> bool {
    let rid = RAWINPUTDEVICE {
        usUsagePage: MOUSE_USAGE_PAGE,
        usUsage: MOUSE_USAGE,
        dwFlags: RIDEV_INPUTSINK,
        hwndTarget: hwnd,
    };
    RegisterRawInputDevices(&rid, 1, size_of::<RAWINPUTDEVICE>() as u32) != 0
}

/// Windows allows one Raw Input target per device class *per process*. The
/// GUI toolkit registers its own; ours is registered later so it wins, but if
/// anything re-registers, mouse motion would silently stop reaching us.
/// Checked periodically; returns true if it had to re-claim.
unsafe fn ensure_raw_input(hwnd: HWND) -> bool {
    let mut devices: [RAWINPUTDEVICE; 16] = zeroed();
    let mut count = devices.len() as u32;
    let n = GetRegisteredRawInputDevices(devices.as_mut_ptr(), &mut count, size_of::<RAWINPUTDEVICE>() as u32);
    let ours = n != u32::MAX
        && devices[..n as usize].iter().any(|d| {
            d.usUsagePage == MOUSE_USAGE_PAGE
                && d.usUsage == MOUSE_USAGE
                && d.hwndTarget == hwnd
                && d.dwFlags & RIDEV_INPUTSINK != 0
        });
    !ours && register_raw_input(hwnd)
}

/// Everything installed on the capture thread, torn down in reverse
/// (plus the mouse hook, while grabbed).
struct Installed {
    hwnd: HWND,
    keyboard: HHOOK,
}

unsafe fn install() -> Result<Installed> {
    refresh_layout();
    let hinst = GetModuleHandleW(null());
    let class = wide("GlideCaptureWindow");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wnd_proc),
        hInstance: hinst,
        lpszClassName: class.as_ptr(),
        ..zeroed()
    };
    RegisterClassW(&wc); // fails harmlessly if already registered by an earlier start
    let hwnd = CreateWindowExW(
        0,
        class.as_ptr(),
        class.as_ptr(),
        0,
        0,
        0,
        0,
        0,
        null_mut(),
        null_mut(),
        hinst,
        null(),
    );
    if hwnd.is_null() {
        return Err(anyhow!("CreateWindowExW failed"));
    }
    if !register_raw_input(hwnd) {
        DestroyWindow(hwnd);
        return Err(anyhow!("RegisterRawInputDevices failed"));
    }
    SetTimer(hwnd, RAW_INPUT_CHECK_TIMER, RAW_INPUT_CHECK_MS, None);
    let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hinst, 0);
    let installed = Installed { hwnd, keyboard };
    sync_mouse_hook();
    if keyboard.is_null() {
        uninstall(&installed);
        return Err(anyhow!("SetWindowsHookExW failed"));
    }
    Ok(installed)
}

unsafe fn uninstall(i: &Installed) {
    if !i.keyboard.is_null() {
        UnhookWindowsHookEx(i.keyboard);
    }
    MOUSE_HOOK.with(|h| {
        if !h.get().is_null() {
            UnhookWindowsHookEx(h.replace(null_mut()));
        }
    });
    KillTimer(i.hwnd, RAW_INPUT_CHECK_TIMER);
    let rid = RAWINPUTDEVICE {
        usUsagePage: MOUSE_USAGE_PAGE,
        usUsage: MOUSE_USAGE,
        dwFlags: RIDEV_REMOVE,
        hwndTarget: null_mut(),
    };
    RegisterRawInputDevices(&rid, 1, size_of::<RAWINPUTDEVICE>() as u32);
    DestroyWindow(i.hwnd);
}

/// Starts capture on its own thread. Returns once hooks are live.
/// Dropping or stopping the handle removes them and restores normal input.
pub fn start_capture(grab: Arc<AtomicBool>, handler: Handler) -> Result<CaptureHandle> {
    super::init_dpi();
    // Development only: react to synthetic (SendInput) keys and mouse instead
    // of the physical devices, so end-to-end tests can drive a real copy.
    if std::env::var_os("GLIDE_TEST_INJECTED").is_some() {
        SELFTEST.store(true, Ordering::Relaxed);
    }
    let (ready_tx, ready_rx) = mpsc::channel::<Result<u32>>();
    let thread = std::thread::Builder::new()
        .name("glide-capture".into())
        .spawn(move || unsafe {
            super::boost_latency_thread();
            CTX.with(|c| *c.borrow_mut() = Some(Ctx { grab, handler }));
            let installed = match install() {
                Ok(i) => i,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(GetCurrentThreadId()));
            let mut msg: MSG = zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
                sync_mouse_hook(); // an edge may have switched
            }
            uninstall(&installed);
            CTX.with(|c| *c.borrow_mut() = None);
        })?;

    let thread_id = ready_rx
        .recv()
        .map_err(|_| anyhow!("capture thread died during startup"))??;
    Ok(CaptureHandle::new(move || {
        unsafe {
            PostThreadMessageW(thread_id, WM_QUIT, 0, 0);
        }
        let _ = thread.join();
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_input_buttons() {
        let evs: Vec<_> = raw_buttons(RI_MOUSE_LEFT_BUTTON_DOWN | RI_MOUSE_BUTTON_4_UP).collect();
        assert_eq!(
            evs,
            [
                InputEvent::Button {
                    button: Button::Left,
                    down: true
                },
                InputEvent::Button {
                    button: Button::Back,
                    down: false
                },
            ]
        );
        assert_eq!(raw_buttons(0).count(), 0, "plain motion");
        assert_eq!(raw_buttons(RI_MOUSE_WHEEL).count(), 0, "wheel isn't a button");
    }
}

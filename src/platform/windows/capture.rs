//! Input capture.
//!
//! WH_KEYBOARD_LL / WH_MOUSE_LL hooks see (and can swallow) every physical
//! key, button and wheel event. Mouse *motion* comes from Raw Input instead,
//! so it is unaccelerated and keeps arriving even while the cursor is frozen.
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
    /// Latest cursor position, taken free from the mouse hook's event data
    /// instead of asking the OS on every raw mouse event.
    static CURSOR: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
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
            CURSOR.with(|c| c.set((m.pt.x, m.pt.y)));
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
            let relative = m.usFlags & MOUSE_MOVE_ABSOLUTE == 0;
            let (dx, dy) = (m.lLastX, m.lLastY);
            if accepted && relative && (dx != 0 || dy != 0) {
                if !grabbed() {
                    // Local use: the only question is whether we're pushing
                    // against an outer edge. Motion itself isn't needed.
                    let (x, y) = CURSOR.with(Cell::get);
                    if let Some(hit) = LAYOUT.with(|l| l.borrow().edge_hit(x, y, dx, dy)) {
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

/// Everything installed on the capture thread, torn down in reverse.
struct Installed {
    hwnd: HWND,
    keyboard: HHOOK,
    mouse: HHOOK,
}

unsafe fn install() -> Result<Installed> {
    refresh_layout();
    if let Some(p) = super::desktop::cursor_pos() {
        CURSOR.with(|c| c.set((p.x, p.y)));
    }
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
    let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hinst, 0);
    let installed = Installed { hwnd, keyboard, mouse };
    if keyboard.is_null() || mouse.is_null() {
        uninstall(&installed);
        return Err(anyhow!("SetWindowsHookExW failed"));
    }
    Ok(installed)
}

unsafe fn uninstall(i: &Installed) {
    if !i.keyboard.is_null() {
        UnhookWindowsHookEx(i.keyboard);
    }
    if !i.mouse.is_null() {
        UnhookWindowsHookEx(i.mouse);
    }
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

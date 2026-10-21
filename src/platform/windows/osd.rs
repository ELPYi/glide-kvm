//! A brief on-screen message ("Switching locked"), shown above everything on
//! the monitor the cursor is on. Click-through, never takes focus, hides
//! itself after a moment. Used to confirm shortcuts, which otherwise act
//! invisibly (glide may be hidden in the tray, or you may be on another PC).
//!
//! Runs on its own small thread with a message loop; `show` just hands it
//! the text.

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect,
    GetMonitorInfoW, InvalidateRect, MonitorFromPoint, SelectObject, SetBkMode, SetTextColor, SetWindowRgn, DT_CENTER,
    DT_SINGLELINE, DT_VCENTER, MONITORINFO, MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::wide;

const SHOW_FOR_MS: u32 = 1300;
const HIDE_TIMER: usize = 1;
const WM_SHOW_OSD: u32 = WM_APP + 1;
const WIDTH: i32 = 300;
const HEIGHT: i32 = 56;

/// The text to show next; read by the OSD thread.
static TEXT: Mutex<String> = Mutex::new(String::new());
/// The OSD thread's id, once started.
static THREAD: OnceLock<u32> = OnceLock::new();

/// Show `text` briefly. Non-blocking; safe from any thread.
pub fn show(text: &str) {
    *TEXT.lock().unwrap_or_else(|e| e.into_inner()) = text.to_string();
    let thread = THREAD.get_or_init(spawn);
    unsafe {
        PostThreadMessageW(*thread, WM_SHOW_OSD, 0, 0);
    }
}

fn spawn() -> u32 {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("glide-osd".into())
        .spawn(move || unsafe {
            let hinst = GetModuleHandleW(null());
            let class = wide("GlideOsd");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinst,
                lpszClassName: class.as_ptr(),
                ..zeroed()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                0,
                0,
                WIDTH,
                HEIGHT,
                null_mut(),
                null_mut(),
                hinst,
                null(),
            );
            SetLayeredWindowAttributes(hwnd, 0, 235, LWA_ALPHA);
            let _ = tx.send(GetCurrentThreadId());
            let mut msg: MSG = zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                if msg.message == WM_SHOW_OSD {
                    present(hwnd);
                } else {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        })
        .expect("spawn osd thread");
    rx.recv().unwrap_or(0)
}

/// Size for the monitor's DPI, place at the bottom-center of the monitor
/// the cursor is on, show without activating, and start the hide timer.
unsafe fn present(hwnd: HWND) {
    let mut cursor = POINT { x: 0, y: 0 };
    GetCursorPos(&mut cursor);
    let mut mi: MONITORINFO = zeroed();
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    GetMonitorInfoW(MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST), &mut mi);
    let work = mi.rcWork;
    let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
    let (w, h) = ((WIDTH as f32 * scale) as i32, (HEIGHT as f32 * scale) as i32);
    let x = work.left + (work.right - work.left - w) / 2;
    let y = work.bottom - h - (80.0 * scale) as i32;
    SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    let radius = (16.0 * scale) as i32;
    SetWindowRgn(hwnd, CreateRoundRectRgn(0, 0, w + 1, h + 1, radius, radius), 1);
    InvalidateRect(hwnd, null(), 1);
    SetTimer(hwnd, HIDE_TIMER, SHOW_FOR_MS, None);
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            let background = CreateSolidBrush(rgb(0x22, 0x24, 0x2A));
            FillRect(dc, &rect, background);
            DeleteObject(background);

            let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
            let face = wide("Segoe UI Semibold");
            let font = CreateFontW(
                -(17.0 * scale) as i32,
                0,
                0,
                0,
                600,
                0,
                0,
                0,
                0,
                0,
                0,
                5, // CLEARTYPE_QUALITY
                0,
                face.as_ptr(),
            );
            let old = SelectObject(dc, font);
            SetBkMode(dc, TRANSPARENT as i32);
            SetTextColor(dc, rgb(0xF2, 0xF4, 0xF8));
            let text = TEXT.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let mut wtext: Vec<u16> = text.encode_utf16().collect();
            DrawTextW(
                dc,
                wtext.as_mut_ptr(),
                wtext.len() as i32,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
            SelectObject(dc, old);
            DeleteObject(font);
            EndPaint(hwnd, &ps);
            0
        }
        WM_TIMER if wparam == HIDE_TIMER => {
            KillTimer(hwnd, HIDE_TIMER);
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        // Never activate or take clicks.
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

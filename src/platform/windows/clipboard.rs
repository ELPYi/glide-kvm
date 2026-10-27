//! This PC's clipboard: notices every copy (a clipboard listener on a
//! hidden window, so it sleeps between copies) and puts clipboards from
//! the other PCs on it.
//!
//! Shared formats: Unicode text, "HTML Format" and a DIB picture (Windows
//! makes the other image formats from it). Copies that password managers
//! mark as private (Windows' clipboard-history opt-outs) are never read.

use std::ptr::{null, null_mut};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use windows_sys::Win32::Foundation::{GlobalFree, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardOwner,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, RemoveClipboardFormatListener,
    SetClipboardData,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::wide;
use crate::clip::sync::{Clipboard, OnCopy};
use crate::clip::{ClipData, MAX_SIZE};
use crate::log;

/// Asks the clipboard thread to apply the pending clipboard.
const WM_APPLY: u32 = WM_APP + 1;

struct Formats {
    html: u32,
    /// Password managers put one of these on the clipboard with a secret.
    private: [u32; 2],
    /// "CanIncludeInClipboardHistory": a DWORD, 0 = keep out of history.
    history: u32,
}

impl Formats {
    fn register() -> Formats {
        let reg = |name: &str| unsafe { RegisterClipboardFormatW(wide(name).as_ptr()) };
        Formats {
            html: reg("HTML Format"),
            private: [
                reg("ExcludeClipboardContentFromMonitorProcessing"),
                reg("Clipboard Viewer Ignore"),
            ],
            history: reg("CanIncludeInClipboardHistory"),
        }
    }
}

struct Shared {
    pending: Mutex<Option<ClipData>>,
}

pub struct WinClipboard {
    shared: Arc<Shared>,
    thread_id: u32,
    hwnd: isize,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Clipboard for WinClipboard {
    fn set(&self, data: ClipData) {
        *self.shared.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(data);
        unsafe {
            PostMessageW(self.hwnd as HWND, WM_APPLY, 0, 0);
        }
    }
}

impl Drop for WinClipboard {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

thread_local! {
    static CTX: std::cell::RefCell<Option<(OnCopy, Arc<Shared>, Formats)>> = const { std::cell::RefCell::new(None) };
}

pub fn open_clipboard(on_copy: OnCopy) -> Result<Box<dyn Clipboard>> {
    let shared = Arc::new(Shared {
        pending: Mutex::new(None),
    });
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(u32, isize)>>();
    let s = shared.clone();
    let thread = std::thread::Builder::new()
        .name("glide-clipboard-os".into())
        .spawn(move || unsafe {
            let hinst = GetModuleHandleW(null());
            let class = wide("GlideClipboardWindow");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinst,
                lpszClassName: class.as_ptr(),
                ..std::mem::zeroed()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                hinst,
                null(),
            );
            if hwnd.is_null() || AddClipboardFormatListener(hwnd) == 0 {
                let _ = ready_tx.send(Err(anyhow!("couldn't watch the clipboard")));
                return;
            }
            CTX.with(|c| *c.borrow_mut() = Some((on_copy, s, Formats::register())));
            let _ = ready_tx.send(Ok((GetCurrentThreadId(), hwnd as isize)));
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                DispatchMessageW(&msg);
            }
            RemoveClipboardFormatListener(hwnd);
            DestroyWindow(hwnd);
            CTX.with(|c| *c.borrow_mut() = None);
        })?;
    let (thread_id, hwnd) = ready_rx.recv().map_err(|_| anyhow!("clipboard thread died"))??;
    Ok(Box::new(WinClipboard {
        shared,
        thread_id,
        hwnd,
        thread: Some(thread),
    }))
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Our own `set` makes us the owner: that change came from another PC.
        WM_CLIPBOARDUPDATE if GetClipboardOwner() != hwnd => {
            CTX.with(|c| {
                if let Some((on_copy, _, formats)) = c.borrow().as_ref() {
                    if let Some(d) = read(hwnd, formats) {
                        on_copy(d);
                    }
                }
            });
            0
        }
        WM_APPLY => {
            CTX.with(|c| {
                if let Some((_, shared, formats)) = c.borrow().as_ref() {
                    let pending = shared.pending.lock().unwrap_or_else(|e| e.into_inner()).take();
                    if let Some(d) = pending {
                        if !write(hwnd, formats, &d) {
                            log!("couldn't put the other PC's clipboard on this one (another app has it open)");
                        }
                    }
                }
            });
            0
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Another app may have the clipboard open for a moment: retry briefly.
unsafe fn open(hwnd: HWND) -> bool {
    for _ in 0..20 {
        if OpenClipboard(hwnd) != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

unsafe fn read(hwnd: HWND, f: &Formats) -> Option<ClipData> {
    if !open(hwnd) {
        return None;
    }
    let d = (|| {
        if f.private.iter().any(|&p| p != 0 && IsClipboardFormatAvailable(p) != 0) {
            return None;
        }
        if let Some(h) = get(f.history) {
            if h.len() >= 4 && u32::from_le_bytes([h[0], h[1], h[2], h[3]]) == 0 {
                return None;
            }
        }
        let text = get(u32::from(CF_UNICODETEXT)).map(|b| {
            let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
            let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
            String::from_utf16_lossy(&units[..end])
        });
        Some(ClipData {
            text,
            html: get(f.html),
            image: get(u32::from(CF_DIB)),
        })
    })();
    CloseClipboard();
    d.filter(|d| !d.is_empty() && d.size() <= MAX_SIZE)
}

/// One format's bytes, if present (and not absurdly large).
unsafe fn get(format: u32) -> Option<Vec<u8>> {
    if format == 0 || IsClipboardFormatAvailable(format) == 0 {
        return None;
    }
    let h = GetClipboardData(format);
    if h.is_null() {
        return None;
    }
    let size = GlobalSize(h);
    if size == 0 || size > MAX_SIZE {
        return None;
    }
    let p = GlobalLock(h) as *const u8;
    if p.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(p, size).to_vec();
    GlobalUnlock(h);
    Some(bytes)
}

unsafe fn write(hwnd: HWND, f: &Formats, d: &ClipData) -> bool {
    if !open(hwnd) {
        return false;
    }
    EmptyClipboard();
    if let Some(t) = &d.text {
        let bytes: Vec<u8> = t.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        put(u32::from(CF_UNICODETEXT), &bytes);
    }
    if let Some(h) = &d.html {
        put(f.html, h);
    }
    if let Some(i) = &d.image {
        put(u32::from(CF_DIB), i);
    }
    CloseClipboard();
    true
}

unsafe fn put(format: u32, bytes: &[u8]) {
    let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
    if h.is_null() {
        return;
    }
    let p = GlobalLock(h) as *mut u8;
    if p.is_null() {
        GlobalFree(h);
        return;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    GlobalUnlock(h);
    // On success the clipboard owns the memory; on failure it's still ours.
    if SetClipboardData(format, h as HANDLE).is_null() {
        GlobalFree(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// Real clipboard: a copy made by another app is reported; glide's own
    /// write isn't (it would bounce back to the other PC); the write lands.
    /// Restores what was on the clipboard.
    #[test]
    #[ignore = "uses this PC's real clipboard"]
    fn sees_copies_and_writes_without_echo() {
        let seen = Arc::new(Mutex::new(Vec::<ClipData>::new()));
        let s = seen.clone();
        let board = open_clipboard(Box::new(move |d| s.lock().unwrap().push(d))).unwrap();

        // "Another app" copies (through a separate window of its own).
        let other = open_clipboard(Box::new(|_| {})).unwrap();
        let before = unsafe {
            let w = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                null_mut(),
                null(),
            );
            let f = Formats::register();
            let old = read(w, &f);
            DestroyWindow(w);
            old
        };
        let copied = ClipData {
            text: Some("glide clipboard test ✓".into()),
            ..Default::default()
        };
        other.set(copied.clone());
        let t = Instant::now();
        while seen.lock().unwrap().is_empty() && t.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            seen.lock().unwrap().first(),
            Some(&copied),
            "another app's copy is seen"
        );

        // glide writes: not reported back.
        seen.lock().unwrap().clear();
        board.set(ClipData {
            text: Some("from the other PC".into()),
            ..Default::default()
        });
        std::thread::sleep(Duration::from_millis(400));
        assert!(seen.lock().unwrap().is_empty(), "own write not echoed");

        if let Some(b) = before {
            other.set(b);
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

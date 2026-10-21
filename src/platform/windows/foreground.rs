//! What the focused app looks like, for game detection (see `crate::game`).
//! A handful of cheap queries, polled a few times a second; nothing here
//! touches the app itself.

use std::mem::{size_of, zeroed};

use windows_sys::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_RUNNING_D3D_FULL_SCREEN};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClipCursor, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsZoomed,
};

use crate::game::Foreground;

/// The focused app, or `None` if nothing (or glide itself) is in focus.
pub fn foreground() -> Option<Foreground> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 || pid == unsafe { GetCurrentProcessId() } {
        return None;
    }
    Some(Foreground {
        app: process_name(pid).unwrap_or_default(),
        fullscreen: covers_its_monitor(hwnd),
        exclusive_fullscreen: exclusive_fullscreen(),
        cursor_confined: cursor_confined(hwnd),
    })
}

/// "C:\...\Game.exe" -> "game.exe". Works for most elevated processes too
/// (limited-information access), but not protected ones.
fn process_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(h);
        ok.then(|| crate::game::normalize_app(&String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// The window fills its entire monitor (taskbar included). A maximized
/// window doesn't count: that's normal desktop use, even with an auto-hiding
/// taskbar that lets it reach the bottom of the screen.
fn covers_its_monitor(hwnd: HWND) -> bool {
    unsafe {
        if IsZoomed(hwnd) != 0 {
            return false;
        }
        let mut w: RECT = zeroed();
        if GetWindowRect(hwnd, &mut w) == 0 {
            return false;
        }
        let mut mi: MONITORINFO = zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi) == 0 {
            return false;
        }
        let m = mi.rcMonitor;
        w.left <= m.left && w.top <= m.top && w.right >= m.right && w.bottom >= m.bottom
    }
}

/// Windows reports a Direct3D app running in exclusive fullscreen.
fn exclusive_fullscreen() -> bool {
    let mut state = 0;
    unsafe { SHQueryUserNotificationState(&mut state) == 0 && state == QUNS_RUNNING_D3D_FULL_SCREEN }
}

/// The focused window has trapped the cursor inside itself (windowed games
/// do this so the mouse can't wander out of their window).
///
/// Counted only when the cursor is confined to part of a monitor, within the
/// focused window. A game in the background can leave the cursor confined to
/// a whole monitor long after you've switched away from it; that's not "in a
/// game". (Fullscreen games are caught by the fullscreen check instead.)
fn cursor_confined(hwnd: HWND) -> bool {
    /// Window rects include a few invisible resize-border pixels.
    const SLACK: i32 = 16;
    unsafe {
        let mut clip: RECT = zeroed();
        let mut win: RECT = zeroed();
        let mut mi: MONITORINFO = zeroed();
        mi.cbSize = size_of::<MONITORINFO>() as u32;
        if GetClipCursor(&mut clip) == 0
            || GetWindowRect(hwnd, &mut win) == 0
            || GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi) == 0
        {
            return false;
        }
        let m = mi.rcMonitor;
        let smaller_than_monitor =
            clip.right - clip.left < m.right - m.left || clip.bottom - clip.top < m.bottom - m.top;
        let inside_window = clip.left >= win.left - SLACK
            && clip.top >= win.top - SLACK
            && clip.right <= win.right + SLACK
            && clip.bottom <= win.bottom + SLACK;
        smaller_than_monitor && inside_window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whatever is focused while tests run (usually a terminal): it's a
    /// real, named app, and an ordinary window isn't treated as a game.
    #[test]
    fn reads_the_focused_app() {
        if let Some(fg) = foreground() {
            assert!(fg.app.is_empty() || fg.app.ends_with(".exe"), "{fg:?}");
            assert!(
                !fg.cursor_confined,
                "the terminal running tests doesn't trap the cursor"
            );
        }
    }
}

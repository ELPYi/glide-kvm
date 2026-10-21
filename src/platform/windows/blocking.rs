//! Can this PC accept injected input right now?
//!
//! Windows silently discards SendInput in two common situations, without any
//! error. Detecting them lets the sharing PC take control back instead of
//! leaving the cursor stuck in limbo:
//!
//! - the **secure desktop** is showing (lock screen, sign-in, UAC prompt,
//!   Ctrl+Alt+Del): no ordinary program can send input there;
//! - an **administrator window** is in front while glide isn't elevated
//!   (Windows' UIPI rule).

use std::mem::size_of;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_READOBJECTS, UOI_NAME,
};
use windows_sys::Win32::System::Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use crate::input::InputBlock;

/// `elevated`: whether this process runs as administrator.
pub fn input_block(elevated: bool) -> Option<InputBlock> {
    if !on_default_desktop() {
        return Some(InputBlock::SecureScreen);
    }
    if !elevated && foreground_is_elevated() {
        return Some(InputBlock::AdminWindow);
    }
    None
}

/// Is the desktop receiving input the normal one ("Default")? The lock
/// screen, UAC and Ctrl+Alt+Del use "Winlogon", which we can't even open.
fn on_default_desktop() -> bool {
    unsafe {
        let desk = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if desk.is_null() {
            return false;
        }
        let mut name = [0u16; 64];
        let mut needed = 0u32;
        let ok = GetUserObjectInformationW(
            desk as HANDLE,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            (name.len() * 2) as u32,
            &mut needed,
        ) != 0;
        CloseDesktop(desk);
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        !ok || String::from_utf16_lossy(&name[..len]).eq_ignore_ascii_case("Default")
    }
}

/// Does the foreground window belong to an elevated (admin) process?
fn foreground_is_elevated() -> bool {
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, &mut pid);
        pid != 0 && pid != std::process::id() && process_is_elevated(pid)
    }
}

/// Is process `pid` running as administrator?
fn process_is_elevated(pid: u32) -> bool {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false; // can't tell: assume fine rather than block wrongly
        }
        let mut token: HANDLE = null_mut();
        let elevated = if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            // A normal process isn't allowed to inspect an admin process's token.
            true
        } else {
            let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                &mut e as *mut TOKEN_ELEVATION as *mut _,
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            ) != 0;
            CloseHandle(token);
            ok && e.TokenIsElevated != 0
        };
        CloseHandle(process);
        elevated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_run_without_crashing() {
        // The answer depends on what's on screen; it must just work and be quick.
        let t = std::time::Instant::now();
        for _ in 0..100 {
            let _ = input_block(false);
        }
        assert!(
            t.elapsed() < std::time::Duration::from_millis(200),
            "too slow: {:?}",
            t.elapsed()
        );
        assert!(on_default_desktop(), "tests run on the normal desktop");
    }
}

#[cfg(test)]
mod live {
    /// Prints what this PC reports right now. Run with --ignored --nocapture.
    #[test]
    #[ignore]
    fn report_current_state() {
        let fg = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        eprintln!("foreground window: {fg:?}");
        eprintln!("input_block (as a normal process): {:?}", super::input_block(false));
        eprintln!("input_block (if we were admin):     {:?}", super::input_block(true));
        if let Ok(pid) = std::env::var("CHECK_PID") {
            let pid: u32 = pid.parse().unwrap();
            eprintln!("process {pid} elevated: {}", super::process_is_elevated(pid));
            eprintln!(
                "this test process elevated: {}",
                super::process_is_elevated(std::process::id())
            );
        }
    }
}

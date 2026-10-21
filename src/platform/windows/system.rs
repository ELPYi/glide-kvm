//! OS integration that isn't input: launch at sign-in, firewall, clock.

use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::ptr::null_mut;

use anyhow::{bail, Context, Result};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    IsIconic, SetForegroundWindow, ShowWindow, SW_HIDE, SW_RESTORE, SW_SHOW,
};

use super::wide;

pub use super::admin::{
    admin_task_installed, admin_task_starts_at_sign_in, install_admin_task, is_elevated, remove_admin_task,
    run_admin_task, run_elevated, ADMIN_TASK, FROM_TASK_ARG,
};
pub use super::instance::{claim_single_instance, find_running_window, InstanceGuard};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "glide";
const FIREWALL_RULE: &str = "glide";

/// Height of the primary screen minus the taskbar, in the same scaled
/// units the UI uses (call before the window exists).
pub fn work_area_height() -> Option<f32> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETWORKAREA};
    let mut r: windows_sys::Win32::Foundation::RECT = unsafe { zeroed() };
    let ok = unsafe { SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut r as *mut _ as *mut _, 0) } != 0;
    ok.then(|| (r.bottom - r.top) as f32)
}

/// Hide a window (to the tray). Paired with [`show_window`].
pub fn hide_window(handle: isize) {
    unsafe {
        ShowWindow(handle as windows_sys::Win32::Foundation::HWND, SW_HIDE);
    }
}

/// Bring a (possibly hidden or minimized) window back to the front. Used by
/// the tray icon, which runs while the app's own frame loop is paused.
pub fn show_window(handle: isize) {
    let hwnd = handle as windows_sys::Win32::Foundation::HWND;
    unsafe {
        ShowWindow(hwnd, if IsIconic(hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
        SetForegroundWindow(hwnd);
    }
}

/// Whether the window is currently shown (not hidden in the tray).
pub fn window_visible(handle: isize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    unsafe { IsWindowVisible(handle as windows_sys::Win32::Foundation::HWND) != 0 }
}

/// Time since the last keyboard or mouse input on this PC (including input
/// glide injects while another PC controls this one).
pub fn idle_for() -> std::time::Duration {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return std::time::Duration::ZERO;
    }
    let now = unsafe { GetTickCount() };
    std::time::Duration::from_millis(now.wrapping_sub(info.dwTime) as u64)
}

/// A brief on-screen message (e.g. "Updated to 0.4.12").
pub fn show_osd(text: &str) {
    super::osd::show(text);
}

/// Local wall-clock time (h, m, s).
pub fn local_time() -> (u8, u8, u8) {
    let mut t = unsafe { zeroed() };
    unsafe { GetLocalTime(&mut t) };
    (t.wHour as u8, t.wMinute as u8, t.wSecond as u8)
}

/// The desktop app's exe (even when called from glide-cli, which sits beside it).
fn app_exe() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    Ok(exe.with_file_name("glide.exe"))
}

fn autostart_command() -> Result<String> {
    Ok(format!("\"{}\" --minimized", app_exe()?.display()))
}

fn read_run_value() -> Option<String> {
    let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let chars = (len as usize / 2).saturating_sub(1); // drop the NUL
    Some(String::from_utf16_lossy(&buf[..chars]))
}

/// True if glide is registered to launch at sign-in *from this location*.
pub fn autostart_enabled() -> bool {
    match (read_run_value(), autostart_command()) {
        (Some(current), Ok(ours)) => current.eq_ignore_ascii_case(&ours),
        _ => false,
    }
}

/// Register (or unregister) launching the app, minimized, at sign-in.
/// Per-user; no admin needed.
pub fn set_autostart(enabled: bool) -> Result<()> {
    let (key, value) = (wide(RUN_KEY), wide(RUN_VALUE));
    let status = unsafe {
        if enabled {
            let data = wide(&autostart_command()?);
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        } else {
            match RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) {
                2 => ERROR_SUCCESS, // ERROR_FILE_NOT_FOUND: already off
                s => s,
            }
        }
    };
    if status != ERROR_SUCCESS {
        bail!("couldn't update the startup entry (error {status})");
    }
    Ok(())
}

/// Add an inbound firewall rule for glide's UDP port. Shows a UAC prompt and
/// blocks until it's answered, so call it off the UI thread.
///
/// Normally unnecessary: clients knock outbound first, which the firewall's
/// stateful filtering treats as permission for the replies.
pub fn open_firewall(port: u16) -> Result<()> {
    let params = format!(
        "/c netsh advfirewall firewall delete rule name=\"{FIREWALL_RULE}\" >nul & \
         netsh advfirewall firewall add rule name=\"{FIREWALL_RULE}\" dir=in action=allow protocol=UDP localport={port} profile=any"
    );
    let (verb, file, params) = (wide("runas"), wide("cmd.exe"), wide(&params));
    let mut info: SHELLEXECUTEINFOW = unsafe { zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = SW_HIDE;
    unsafe {
        if ShellExecuteExW(&mut info) == 0 {
            bail!("the administrator prompt was declined");
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        if code != 0 {
            return Err(anyhow::anyhow!("netsh exited with code {code}")).context("adding the firewall rule failed");
        }
    }
    Ok(())
}

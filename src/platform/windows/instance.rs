//! One glide at a time. Launching glide while it's already running (say,
//! hidden in the tray) brings the existing window forward instead.

use std::path::Path;
use std::ptr::null;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, HWND, LPARAM,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowTextW, GetWindowThreadProcessId};

use super::wide;

const MUTEX_NAME: &str = "Local\\glide-app-instance";
const WINDOW_TITLE: &str = "glide";

/// Proof that this is the running glide; released when dropped.
pub struct InstanceGuard(isize);

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0 as _);
        }
    }
}

/// Claim "the running glide". `None` if another copy already has it.
/// Keep the guard alive for as long as this copy should count as running.
pub fn claim_single_instance() -> Option<InstanceGuard> {
    // Development only: GLIDE_INSTANCE gives a test copy its own instance
    // name so it can run alongside your real glide.
    let name = match std::env::var("GLIDE_INSTANCE") {
        Ok(suffix) => wide(&format!("{MUTEX_NAME}-{suffix}")),
        Err(_) => wide(MUTEX_NAME),
    };
    unsafe {
        let handle = CreateMutexW(null(), 0, name.as_ptr());
        let err = GetLastError();
        // An elevated copy's mutex can't be opened from a normal one: that
        // also means "already running".
        if err == ERROR_ALREADY_EXISTS || (handle.is_null() && err == ERROR_ACCESS_DENIED) {
            if !handle.is_null() {
                CloseHandle(handle);
            }
            return None;
        }
        (!handle.is_null()).then_some(InstanceGuard(handle as isize))
    }
}

/// The window of another running glide, if any.
pub fn find_running_window() -> Option<isize> {
    let mut found: isize = 0;
    unsafe {
        EnumWindows(Some(visit), &mut found as *mut isize as LPARAM);
    }
    (found != 0).then_some(found)
}

unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let mut title = [0u16; 16];
    let n = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
    if n <= 0 || String::from_utf16_lossy(&title[..n as usize]) != WINDOW_TITLE {
        return 1; // keep looking
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == std::process::id() || !is_glide_process(pid) {
        return 1;
    }
    *(lparam as *mut isize) = hwnd as isize;
    0 // found it, stop
}

/// Is `pid` a glide.exe? (Guards against, e.g., an Explorer window showing a folder named "glide".)
fn is_glide_process(pid: u32) -> bool {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        CloseHandle(process);
        ok && Path::new(&String::from_utf16_lossy(&buf[..len as usize]))
            .file_name()
            .is_some_and(|f| f.eq_ignore_ascii_case("glide.exe"))
    }
}

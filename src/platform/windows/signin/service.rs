//! The Windows service: runs as SYSTEM in session 0, where it can't show or
//! inject anything itself, and keeps one helper running in the session at
//! the screen (a new one after switching user or signing out).

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use anyhow::{bail, Result};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, NO_ERROR, WAIT_TIMEOUT};
use windows_sys::Win32::Security::{
    DuplicateTokenEx, SecurityImpersonation, SetTokenInformation, TokenPrimary, TokenSessionId, TOKEN_ALL_ACCESS,
};
use windows_sys::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows_sys::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW, SERVICE_ACCEPT_SHUTDOWN,
    SERVICE_ACCEPT_STOP, SERVICE_CONTROL_INTERROGATE, SERVICE_CONTROL_SHUTDOWN, SERVICE_CONTROL_STOP, SERVICE_RUNNING,
    SERVICE_STATUS, SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE, SERVICE_STOPPED, SERVICE_STOP_PENDING,
    SERVICE_TABLE_ENTRYW, SERVICE_WIN32_OWN_PROCESS,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, SetEvent, TerminateProcess,
    WaitForSingleObject, CREATE_NO_WINDOW, PROCESS_INFORMATION, STARTUPINFOW,
};

use super::{log, SERVICE_NAME};

/// How often to check the helper is still there, in the right session.
const CHECK_EVERY: u32 = 1000; // ms
const NO_SESSION: u32 = 0xFFFF_FFFF;

static STATUS: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(null_mut());
static STOP: AtomicPtr<core::ffi::c_void> = AtomicPtr::new(null_mut());
static STOPPING: AtomicBool = AtomicBool::new(false);

/// Hand this process to the service manager (only works when it started us).
pub fn run() -> Result<()> {
    let mut name = super::super::wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: null_mut(),
            lpServiceProc: None,
        },
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        bail!("not started as a service ({})", unsafe { GetLastError() });
    }
    Ok(())
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    let name = super::super::wide(SERVICE_NAME);
    let status = RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(control), null());
    if status.is_null() {
        return;
    }
    STATUS.store(status, Ordering::SeqCst);
    let stop = CreateEventW(null(), 1, 0, null());
    STOP.store(stop, Ordering::SeqCst);
    report(SERVICE_RUNNING);
    log("service started");

    let mut helper = Helper::default();
    while !STOPPING.load(Ordering::SeqCst) {
        helper.keep_running();
        if WaitForSingleObject(stop, CHECK_EVERY) != WAIT_TIMEOUT {
            break;
        }
    }
    helper.stop();
    log("service stopped");
    report(SERVICE_STOPPED);
}

unsafe extern "system" fn control(code: u32, _: u32, _: *mut core::ffi::c_void, _: *mut core::ffi::c_void) -> u32 {
    match code {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            STOPPING.store(true, Ordering::SeqCst);
            report(SERVICE_STOP_PENDING);
            let stop = STOP.load(Ordering::SeqCst);
            if !stop.is_null() {
                SetEvent(stop);
            }
            NO_ERROR
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR,
        _ => 120, // ERROR_CALL_NOT_IMPLEMENTED
    }
}

fn report(state: SERVICE_STATUS_CURRENT_STATE) {
    let status: SERVICE_STATUS_HANDLE = STATUS.load(Ordering::SeqCst);
    let mut s: SERVICE_STATUS = unsafe { zeroed() };
    s.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
    s.dwCurrentState = state;
    s.dwControlsAccepted = if state == SERVICE_RUNNING {
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
    } else {
        0
    };
    s.dwWaitHint = 3000;
    unsafe { SetServiceStatus(status, &s) };
}

/// The helper process we started, and in which session.
#[derive(Default)]
struct Helper {
    process: Option<HANDLE>,
    session: u32,
}

impl Helper {
    fn keep_running(&mut self) {
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        if session == NO_SESSION {
            return; // between sessions: nothing at the screen yet
        }
        let alive = self
            .process
            .is_some_and(|p| unsafe { WaitForSingleObject(p, 0) } == WAIT_TIMEOUT);
        if alive && self.session == session {
            return;
        }
        if alive {
            log(&format!("session {} is at the screen now", session));
        } else if self.process.is_some() {
            log("the helper stopped; starting it again");
        }
        self.stop();
        match start_helper(session) {
            Ok(p) => {
                self.process = Some(p);
                self.session = session;
            }
            Err(e) => log(&format!("couldn't start the helper in session {session}: {e:#}")),
        }
    }

    fn stop(&mut self) {
        if let Some(p) = self.process.take() {
            unsafe {
                TerminateProcess(p, 0);
                CloseHandle(p);
            }
        }
    }
}

/// Start `glide-secure.exe --helper` as SYSTEM in `session`, on the
/// sign-in desktop (it moves to whichever desktop has the input).
fn start_helper(session: u32) -> Result<HANDLE> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().map(|d| d.display().to_string()).unwrap_or_default();
    let app = super::super::wide(&exe.display().to_string());
    let mut command = super::super::wide(&format!("\"{}\" --helper", exe.display()));
    let mut desktop = super::super::wide(r"winsta0\winlogon");
    let dir = super::super::wide(&dir);
    unsafe {
        let mut own: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut own) == 0 {
            bail!("couldn't read the service's token ({})", GetLastError());
        }
        let mut token: HANDLE = null_mut();
        let duplicated = DuplicateTokenEx(
            own,
            TOKEN_ALL_ACCESS,
            null(),
            SecurityImpersonation,
            TokenPrimary,
            &mut token,
        );
        CloseHandle(own);
        if duplicated == 0 {
            bail!("couldn't copy the service's token ({})", GetLastError());
        }
        let set = SetTokenInformation(
            token,
            TokenSessionId,
            (&session as *const u32).cast(),
            size_of::<u32>() as u32,
        );
        if set == 0 {
            let e = GetLastError();
            CloseHandle(token);
            bail!("couldn't move the token to session {session} ({e})");
        }
        let mut si: STARTUPINFOW = zeroed();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        si.lpDesktop = desktop.as_mut_ptr();
        let mut pi: PROCESS_INFORMATION = zeroed();
        let ok = CreateProcessAsUserW(
            token,
            app.as_ptr(),
            command.as_mut_ptr(),
            null(),
            null(),
            0,
            CREATE_NO_WINDOW,
            null(),
            dir.as_ptr(),
            &si,
            &mut pi,
        );
        let e = GetLastError();
        CloseHandle(token);
        if ok == 0 {
            bail!("couldn't start it ({e})");
        }
        CloseHandle(pi.hThread);
        Ok(pi.hProcess)
    }
}

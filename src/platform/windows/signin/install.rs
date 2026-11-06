//! Installing, updating and removing the service. Installing and removing
//! need administrator rights (glide runs them elevated).
//!
//! The service runs `C:\Program Files\glide\glide-secure.exe`. Only
//! administrators can change that folder, and what goes in it is always a
//! copy glide checked against the signed release manifest (or, in a local
//! build, the one built next to glide.exe).

use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Services::{
    ChangeServiceConfig2W, ChangeServiceConfigW, CloseServiceHandle, ControlService, CreateServiceW, DeleteService,
    OpenSCManagerW, OpenServiceW, QueryServiceStatus, StartServiceW, SC_ACTION, SC_ACTION_RESTART, SC_HANDLE,
    SC_MANAGER_CONNECT, SC_MANAGER_CREATE_SERVICE, SERVICE_ALL_ACCESS, SERVICE_AUTO_START, SERVICE_CONFIG_DESCRIPTION,
    SERVICE_CONFIG_FAILURE_ACTIONS, SERVICE_CONTROL_STOP, SERVICE_DESCRIPTIONW, SERVICE_ERROR_NORMAL,
    SERVICE_FAILURE_ACTIONSW, SERVICE_QUERY_STATUS, SERVICE_STATUS, SERVICE_STOPPED, SERVICE_WIN32_OWN_PROCESS,
};

use super::{install_dir, SERVICE_DESCRIPTION, SERVICE_DISPLAY_NAME, SERVICE_NAME};
use crate::log;
use crate::update::{sha256_hex, SECURE_EXE};

/// Is the service installed? (Anyone may ask.)
pub fn installed() -> bool {
    unsafe {
        let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
        if scm.is_null() {
            return false;
        }
        let svc = OpenServiceW(scm, super::super::wide(SERVICE_NAME).as_ptr(), SERVICE_QUERY_STATUS);
        let found = !svc.is_null();
        if found {
            CloseServiceHandle(svc);
        }
        CloseServiceHandle(scm);
        found
    }
}

/// Install (or reinstall) the service and start it. Needs administrator.
pub fn install() -> Result<()> {
    let program = trusted_program()?;
    let dir = install_dir().context("couldn't find the Program Files folder")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let exe = dir.join(SECURE_EXE);
    unsafe {
        let scm = Scm::open(SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE)?;
        let name = super::super::wide(SERVICE_NAME);
        let existing = Handle(OpenServiceW(scm.0, name.as_ptr(), SERVICE_ALL_ACCESS));
        if !existing.0.is_null() {
            stop(existing.0);
        }
        write_program(&exe, &program)?;
        let command = super::super::wide(&format!("\"{}\" --service", exe.display()));
        let display = super::super::wide(SERVICE_DISPLAY_NAME);
        let svc = if existing.0.is_null() {
            let svc = Handle(CreateServiceW(
                scm.0,
                name.as_ptr(),
                display.as_ptr(),
                SERVICE_ALL_ACCESS,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                command.as_ptr(),
                null(),
                null_mut(),
                null(),
                null(), // LocalSystem
                null(),
            ));
            if svc.0.is_null() {
                bail!("couldn't create the service ({})", GetLastError());
            }
            svc
        } else {
            let changed = ChangeServiceConfigW(
                existing.0,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                command.as_ptr(),
                null(),
                null_mut(),
                null(),
                null(),
                null(),
                display.as_ptr(),
            );
            if changed == 0 {
                bail!("couldn't update the service ({})", GetLastError());
            }
            existing
        };
        let mut description = super::super::wide(SERVICE_DESCRIPTION);
        let d = SERVICE_DESCRIPTIONW {
            lpDescription: description.as_mut_ptr(),
        };
        ChangeServiceConfig2W(
            svc.0,
            SERVICE_CONFIG_DESCRIPTION,
            (&d as *const SERVICE_DESCRIPTIONW).cast(),
        );
        // If it ever crashes, Windows starts it again.
        let mut actions = [SC_ACTION {
            Type: SC_ACTION_RESTART,
            Delay: 2000,
        }; 3];
        let f = SERVICE_FAILURE_ACTIONSW {
            dwResetPeriod: 24 * 60 * 60,
            lpRebootMsg: null_mut(),
            lpCommand: null_mut(),
            cActions: actions.len() as u32,
            lpsaActions: actions.as_mut_ptr(),
        };
        ChangeServiceConfig2W(
            svc.0,
            SERVICE_CONFIG_FAILURE_ACTIONS,
            (&f as *const SERVICE_FAILURE_ACTIONSW).cast(),
        );
        if StartServiceW(svc.0, 0, null()) == 0 && GetLastError() != 1056 {
            // 1056: already running
            bail!("couldn't start the service ({})", GetLastError());
        }
    }
    log!("sign-in screen support installed");
    Ok(())
}

/// Stop and remove the service and its program. Needs administrator.
pub fn remove() -> Result<()> {
    unsafe {
        let scm = Scm::open(SC_MANAGER_CONNECT)?;
        let svc = Handle(OpenServiceW(
            scm.0,
            super::super::wide(SERVICE_NAME).as_ptr(),
            SERVICE_ALL_ACCESS,
        ));
        if !svc.0.is_null() {
            stop(svc.0);
            if DeleteService(svc.0) == 0 && GetLastError() != 1072 {
                // 1072: already marked for deletion
                bail!("couldn't remove the service ({})", GetLastError());
            }
        }
    }
    if let Some(dir) = install_dir() {
        let exe = dir.join(SECURE_EXE);
        retry(|| match std::fs::remove_file(&exe) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        })
        .with_context(|| format!("removing {}", exe.display()))?;
        let _ = std::fs::remove_file(dir.join("glide-secure.log"));
        let _ = std::fs::remove_dir(&dir); // only if empty
    }
    log!("sign-in screen support removed");
    Ok(())
}

/// After an update: if the installed service is an older version, replace
/// it. Runs in glide when it's elevated; does nothing otherwise.
pub fn refresh() {
    if !super::super::admin::is_elevated() || !installed() {
        return;
    }
    let Some(exe) = install_dir().map(|d| d.join(SECURE_EXE)) else {
        return;
    };
    let installed_sha = std::fs::read(&exe).map(|b| sha256_hex(&b)).unwrap_or_default();
    let local_sha = std::fs::read(local_program()).map(|b| sha256_hex(&b)).ok();
    if local_sha.as_deref() == Some(installed_sha.as_str()) || !crate::is_release_build() {
        return; // the same as this version's (or a local build: leave it)
    }
    match trusted_program() {
        Ok(program) if sha256_hex(&program) == installed_sha => {}
        Ok(_) => match install() {
            Ok(()) => log!("updated the sign-in screen support"),
            Err(e) => log!("couldn't update the sign-in screen support: {e:#}"),
        },
        Err(e) => log!("couldn't check the sign-in screen support is current: {e:#}"),
    }
}

/// `glide-secure.exe` next to glide.exe.
fn local_program() -> PathBuf {
    std::env::current_exe()
        .map(|p| p.with_file_name(SECURE_EXE))
        .unwrap_or_else(|_| PathBuf::from(SECURE_EXE))
}

/// This version's glide-secure.exe, from a source that can be trusted with
/// SYSTEM rights: the signed release (the copy next to glide.exe is in a
/// folder any program can change), or in a local build the one built here.
fn trusted_program() -> Result<Vec<u8>> {
    if !crate::is_release_build() {
        let path = local_program();
        return std::fs::read(&path).with_context(|| format!("reading {}", path.display()));
    }
    let program = crate::update::verified_release_file(SECURE_EXE)?;
    // Keep a copy next to glide.exe too, so later checks needn't download.
    let local = local_program();
    if std::fs::read(&local).ok().as_deref() != Some(program.as_slice()) {
        let _ = std::fs::write(&local, &program);
    }
    Ok(program)
}

fn write_program(path: &Path, program: &[u8]) -> Result<()> {
    retry(|| std::fs::write(path, program)).with_context(|| format!("writing {}", path.display()))
}

/// File operations right after stopping the service can briefly fail while
/// its processes exit.
fn retry(mut f: impl FnMut() -> std::io::Result<()>) -> std::io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match f() {
            Ok(()) => return Ok(()),
            Err(e) if Instant::now() >= deadline => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

/// Ask the service to stop and wait (a few seconds at most) until it has.
unsafe fn stop(svc: SC_HANDLE) {
    let mut status: SERVICE_STATUS = std::mem::zeroed();
    ControlService(svc, SERVICE_CONTROL_STOP, &mut status);
    let deadline = Instant::now() + Duration::from_secs(5);
    while QueryServiceStatus(svc, &mut status) != 0
        && status.dwCurrentState != SERVICE_STOPPED
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(100));
    }
}

struct Handle(SC_HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseServiceHandle(self.0) };
        }
    }
}

struct Scm(SC_HANDLE);
impl Scm {
    fn open(access: u32) -> Result<Scm> {
        let scm = unsafe { OpenSCManagerW(null(), null(), access) };
        if scm.is_null() {
            bail!("couldn't open Windows' service list ({})", unsafe { GetLastError() });
        }
        Ok(Scm(scm))
    }
}
impl Drop for Scm {
    fn drop(&mut self) {
        unsafe { CloseServiceHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asking_whether_installed_needs_no_admin() {
        // Must answer (either way) without admin rights or a crash.
        assert_eq!(installed(), installed());
    }
}

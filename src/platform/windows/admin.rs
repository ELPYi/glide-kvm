//! Running as administrator, without a UAC prompt on every launch.
//!
//! Windows blocks a normal program from sending input to, or seeing input
//! aimed at, windows running as administrator (Task Manager, installers,
//! admin terminals). glide needs admin rights to control those.
//!
//! Approach: once, with a single UAC prompt, glide registers a Task Scheduler
//! task set to "run with highest privileges". Starting that task never shows
//! a UAC prompt, so from then on `glide.exe` simply hands off to it. The same
//! task can also start glide at sign-in.
//!
//! The task runs a copy of glide in `C:\Program Files\glide`, not the one in
//! your own folder: any program can change files there, and one that
//! replaced glide.exe would otherwise get admin rights the next time glide
//! started. The copy is always checked against the signed release (or, in a
//! local build, is the running program), and `--home` points it back at your
//! folder for its settings.

use std::ffi::OsStr;
use std::mem::{size_of, zeroed};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, INFINITE,
};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

use super::wide;

pub const ADMIN_TASK: &str = "glide (administrator)";
/// Flag that the task-started instance uses to tell how it was launched.
pub const FROM_TASK_ARG: &str = "--from-task";
/// Followed by the folder holding glide.toml (see `config::set_home`).
pub const HOME_ARG: &str = "--home";
const NO_WINDOW: u32 = 0x0800_0000; // CREATE_NO_WINDOW: no console flash

/// Is this process running with administrator rights?
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation: TOKEN_ELEVATION = zeroed();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut TOKEN_ELEVATION as *mut _,
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

fn schtasks<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(args: I) -> Result<String> {
    let out = Command::new("schtasks")
        .args(args)
        .creation_flags(NO_WINDOW)
        .output()
        .context("running schtasks")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn admin_task_installed() -> bool {
    schtasks(["/Query", "/TN", ADMIN_TASK]).is_ok()
}

/// Whether the admin task also starts glide when you sign in.
pub fn admin_task_starts_at_sign_in() -> bool {
    schtasks(["/Query", "/TN", ADMIN_TASK, "/XML"]).is_ok_and(|xml| xml.contains("LogonTrigger"))
}

/// Start the admin task (no UAC prompt). Returns once it's been started.
pub fn run_admin_task() -> Result<()> {
    schtasks(["/Run", "/TN", ADMIN_TASK]).map(|_| ())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn app_exe() -> Result<PathBuf> {
    Ok(std::env::current_exe()?.with_file_name("glide.exe"))
}

/// The administrator copy: `C:\Program Files\glide\glide.exe`.
fn protected_exe() -> Result<PathBuf> {
    Ok(super::signin::install_dir()
        .context("couldn't find the Program Files folder")?
        .join("glide.exe"))
}

/// Put this version of glide in Program Files (if it isn't already).
/// Must run as administrator.
fn install_protected_copy() -> Result<PathBuf> {
    use crate::update::{install, sha256_hex, stage, APP_EXE};
    let target = protected_exe()?;
    let program = if crate::is_release_build() {
        crate::update::verified_release_file(APP_EXE)?
    } else {
        std::fs::read(std::env::current_exe()?).context("reading glide.exe")?
    };
    if std::fs::read(&target).is_ok_and(|b| b == program) {
        return Ok(target);
    }
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    // It may be the copy that's running: swap it the way updates do.
    stage(&target, &program, &sha256_hex(&program))?;
    install(&target)?;
    Ok(target)
}

/// Does the admin task run the Program Files copy? (Older versions pointed
/// it at your own folder.)
fn task_runs_protected_copy() -> bool {
    let Ok(exe) = protected_exe() else { return false };
    schtasks(["/Query", "/TN", ADMIN_TASK, "/XML"]).is_ok_and(|xml| {
        xml.contains(&format!(
            "<Command>{}</Command>",
            xml_escape(&exe.display().to_string())
        ))
    })
}

/// Admin mode set up by an older version: move it to the Program Files
/// copy. Runs in the administrator copy at startup; quick when there's
/// nothing to do.
pub fn protect_admin_task() {
    if !is_elevated() || !admin_task_installed() || task_runs_protected_copy() {
        return;
    }
    match install_admin_task(admin_task_starts_at_sign_in()) {
        Ok(()) => crate::log!("admin mode now runs glide from Program Files"),
        Err(e) => crate::log!("couldn't move admin mode to Program Files: {e:#}"),
    }
}

/// The task definition. Notable choices: normal priority (Task Scheduler's
/// default would run glide below normal) and no time limit (the default
/// stops tasks after 72 hours).
fn task_xml(exe: &str, args: &str, dir: &str, user: &str, at_sign_in: bool) -> String {
    let trigger = if at_sign_in {
        format!("<LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger>")
    } else {
        String::new()
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Starts glide with administrator rights so it can control admin windows such as Task Manager.</Description></RegistrationInfo>
  <Triggers>{trigger}</Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>{args}</Arguments>
      <WorkingDirectory>{dir}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// Create (or replace) the admin task. Must run as administrator.
pub fn install_admin_task(at_sign_in: bool) -> Result<()> {
    let exe = install_protected_copy()?;
    let dir = exe.parent().map(|d| d.display().to_string()).unwrap_or_default();
    // Your own glide folder, where the settings are.
    let home = crate::config::default_path()
        .parent()
        .map(|d| d.display().to_string())
        .context("couldn't tell where glide's settings are")?;
    let args = format!("{FROM_TASK_ARG} {HOME_ARG} \"{home}\"");
    let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(d), Ok(u)) => format!("{d}\\{u}"),
        (_, Ok(u)) => u,
        _ => bail!("couldn't tell which user is signed in"),
    };
    let xml = task_xml(
        &xml_escape(&exe.display().to_string()),
        &xml_escape(&args),
        &xml_escape(&dir),
        &xml_escape(&user),
        at_sign_in,
    );
    // schtasks wants the file as UTF-16 with a byte-order mark.
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    let path = std::env::temp_dir().join("glide-admin-task.xml");
    std::fs::write(&path, bytes)?;
    let result = schtasks(["/Create", "/TN", ADMIN_TASK, "/XML", &path.display().to_string(), "/F"]);
    let _ = std::fs::remove_file(&path);
    result.map(|_| ())
}

/// Delete the admin task. Must run as administrator.
pub fn remove_admin_task() -> Result<()> {
    if !admin_task_installed() {
        return Ok(());
    }
    schtasks(["/Delete", "/TN", ADMIN_TASK, "/F"]).map(|_| ())
}

/// Run glide.exe with `args` as administrator (UAC prompt), wait for it to
/// finish, and report its exit code as success/failure.
pub fn run_elevated(args: &str) -> Result<()> {
    let exe = app_exe()?;
    let (verb, file, params) = (wide("runas"), wide(&exe.display().to_string()), wide(args));
    let mut info: SHELLEXECUTEINFOW = unsafe { zeroed() };
    info.cbSize = size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = SW_HIDE;
    unsafe {
        if ShellExecuteExW(&mut info) == 0 {
            return Err(anyhow!("the administrator prompt was declined"));
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        GetExitCodeProcess(info.hProcess, &mut code);
        CloseHandle(info.hProcess);
        if code != 0 {
            bail!("the administrator step failed (code {code})");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_definition() {
        let with = task_xml(
            "C:\\Program Files\\glide\\glide.exe",
            "--from-task --home &quot;C:\\Users\\me\\glide&quot;",
            "C:\\Program Files\\glide",
            "PC\\me",
            true,
        );
        assert!(with.contains("<LogonTrigger>"));
        assert!(with.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(
            with.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"),
            "never stopped after 72 h"
        );
        assert!(
            with.contains("<Priority>4</Priority>"),
            "normal priority, not below normal"
        );
        assert!(with.contains(FROM_TASK_ARG));
        assert!(with.contains("<Arguments>--from-task --home &quot;C:\\Users\\me\\glide&quot;</Arguments>"));
        assert!(!task_xml("a", "", "b", "c", false).contains("LogonTrigger"));
        assert_eq!(xml_escape("a&b<c>"), "a&amp;b&lt;c&gt;");
    }

    #[test]
    fn elevation_check_runs() {
        // Whatever the answer, it must not crash, and must be consistent.
        assert_eq!(is_elevated(), is_elevated());
    }
}

#[cfg(test)]
mod schtasks_check {
    use super::*;

    /// Registers the exact task definition (minus the admin run level, which
    /// needs a UAC prompt) under a throwaway name, to prove Windows accepts
    /// it, then deletes it. Run with `--ignored`.
    #[test]
    #[ignore]
    fn windows_accepts_the_task_definition() {
        let name = "glide-selfcheck-delete-me";
        let user = format!(
            r"{}\{}",
            std::env::var("USERDOMAIN").unwrap(),
            std::env::var("USERNAME").unwrap()
        );
        // The real arguments' shape: a quoted folder with spaces.
        let args = format!(r#"{FROM_TASK_ARG} {HOME_ARG} "C:\Users\Some One\glide""#);
        let xml = task_xml(
            r"C:\Windows\System32\cmd.exe",
            &xml_escape(&args),
            r"C:\Windows",
            &xml_escape(&user),
            true,
        )
        .replace("HighestAvailable", "LeastPrivilege");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
        let path = std::env::temp_dir().join("glide-selfcheck.xml");
        std::fs::write(&path, bytes).unwrap();
        let created = schtasks(["/Create", "/TN", name, "/XML", &path.display().to_string(), "/F"]);
        let _ = std::fs::remove_file(&path);
        let query = schtasks(["/Query", "/TN", name, "/XML"]);
        let _ = schtasks(["/Delete", "/TN", name, "/F"]);
        created.expect("Windows rejected the task definition");
        let q = query.unwrap();
        assert!(q.contains("LogonTrigger") && q.contains("PT0S"), "{q}");
        assert!(
            q.contains(&format!("<Arguments>{args}</Arguments>")),
            "arguments kept as given: {q}"
        );
        eprintln!("task definition accepted by Task Scheduler");
    }
}

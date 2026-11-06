//! The helper as the client: while the sign-in screen is up and no glide
//! runs in this session (the PC has just started and nobody has signed in,
//! or you signed out), the helper connects to the sharing PC itself, so you
//! can sign in from there. Once the normal desktop is back, or glide starts
//! in the session, it lets go (glide needs the same network port, and waits
//! a few seconds for it). So quitting glide still means this PC can't be
//! controlled, except to sign in.
//!
//! It uses `signin.toml` next to it: what it needs of this PC's settings,
//! written there by glide (readable only by administrators and SYSTEM, since
//! it holds the secret). No clipboard or sound, and it changes nothing
//! about the cursor or pointer settings.

use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;

use super::{log, standby_config_path};
use crate::config::{Config, Role};
use crate::engine::Engine;

const CHECK_EVERY: Duration = Duration::from_secs(1);

pub fn run(session: u32) {
    let mut engine: Option<Engine> = None;
    // Logged once per change, not every second.
    let mut last_problem = String::new();
    loop {
        let wanted = !super::follow_input_desktop() && !glide_running_in(session);
        match (&engine, wanted) {
            (Some(_), false) => {
                log("signed in (or glide started): letting go of the connection");
                if let Some(e) = engine.take() {
                    e.stop();
                }
            }
            (None, true) => match start() {
                Ok(e) => {
                    log("sign-in screen with no glide running: connecting to the sharing PC");
                    last_problem.clear();
                    engine = Some(e);
                }
                Err(e) => {
                    let problem = format!("{e:#}");
                    if problem != last_problem {
                        log(&format!("not connecting for the sign-in screen: {problem}"));
                        last_problem = problem;
                    }
                }
            },
            _ => {}
        }
        std::thread::sleep(CHECK_EVERY);
    }
}

fn start() -> anyhow::Result<Engine> {
    let path = standby_config_path().ok_or_else(|| anyhow::anyhow!("no Program Files folder"))?;
    if !path.exists() {
        anyhow::bail!("this PC isn't set up as a controlled PC in glide");
    }
    let cfg = Config::load(&path)?;
    Engine::start_headless_client(&cfg)
}

/// Is glide.exe running in `session`?
fn glide_running_in(session: u32) -> bool {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return true; // can't tell: don't take the port from it
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut more = Process32FirstW(snap, &mut entry) != 0;
        while more {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if name.eq_ignore_ascii_case("glide.exe") {
                let mut s = u32::MAX;
                ProcessIdToSessionId(entry.th32ProcessID, &mut s);
                if s == session {
                    found = true;
                    break;
                }
            }
            more = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        found
    }
}

/// What the helper needs of this PC's settings: being the controlled PC,
/// how to reach the sharing PC, and nothing that touches the clipboard or
/// sound.
pub fn standby_config(cfg: &Config) -> Option<Config> {
    if cfg.role != Some(Role::Client) {
        return None;
    }
    let mut c = cfg.clone();
    c.clipboard = false;
    c.audio.share = false;
    c.audio.play = false;
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standby_settings_keep_the_connection_and_drop_the_extras() {
        let mut cfg = Config::new_default_with_secret("0123456789abcdefghij");
        assert!(standby_config(&cfg).is_none(), "not a controlled PC");
        cfg.role = Some(Role::Client);
        cfg.server_address = Some("10.0.0.2".into());
        let s = standby_config(&cfg).unwrap();
        assert_eq!(s.secret, cfg.secret);
        assert_eq!(s.server_address, cfg.server_address);
        assert_eq!(s.port, cfg.port);
        assert!(!s.clipboard && !s.audio.share);
    }

    #[test]
    fn finds_glide_by_session() {
        // Tests aren't glide.exe; whatever runs, the check must not crash.
        let mut session = 0u32;
        unsafe { ProcessIdToSessionId(std::process::id(), &mut session) };
        let _ = glide_running_in(session);
        assert!(!glide_running_in(u32::MAX - 1), "no such session");
    }
}

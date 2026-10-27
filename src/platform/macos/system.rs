//! Launch at sign-in (a LaunchAgent), the app window, idle time and "only
//! one glide at a time". Windows' administrator mode has no Mac
//! counterpart: those calls report "not installed" and do nothing.

use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

use super::ffi::*;

pub const FROM_TASK_ARG: &str = "--from-task";
const AGENT_LABEL: &str = "app.glide";

fn launch_agent() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{AGENT_LABEL}.plist")),
    )
}

pub fn autostart_enabled() -> bool {
    launch_agent().is_some_and(|p| p.exists())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn set_autostart(enabled: bool) -> Result<()> {
    let path = launch_agent().context("no home folder")?;
    if !enabled {
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>--minimized</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>ProcessType</key>
    <string>Interactive</string>
</dict>
</plist>
"#,
        xml_escape(&exe.to_string_lossy())
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, plist).with_context(|| format!("writing {}", path.display()))
}

/// macOS asks by itself the first time glide listens on the network.
pub fn open_firewall(_port: u16) -> Result<()> {
    Ok(())
}

/// The window behind the app's view (the handle is its NSView).
fn window(handle: isize) -> Option<Retained<AnyObject>> {
    let view = handle as *const AnyObject;
    if view.is_null() {
        return None;
    }
    unsafe { msg_send![&*view, window] }
}

pub fn show_window(handle: isize) {
    let Some(w) = window(handle) else { return };
    unsafe {
        let app: Retained<AnyObject> = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![&*app, activateIgnoringOtherApps: true];
        let _: () = msg_send![&*w, makeKeyAndOrderFront: Option::<&AnyObject>::None];
    }
}

pub fn hide_window(handle: isize) {
    let Some(w) = window(handle) else { return };
    unsafe {
        let _: () = msg_send![&*w, orderOut: Option::<&AnyObject>::None];
    }
}

pub fn window_visible(handle: isize) -> bool {
    window(handle).is_some_and(|w| unsafe { msg_send![&*w, isVisible] })
}

pub fn idle_for() -> Duration {
    let secs = unsafe {
        CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateCombinedSessionState, kCGAnyInputEventType)
    };
    Duration::from_secs_f64(secs.max(0.0))
}

pub fn show_osd(_text: &str) {}

/// The main display's height, less the menu bar and Dock (roughly).
pub fn work_area_height() -> Option<f32> {
    let b = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    (b.size.height > 0.0).then_some(b.size.height as f32 - 100.0)
}

/// Proof that this is the running glide; released when dropped (or when
/// glide exits, however it exits).
pub struct InstanceGuard(#[allow(dead_code)] std::fs::File);

pub fn claim_single_instance() -> Option<InstanceGuard> {
    // Development only: GLIDE_INSTANCE gives a test copy its own lock.
    let name = match std::env::var("GLIDE_INSTANCE") {
        Ok(suffix) => format!("glide-app-instance-{suffix}.lock"),
        Err(_) => "glide-app-instance.lock".into(),
    };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(std::env::temp_dir().join(name))
        .ok()?;
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    locked.then_some(InstanceGuard(file))
}

/// macOS itself brings an already-running app forward when it's opened again.
pub fn find_running_window() -> Option<isize> {
    None
}

pub fn is_elevated() -> bool {
    false
}

pub fn admin_task_installed() -> bool {
    false
}

pub fn admin_task_starts_at_sign_in() -> bool {
    false
}

const NO_ADMIN: &str = "administrator mode is Windows-only";

pub fn run_admin_task() -> Result<()> {
    bail!(NO_ADMIN)
}

pub fn install_admin_task(_at_sign_in: bool) -> Result<()> {
    bail!(NO_ADMIN)
}

pub fn remove_admin_task() -> Result<()> {
    bail!(NO_ADMIN)
}

pub fn run_elevated(_args: &str) -> Result<()> {
    bail!(NO_ADMIN)
}

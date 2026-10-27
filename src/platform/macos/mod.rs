//! macOS backend.
//!
//! - [`desktop`]: injection (CGEventPost) and display geometry
//! - [`capture`]: a CoreGraphics event tap on a dedicated thread
//! - [`keymap`]: macOS key codes <-> neutral evdev codes
//! - [`cursor`]: hiding the cursor while it's on another PC
//! - [`clipboard`]: watching and filling the Mac clipboard (NSPasteboard)
//! - [`system`]: launch at sign-in, the app window, idle time, one instance
//! - [`ffi`]: the CoreGraphics / CoreFoundation declarations
//!
//! Using (and blocking) the mouse and keyboard needs the Accessibility
//! permission; macOS asks the first time sharing starts. Sharing sound
//! isn't available on the Mac yet.

mod capture;
mod clipboard;
mod cursor;
mod desktop;
mod ffi;
mod keymap;
pub mod system;

use std::sync::Once;
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

pub use capture::start_capture;
pub use clipboard::open_clipboard;
pub use desktop::desktop;

const NO_SOUND: &str = "sharing sound isn't available on macOS yet";

pub fn open_loopback(_mute: bool) -> Result<Box<dyn crate::audio::AudioIn>> {
    bail!(NO_SOUND)
}

pub fn open_speaker() -> Result<Box<dyn crate::audio::AudioOut>> {
    bail!(NO_SOUND)
}

extern "C" {
    fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
}

/// QOS_CLASS_USER_INTERACTIVE: what macOS gives the thread drawing the UI.
const QOS_USER_INTERACTIVE: u32 = 0x21;

/// Call at the top of a thread that handles input or mouse packets.
pub fn boost_latency_thread() {
    unsafe {
        pthread_set_qos_class_self_np(QOS_USER_INTERACTIVE, 0);
    }
}

/// Keep macOS from slowing glide down while it's in the background
/// ("App Nap" stretches timers by up to seconds, which would stall the
/// mouse). Held for as long as glide runs.
pub fn raise_process_priority() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        // NSActivityUserInitiated | NSActivityLatencyCritical
        const OPTIONS: u64 = 0x00FF_FFFF | 0xFF_0000_0000;
        let info: Option<Retained<AnyObject>> = msg_send![class!(NSProcessInfo), processInfo];
        let reason: Option<Retained<AnyObject>> =
            msg_send![class!(NSString), stringWithUTF8String: c"Sharing the mouse and keyboard".as_ptr()];
        if let (Some(info), Some(reason)) = (info, reason) {
            let token: Option<Retained<AnyObject>> =
                msg_send![&*info, beginActivityWithOptions: OPTIONS, reason: &*reason];
            std::mem::forget(token); // never ended
        }
    });
}

/// macOS sleeps accurately enough (tens of microseconds) without help.
pub fn precise_sleep(d: Duration) {
    std::thread::sleep(d);
}

/// Mark glide's packets as interactive (DSCP "expedited forwarding"); on
/// Wi-Fi that's the voice queue. Best-effort.
pub fn prioritize_traffic(sock: &std::net::UdpSocket, _dest: std::net::SocketAddr) -> bool {
    use std::os::fd::AsRawFd;
    const EXPEDITED: libc::c_int = 0xB8;
    let ok = unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::IPPROTO_IP,
            libc::IP_TOS,
            &EXPEDITED as *const libc::c_int as *const libc::c_void,
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    ok == 0
}

/// HTTPS download (updates), with the curl every Mac has.
pub fn http_get(url: &str) -> Result<Vec<u8>> {
    let out = std::process::Command::new("/usr/bin/curl")
        .args([
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--max-time",
            "300",
            url,
        ])
        .output()
        .context("running curl")?;
    ensure!(
        out.status.success(),
        "download failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(out.stdout)
}

pub fn local_time() -> (u8, u8, u8) {
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    unsafe { libc::localtime_r(&now, &mut tm) };
    (tm.tm_hour as u8, tm.tm_min as u8, tm.tm_sec as u8)
}

/// `glide-cli selftest`: checks the permission and that input can be captured.
pub fn selftest() -> Result<()> {
    desktop::ensure_permission()?;
    let capture = start_capture(
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Box::new(|_| false),
    )?;
    capture.stop();
    let screens = desktop::screen_layout().bounds();
    println!(
        "ok: Accessibility allowed, input capture works, screens span {}x{} points",
        screens.right - screens.left,
        screens.bottom - screens.top
    );
    Ok(())
}

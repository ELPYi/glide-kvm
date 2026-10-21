//! Windows backend.
//!
//! - [`desktop`]: injection (SendInput) and cursor/monitor geometry
//! - [`capture`]: low-level hooks + Raw Input on a dedicated thread
//! - [`keymap`]: Windows scan codes <-> neutral evdev codes
//! - [`system`]: autostart at sign-in, firewall rule, local time
//! - [`pointer`]: reading/mirroring pointer speed and acceleration
//! - [`timing`]: sub-millisecond sleeps
//! - [`qos`]: Wi-Fi/network priority tagging for glide's packets
//! - [`sched`]: CPU priority (MMCSS) for latency-critical threads
//! - [`admin`]: running as administrator via a scheduled task
//! - [`instance`]: one running copy; a second launch shows the first
//! - [`blocking`]: can this PC accept input right now (lock screen, admin window)
//! - [`cursor`]: hiding the cursor while it's on another PC
//! - [`osd`]: brief on-screen confirmations ("Switching locked")
//! - [`http`]: HTTPS downloads for updates (WinHTTP)
//! - [`selftest`]: checks the assumptions capture relies on

mod admin;
mod blocking;
mod capture;
mod cursor;
mod desktop;
mod http;
mod instance;
mod keymap;
mod osd;
mod pointer;
mod qos;
mod sched;
mod selftest;
pub mod system;
mod timing;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

pub use capture::start_capture;
pub use desktop::desktop;
pub use http::http_get;
pub use qos::prioritize as prioritize_traffic;
pub use sched::{boost_latency_thread, raise_process_priority};
pub use selftest::selftest;
pub use system::local_time;
pub use timing::precise_sleep;

/// Normally injected input (from other software, or from glide itself) is
/// ignored by capture. The selftest flips this so *only* injected input is
/// seen, letting it drive the hooks without your real mouse muddying results.
static SELFTEST: AtomicBool = AtomicBool::new(false);

/// Should capture process an event with this injected flag?
fn wanted(injected: bool) -> bool {
    injected == SELFTEST.load(Ordering::Relaxed)
}

/// Work in physical pixels everywhere, so cursor maths is right on scaled monitors.
fn init_dpi() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    });
}

/// NUL-terminated UTF-16 for Win32 "W" functions.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

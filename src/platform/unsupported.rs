//! Shared stand-in for OSes whose backend isn't written yet. Everything
//! compiles and fails at runtime with a clear message, so the rest of the
//! crate (and its tests) keep building on every OS.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use anyhow::{bail, Result};

use super::{CaptureHandle, Desktop, Handler};

const MSG: &str = "glide doesn't support this operating system yet";

pub fn desktop() -> Result<Arc<dyn Desktop>> {
    bail!(MSG)
}

pub fn start_capture(_grab: Arc<AtomicBool>, _handler: Handler) -> Result<CaptureHandle> {
    bail!(MSG)
}

pub fn selftest() -> Result<()> {
    bail!(MSG)
}

pub fn boost_latency_thread() {}

pub fn http_get(_url: &str) -> Result<Vec<u8>> {
    bail!(MSG)
}

pub fn raise_process_priority() {}

pub fn open_loopback(_mute: bool) -> Result<Box<dyn crate::audio::AudioIn>> {
    bail!(MSG)
}

pub fn open_speaker() -> Result<Box<dyn crate::audio::AudioOut>> {
    bail!(MSG)
}

pub fn prioritize_traffic(_sock: &std::net::UdpSocket, _dest: std::net::SocketAddr) -> bool {
    false
}

pub fn precise_sleep(d: std::time::Duration) {
    std::thread::sleep(d);
}

/// UTC; a real backend should return local time.
pub fn local_time() -> (u8, u8, u8) {
    let secs = crate::now_micros() / 1_000_000;
    (((secs / 3600) % 24) as u8, ((secs / 60) % 60) as u8, (secs % 60) as u8)
}

pub mod system {
    use anyhow::{bail, Result};

    pub fn autostart_enabled() -> bool {
        false
    }

    pub fn set_autostart(_enabled: bool) -> Result<()> {
        bail!(super::MSG)
    }

    pub fn open_firewall(_port: u16) -> Result<()> {
        bail!(super::MSG)
    }

    pub const FROM_TASK_ARG: &str = "--from-task";

    pub fn show_window(_handle: isize) {}

    pub fn hide_window(_handle: isize) {}

    pub fn window_visible(_handle: isize) -> bool {
        true
    }

    pub fn idle_for() -> std::time::Duration {
        std::time::Duration::ZERO
    }

    pub fn show_osd(_text: &str) {}

    pub fn work_area_height() -> Option<f32> {
        None
    }

    pub struct InstanceGuard;

    pub fn claim_single_instance() -> Option<InstanceGuard> {
        Some(InstanceGuard)
    }

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

    pub fn run_admin_task() -> Result<()> {
        bail!(super::MSG)
    }

    pub fn install_admin_task(_at_sign_in: bool) -> Result<()> {
        bail!(super::MSG)
    }

    pub fn remove_admin_task() -> Result<()> {
        bail!(super::MSG)
    }

    pub fn run_elevated(_args: &str) -> Result<()> {
        bail!(super::MSG)
    }
}

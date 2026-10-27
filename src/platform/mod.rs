//! Everything OS-specific lives behind this module. The rest of the crate
//! only sees the [`Desktop`] trait and the free functions re-exported below.
//!
//! ## Contract each OS module must provide
//!
//! ```text
//! fn desktop() -> Result<Arc<dyn Desktop>>
//! fn start_capture(grab: Arc<AtomicBool>, handler: Handler) -> Result<CaptureHandle>
//! fn selftest() -> Result<()>
//! fn local_time() -> (u8, u8, u8)
//! fn precise_sleep(d: Duration)      // sub-millisecond accurate
//! fn prioritize_traffic(sock, dest) -> bool   // best-effort QoS tagging
//! fn boost_latency_thread()          // CPU priority for hot threads
//! fn http_get(url) -> Result<Vec<u8>>  // HTTPS download (updates)
//! fn open_loopback(mute) -> Result<Box<dyn AudioIn>>  // what this PC plays
//! fn open_speaker() -> Result<Box<dyn AudioOut>>   // play sound here
//! fn open_clipboard(on_copy) -> Result<Box<dyn Clipboard>>  // shared clipboard
//! fn raise_process_priority()
//! mod system {
//!     fn autostart_enabled() -> bool
//!     fn set_autostart(enabled: bool) -> Result<()>
//!     fn open_firewall(port: u16) -> Result<()>
//!     fn is_elevated() / install_admin_task() / run_admin_task() / ...  // admin mode
//! }
//! ```
//!
//! `cargo check --target x86_64-unknown-linux-gnu` / `aarch64-apple-darwin`
//! verifies the stubs keep satisfying it.

use crate::hotkey::Mods;
use crate::input::{InputBlock, InputEvent, PointerFeel, Side};

pub mod geometry;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeHit {
    pub side: Side,
    /// Position along the edge, 0.0..=1.0 across the whole desktop.
    pub frac: f32,
}

/// One physical input event as seen by the capture layer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Captured {
    Input(InputEvent),
    /// The cursor is pinned against an outer edge and still being pushed outward.
    /// Only reported while not grabbed.
    Edge(EdgeHit),
    /// The modifiers the OS says are actually held, sent just before a key
    /// press while not grabbed. Corrects keys whose release was never seen
    /// (it went to the lock screen, or to an admin window like Task Manager),
    /// which would otherwise make every shortcut stop matching.
    Modifiers(Mods),
}

/// Returns true to swallow the event locally. Called on the capture thread
/// (inside OS input hooks), so it must be fast and never block for long.
/// Motion is swallowed according to the `grab` flag, not this return value.
pub type Handler = Box<dyn FnMut(Captured) -> bool + Send>;

/// Screen-level operations: injecting input and moving the cursor.
pub trait Desktop: Send + Sync {
    fn inject(&self, ev: &InputEvent);
    /// Put the cursor on `side` of the desktop, `frac` of the way along it.
    fn warp_to_edge(&self, side: Side, frac: f32);
    /// If the cursor sits on an outer desktop edge and (dx, dy) pushes past it.
    fn edge_hit(&self, dx: i32, dy: i32) -> Option<EdgeHit>;
    /// This machine's pointer speed/acceleration, if the OS exposes it.
    fn pointer_feel(&self) -> Option<PointerFeel> {
        None
    }
    /// Briefly show a short message on screen (e.g. "Switching locked").
    fn notify(&self, _text: &str) {}
    /// Whether this PC can accept injected input right now. Windows gives no
    /// error when it can't, so this is checked directly.
    fn input_block(&self) -> Option<InputBlock> {
        None
    }
    /// Hide the cursor on this PC while it's "on another PC" (`false` shows
    /// it again). Hiding must never outlive glide.
    fn set_cursor_hidden(&self, _hidden: bool) {}
    /// Where the cursor is, to notice this PC being used directly.
    fn cursor_position(&self) -> Option<(i32, i32)> {
        None
    }
    /// What the focused app looks like, to tell when a game is in focus.
    fn foreground(&self) -> Option<crate::game::Foreground> {
        None
    }
    /// Temporarily use `feel` (not saved; signing out restores it anyway).
    /// `None` restores this machine's own settings.
    fn set_pointer_feel(&self, _feel: Option<PointerFeel>) {}
}

/// A running input capture. Stops (and restores normal input) when dropped.
pub struct CaptureHandle {
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl CaptureHandle {
    pub fn new(stop: impl FnOnce() + Send + 'static) -> Self {
        CaptureHandle {
            stop: Some(Box::new(stop)),
        }
    }

    pub fn stop(mut self) {
        if let Some(f) = self.stop.take() {
            f();
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        if let Some(f) = self.stop.take() {
            f();
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod unsupported;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::{
    boost_latency_thread, desktop, http_get, local_time, open_clipboard, open_loopback, open_speaker, precise_sleep,
    prioritize_traffic, raise_process_priority, selftest, start_capture, system,
};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use self::linux::{
    boost_latency_thread, desktop, http_get, local_time, open_clipboard, open_loopback, open_speaker, precise_sleep,
    prioritize_traffic, raise_process_priority, selftest, start_capture, system,
};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use self::macos::{
    boost_latency_thread, desktop, http_get, local_time, open_clipboard, open_loopback, open_speaker, precise_sleep,
    prioritize_traffic, raise_process_priority, selftest, start_capture, system,
};

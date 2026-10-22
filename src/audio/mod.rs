//! Sound from the other PCs, played on the sharing PC.
//!
//! Each PC being controlled records everything it plays ("loopback"),
//! compresses it with Opus and sends it to the sharing PC, which mixes it
//! into its own speakers or headset. It streams all the time unless
//! switched off in Settings (on either PC).
//!
//! ```text
//!  laptop                                        main PC
//!  loopback -> 10 ms frames -> Opus -> UDP  ==>  decode -> playout buffer -> mix -> speakers
//!  (share)                                       (play)    (smooths Wi-Fi jitter, clock drift)
//! ```
//!
//! **Kept off the mouse's path.** Audio runs on its own threads on both
//! PCs, at normal priority, and travels the other way from mouse packets
//! (laptop -> main PC, while the mouse goes main PC -> laptop). The server
//! hands audio packets straight to the player without taking the lock the
//! input hook uses. It shares the existing connection because a second one
//! would need a firewall rule on the main PC.
//!
//! - [`codec`]: Opus encoder and decoder
//! - [`playout`]: the per-PC buffer between the network and the speakers
//! - [`share`]: client side: capture, encode, send
//! - [`play`]: server side: receive, decode, mix, play

pub mod codec;
pub mod play;
pub mod playout;
pub mod share;

use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Everything is 48 kHz stereo float; Windows converts from and to each
/// device's own format.
pub const SAMPLE_RATE: usize = 48_000;
pub const CHANNELS: usize = 2;
/// Samples per channel in one 10 ms frame (one packet).
pub const FRAME: usize = SAMPLE_RATE / 100;
/// Interleaved samples in one frame.
pub const FRAME_SAMPLES: usize = FRAME * CHANNELS;

/// Frames (per channel) in `ms` milliseconds.
pub const fn frames_in_ms(ms: usize) -> usize {
    SAMPLE_RATE / 1000 * ms
}

/// `[audio]` in glide.toml. Live-adjustable.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AudioSettings {
    /// Sharing PC: play the other PCs' sound here.
    pub play: bool,
    /// Sharing PC: how loud their sound is, 0-100.
    pub volume: u8,
    /// Other PCs: send this PC's sound to the sharing PC.
    pub share: bool,
    /// Other PCs: mute their own speakers while sending, so the sound
    /// isn't heard twice. (What's sent is unaffected.)
    pub mute_while_sharing: bool,
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings {
            play: true,
            volume: 100,
            share: true,
            mute_while_sharing: true,
        }
    }
}

impl AudioSettings {
    /// `volume` as a gain. Squared, so the slider feels even to the ear.
    pub fn gain(&self) -> f32 {
        let v = f32::from(self.volume.min(100)) / 100.0;
        v * v
    }
}

/// What this PC is playing, captured as it goes to the speakers.
pub trait AudioIn {
    /// Append everything captured since the last call (interleaved stereo).
    fn read(&mut self, out: &mut Vec<f32>) -> Result<()>;
    /// Sleep until more sound is ready, or at most `max`.
    fn wait(&mut self, max: Duration);
    /// The default output device changed (headphones plugged in, say):
    /// reopen to follow it. Called about once a second, which is also when
    /// the speakers are muted again if something unmuted them.
    fn stale(&mut self) -> bool;
}

/// The speakers (or headset) sound is played on.
pub trait AudioOut {
    /// Sleep until the device wants more, or at most `max`.
    fn wait(&mut self, max: Duration);
    /// Frames already handed to the device but not yet heard.
    fn queued(&mut self) -> Result<usize>;
    /// Queue interleaved stereo samples.
    fn write(&mut self, samples: &[f32]) -> Result<()>;
    /// See [`AudioIn::stale`].
    fn stale(&mut self) -> bool;
}

/// Opens a device on the audio thread (COM objects stay on the thread that
/// made them). `OpenIn`'s argument: mute this PC's speakers meanwhile.
pub type OpenIn = Box<dyn Fn(bool) -> Result<Box<dyn AudioIn>> + Send>;
pub type OpenOut = Box<dyn Fn() -> Result<Box<dyn AudioOut>> + Send>;

/// The client's way to the sharing PC (implemented by the client).
pub trait Uplink: Send {
    /// Connected, and the sharing PC wants sound.
    fn ready(&self) -> bool;
    /// Send one Opus frame. `stream` identifies this run of the sender.
    fn send(&self, stream: u32, seq: u32, opus: &[u8]);
}

//! Client side: record what this PC plays and send it to the sharing PC.
//!
//! One thread, normal priority. While connected (and the sharing PC wants
//! sound) it reads the captured sound every few milliseconds, cuts it into
//! 10 ms frames, compresses each and sends it. During silence it sends
//! nothing.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::codec::{Encoder, MAX_PACKET};
use super::{AudioIn, AudioSettings, OpenIn, Uplink, FRAME_SAMPLES};
use crate::log;

/// Longest wait for sound, so setting changes are noticed promptly.
const WAIT_AT_MOST: Duration = Duration::from_millis(100);
/// While not sending, how often to check whether to start.
const IDLE_CHECK: Duration = Duration::from_millis(250);
/// After failing to open the sound device, wait this long to try again.
const RETRY_AFTER: Duration = Duration::from_secs(5);
/// How often to check whether the default output device changed.
const DEVICE_CHECK: Duration = Duration::from_secs(1);
/// Frames of silence still sent after sound stops, so the other end's
/// decoder settles on silence too, before going quiet.
const SILENT_TAIL: u32 = 3;
/// Quieter than this is silence (about -80 dB).
const SILENT: f32 = 1e-4;

pub struct Sharer {
    settings: Arc<Flags>,
    /// Recording and sending right now.
    sending: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// The settings the sharing thread reads.
#[derive(Default)]
struct Flags {
    share: AtomicBool,
    mute: AtomicBool,
}

impl Sharer {
    pub fn start(settings: AudioSettings, uplink: Box<dyn Uplink>, open: OpenIn) -> Sharer {
        let flags = Arc::new(Flags::default());
        let sending = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(true));
        let (f, s, r) = (flags.clone(), sending.clone(), running.clone());
        let thread = std::thread::Builder::new()
            .name("glide-audio-share".into())
            .spawn(move || {
                share_loop(&f, &r, uplink.as_ref(), &open, &s);
                s.store(false, Ordering::Relaxed);
            })
            .ok();
        let sharer = Sharer {
            settings: flags,
            sending,
            running,
            thread,
        };
        sharer.set(settings);
        sharer
    }

    pub fn set(&self, settings: AudioSettings) {
        self.settings.share.store(settings.share, Ordering::Relaxed);
        self.settings.mute.store(settings.mute_while_sharing, Ordering::Relaxed);
    }

    /// Recording this PC's sound for the sharing PC right now.
    pub fn sending(&self) -> bool {
        self.sending.load(Ordering::Relaxed)
    }
}

impl Drop for Sharer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Capture {
    input: Box<dyn AudioIn>,
    checked: Instant,
    /// Opened with this PC's speakers muted.
    muted: bool,
}

fn share_loop(flags: &Flags, running: &AtomicBool, uplink: &dyn Uplink, open: &OpenIn, sending: &AtomicBool) {
    let mut encoder = match Encoder::new() {
        Ok(e) => e,
        Err(e) => {
            log!("sound sharing is off: {e:#}");
            return;
        }
    };
    // Identifies this run, so the other end starts clean if glide restarts.
    let stream = crate::now_micros() as u32;
    let mut seq = 0u32;
    let mut capture: Option<Capture> = None;
    let mut failed_at: Option<Instant> = None;
    let mut pcm: Vec<f32> = Vec::with_capacity(FRAME_SAMPLES * 8);
    let mut packet = [0u8; MAX_PACKET];
    let mut silent_run = 0u32;

    while running.load(Ordering::Relaxed) {
        sending.store(capture.is_some(), Ordering::Relaxed);
        let mute = flags.mute.load(Ordering::Relaxed);
        if capture.as_ref().is_some_and(|c| c.muted != mute) {
            capture = None; // reopen with the speakers (un)muted
        }
        if !(flags.share.load(Ordering::Relaxed) && uplink.ready()) {
            if capture.take().is_some() {
                log!("stopped sending sound");
            }
            pcm.clear();
            std::thread::sleep(IDLE_CHECK);
            continue;
        }
        if capture.is_none() {
            if failed_at.is_some_and(|t| t.elapsed() < RETRY_AFTER) {
                std::thread::sleep(IDLE_CHECK);
                continue;
            }
            match open(mute) {
                Ok(input) => {
                    if !sending.load(Ordering::Relaxed) {
                        log!("sending this PC's sound");
                    }
                    failed_at = None;
                    capture = Some(Capture {
                        input,
                        checked: Instant::now(),
                        muted: mute,
                    });
                }
                Err(e) => {
                    if failed_at.is_none() {
                        log!("can't record this PC's sound: {e:#}");
                    }
                    failed_at = Some(Instant::now());
                    continue;
                }
            }
        }
        let c = capture.as_mut().expect("opened above");
        if c.checked.elapsed() >= DEVICE_CHECK {
            c.checked = Instant::now();
            if c.input.stale() {
                log!("sound device changed; following it");
                capture = None;
                continue;
            }
        }
        if let Err(e) = c.input.read(&mut pcm) {
            log!("lost this PC's sound ({e:#}); reopening");
            capture = None;
            pcm.clear();
            std::thread::sleep(IDLE_CHECK);
            continue;
        }

        let whole = pcm.len() / FRAME_SAMPLES * FRAME_SAMPLES;
        for frame in pcm[..whole].as_chunks::<FRAME_SAMPLES>().0 {
            if frame.iter().all(|s| s.abs() < SILENT) {
                silent_run = silent_run.saturating_add(1);
                if silent_run > SILENT_TAIL {
                    continue;
                }
            } else {
                silent_run = 0;
            }
            match encoder.encode(frame, &mut packet) {
                Ok(n) => {
                    uplink.send(stream, seq, &packet[..n]);
                    seq = seq.wrapping_add(1);
                }
                Err(e) => log!("{e:#}"),
            }
        }
        pcm.drain(..whole);
        c.input.wait(WAIT_AT_MOST);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeIn {
        /// Frames handed out per read.
        script: Arc<Mutex<Vec<Vec<f32>>>>,
    }

    impl AudioIn for FakeIn {
        fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<()> {
            if let Some(chunk) = self.script.lock().unwrap().pop() {
                out.extend(chunk);
            }
            Ok(())
        }
        fn wait(&mut self, _max: Duration) {
            std::thread::sleep(Duration::from_millis(2));
        }
        fn stale(&mut self) -> bool {
            false
        }
    }

    #[derive(Clone, Default)]
    struct FakeUplink {
        sent: Arc<Mutex<Vec<(u32, usize)>>>,
    }

    impl Uplink for FakeUplink {
        fn ready(&self) -> bool {
            true
        }
        fn send(&self, _stream: u32, seq: u32, opus: &[u8]) {
            self.sent.lock().unwrap().push((seq, opus.len()));
        }
    }

    #[test]
    fn sends_sound_in_frames_and_goes_quiet_in_silence() {
        let tone: Vec<f32> = (0..FRAME_SAMPLES * 3)
            .map(|i| ((i / 2) as f32 * 0.05).sin() * 0.5)
            .collect();
        let silence = vec![0.0; FRAME_SAMPLES * 10];
        // Popped from the end: 1.5 frames, the rest of the tone, then silence.
        let script = Arc::new(Mutex::new(vec![
            silence,
            tone[FRAME_SAMPLES * 3 / 2..].to_vec(),
            tone[..FRAME_SAMPLES * 3 / 2].to_vec(),
        ]));
        let uplink = FakeUplink::default();
        let s = script.clone();
        let sharer = Sharer::start(
            AudioSettings::default(),
            Box::new(uplink.clone()),
            Box::new(move |_mute| Ok(Box::new(FakeIn { script: s.clone() }) as Box<dyn AudioIn>)),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !script.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100));
        drop(sharer);
        let sent = uplink.sent.lock().unwrap().clone();
        let seqs: Vec<u32> = sent.iter().map(|(s, _)| *s).collect();
        assert_eq!(
            seqs,
            (0..3 + SILENT_TAIL).collect::<Vec<_>>(),
            "3 frames of sound, a short silent tail, then nothing"
        );
    }
}

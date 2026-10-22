//! Server side: play the other PCs' sound on this PC.
//!
//! The network thread hands each packet over with [`Player::push`], which
//! never blocks (a full queue drops the packet). The player's own thread
//! decodes, keeps a [`Playout`] buffer per sending PC, mixes them and feeds
//! the speakers. The sound device is only opened while sound is arriving,
//! and closed a few seconds after it stops, so glide doesn't keep the
//! audio device (and the PC) awake for nothing.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::playout::Playout;
use super::{frames_in_ms, AudioOut, AudioSettings, OpenOut, CHANNELS};
use crate::log;

/// Packets waiting for the player thread (~1 s from one PC).
const QUEUE: usize = 128;
/// Keep about this much handed to the sound device. Its own period is
/// 10 ms; a little more covers the thread waking late.
const DEVICE_QUEUE: usize = frames_in_ms(15);
const WAIT: Duration = Duration::from_millis(20);
const DEVICE_CHECK: Duration = Duration::from_secs(1);
const RETRY_AFTER: Duration = Duration::from_secs(5);

struct Packet {
    from: SocketAddr,
    stream: u32,
    seq: u32,
    opus: Vec<u8>,
}

/// Shared between the player thread and everyone else.
struct Shared {
    enabled: AtomicBool,
    /// `AudioSettings::gain` as f32 bits.
    gain: AtomicU32,
    /// PCs whose sound is playing right now (for the UI).
    playing: Mutex<Vec<SocketAddr>>,
}

pub struct Player {
    tx: SyncSender<Packet>,
    shared: Arc<Shared>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    pub fn start(settings: AudioSettings, open: OpenOut) -> Player {
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let shared = Arc::new(Shared {
            enabled: AtomicBool::new(settings.play),
            gain: AtomicU32::new(settings.gain().to_bits()),
            playing: Mutex::new(Vec::new()),
        });
        let running = Arc::new(AtomicBool::new(true));
        let (s, r) = (shared.clone(), running.clone());
        let thread = std::thread::Builder::new()
            .name("glide-audio-play".into())
            .spawn(move || play_loop(&rx, &s, &r, &open))
            .ok();
        Player {
            tx,
            shared,
            running,
            thread,
        }
    }

    /// Whether to ask the other PCs for their sound.
    pub fn wants_sound(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }

    pub fn set(&self, settings: AudioSettings) {
        self.shared.enabled.store(settings.play, Ordering::Relaxed);
        self.shared.gain.store(settings.gain().to_bits(), Ordering::Relaxed);
    }

    /// One packet from `from`. Called on the network thread: never blocks.
    pub fn push(&self, from: SocketAddr, stream: u32, seq: u32, opus: &[u8]) {
        if self.wants_sound() {
            let _ = self.tx.try_send(Packet {
                from,
                stream,
                seq,
                opus: opus.to_vec(),
            });
        }
    }

    /// Sound from `addr` is playing right now.
    pub fn playing_from(&self, addr: SocketAddr) -> bool {
        self.shared
            .playing
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&addr)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Output {
    device: Box<dyn AudioOut>,
    checked: Instant,
}

fn play_loop(rx: &Receiver<Packet>, shared: &Shared, running: &AtomicBool, open: &OpenOut) {
    let mut streams: HashMap<SocketAddr, Playout> = HashMap::new();
    let mut output: Option<Output> = None;
    let mut failed_at: Option<Instant> = None;
    let mut mix: Vec<f32> = Vec::new();

    while running.load(Ordering::Relaxed) {
        // Nothing playing: sleep until a packet arrives.
        if output.is_none() {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(p) => receive(&mut streams, p),
                Err(RecvTimeoutError::Timeout) => {
                    let now = Instant::now();
                    streams.retain(|_, s| !s.idle(now));
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if failed_at.is_some_and(|t| t.elapsed() < RETRY_AFTER) {
                continue;
            }
            match open() {
                Ok(device) => {
                    failed_at = None;
                    output = Some(Output {
                        device,
                        checked: Instant::now(),
                    });
                }
                Err(e) => {
                    if failed_at.is_none() {
                        log!("can't play sound from the other PC: {e:#}");
                    }
                    failed_at = Some(Instant::now());
                    continue;
                }
            }
        }

        let out = output.as_mut().expect("opened above");
        out.device.wait(WAIT);
        loop {
            match rx.try_recv() {
                Ok(p) => receive(&mut streams, p),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if !shared.enabled.load(Ordering::Relaxed) {
            streams.clear();
        }

        let now = Instant::now();
        streams.retain(|_, s| !s.idle(now));
        let playing: Vec<SocketAddr> = streams.iter().filter(|(_, s)| s.active(now)).map(|(a, _)| *a).collect();
        *shared.playing.lock().unwrap_or_else(|e| e.into_inner()) = playing;
        if streams.is_empty() {
            output = None; // let the device (and the PC) rest
            continue;
        }

        if out.checked.elapsed() >= DEVICE_CHECK {
            out.checked = now;
            if out.device.stale() {
                log!("sound device changed; following it");
                output = None;
                continue;
            }
        }
        let queued = match out.device.queued() {
            Ok(q) => q,
            Err(e) => {
                log!("lost the sound device ({e:#}); reopening");
                output = None;
                continue;
            }
        };
        let frames = DEVICE_QUEUE.saturating_sub(queued);
        if frames == 0 {
            continue;
        }
        mix.clear();
        mix.resize(frames * CHANNELS, 0.0);
        let gain = f32::from_bits(shared.gain.load(Ordering::Relaxed));
        for s in streams.values_mut() {
            s.pull(&mut mix, gain);
        }
        for s in mix.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
        if let Err(e) = out.device.write(&mix) {
            log!("lost the sound device ({e:#}); reopening");
            output = None;
        }
    }
}

fn receive(streams: &mut HashMap<SocketAddr, Playout>, p: Packet) {
    let now = Instant::now();
    let s = match streams.entry(p.from) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(e) => match Playout::new() {
            Ok(s) => e.insert(s),
            Err(err) => {
                log!("{err:#}");
                return;
            }
        },
    };
    s.push(p.stream, p.seq, &p.opus, now);
}

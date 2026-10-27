//! Where glide's CPU time goes: a real server and client over loopback
//! (mock desktops, fake sound devices), each scenario run for a few seconds
//! while every thread's CPU time is read from Windows.
//!
//! `cargo test --release --test cpu -- --ignored --nocapture`

#![cfg(windows)]

mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use glide::audio::play::Player;
use glide::audio::share::Sharer;
use glide::audio::{AudioIn, AudioOut, AudioSettings, CHANNELS, SAMPLE_RATE};
use glide::input::{InputEvent, Side};
use glide::platform::{Captured, EdgeHit};

const RUN_FOR: Duration = Duration::from_secs(4);
/// A gaming mouse's report rate.
const MOUSE_HZ: u32 = 8000;

/// Plays a tone in 10 ms chunks, like Windows' capture does.
struct ToneIn {
    since: Instant,
    made: usize,
}

impl AudioIn for ToneIn {
    fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<()> {
        let due = (self.since.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as usize / 480 * 480;
        for i in self.made..due {
            let s = (i as f32 / SAMPLE_RATE as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            out.extend([s; CHANNELS]);
        }
        self.made = due.max(self.made);
        Ok(())
    }
    fn wait(&mut self, _max: Duration) {
        std::thread::sleep(Duration::from_millis(10));
    }
    fn stale(&mut self) -> bool {
        false
    }
}

/// Speakers that wake every few ms, like Windows' event-driven playback.
struct NullOut {
    since: Instant,
    end: usize,
}

impl AudioOut for NullOut {
    fn wait(&mut self, max: Duration) {
        std::thread::sleep(max.min(Duration::from_millis(5)));
    }
    fn queued(&mut self) -> anyhow::Result<usize> {
        let now = (self.since.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as usize;
        Ok(self.end.saturating_sub(now))
    }
    fn write(&mut self, samples: &[f32]) -> anyhow::Result<()> {
        let now = (self.since.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as usize;
        self.end = self.end.max(now) + samples.len() / CHANNELS;
        Ok(())
    }
    fn stale(&mut self) -> bool {
        false
    }
}

/// CPU time (user + kernel) of every thread in this process, by name.
fn thread_times() -> BTreeMap<String, Duration> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcessId, GetThreadDescription, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION,
    };
    let ft =
        |f: FILETIME| Duration::from_nanos(((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) * 100);
    let mut out = BTreeMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        assert!(snap != INVALID_HANDLE_VALUE);
        let mut e: THREADENTRY32 = std::mem::zeroed();
        e.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut ok = Thread32First(snap, &mut e) != 0;
        while ok {
            if e.th32OwnerProcessID == GetCurrentProcessId() {
                let h = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, e.th32ThreadID);
                if !h.is_null() {
                    let mut name_ptr = std::ptr::null_mut();
                    let mut name = String::new();
                    if GetThreadDescription(h, &mut name_ptr) >= 0 && !name_ptr.is_null() {
                        let len = (0..).take_while(|&i| *name_ptr.add(i) != 0).count();
                        name = String::from_utf16_lossy(std::slice::from_raw_parts(name_ptr, len));
                        windows_sys::Win32::Foundation::LocalFree(name_ptr as _);
                    }
                    if name.is_empty() {
                        name = "(unnamed)".into();
                    }
                    let (mut c, mut x, mut k, mut u) = std::mem::zeroed();
                    if GetThreadTimes(h, &mut c, &mut x, &mut k, &mut u) != 0 {
                        *out.entry(name).or_default() += ft(k) + ft(u);
                    }
                    CloseHandle(h);
                }
            }
            ok = Thread32Next(snap, &mut e) != 0;
        }
        CloseHandle(snap);
    }
    out
}

/// Runs for RUN_FOR and prints each thread's share of one core.
fn measure(what: &str) {
    let before = thread_times();
    std::thread::sleep(RUN_FOR);
    let after = thread_times();
    let mut rows: Vec<(String, f64)> = after
        .iter()
        .filter(|(name, _)| !name.starts_with("bench"))
        .map(|(name, t)| {
            let d = t.saturating_sub(before.get(name).copied().unwrap_or_default());
            (name.clone(), d.as_secs_f64() / RUN_FOR.as_secs_f64() * 100.0)
        })
        .filter(|(_, pct)| *pct >= 0.01)
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    let total: f64 = rows.iter().map(|r| r.1).sum();
    eprintln!("\n{what}: {total:.2}% of one core");
    for (name, pct) in rows {
        eprintln!("  {pct:6.2}%  {name}");
    }
}

/// Moves the mouse at MOUSE_HZ (from a "bench" thread, not counted).
fn wiggle(p: &Pair, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    let server = p.server.clone();
    std::thread::Builder::new()
        .name("bench-mouse".into())
        .spawn(move || {
            let every = Duration::from_secs(1) / MOUSE_HZ;
            let start = Instant::now();
            let mut n = 0u32;
            while !stop.load(Ordering::Relaxed) {
                n += 1;
                let dx = if (n / 400).is_multiple_of(2) { 1 } else { -1 };
                server.on_captured(Captured::Input(InputEvent::Motion { dx, dy: 0 }));
                while start.elapsed() < every * n {
                    std::hint::spin_loop();
                }
            }
        })
        .unwrap()
}

#[test]
#[ignore = "benchmark; run in release"]
fn where_the_cpu_goes() {
    let p = connected_pair(classic());
    std::thread::sleep(Duration::from_millis(500));
    measure("connected, using this PC");

    p.server.on_captured(Captured::Edge(EdgeHit {
        side: Side::Right,
        frac: 0.5,
    }));
    let client = p.client.as_ref().unwrap();
    wait_for("enter", || client.status().active);
    measure("controlling the other PC, mouse still");

    let stop = Arc::new(AtomicBool::new(false));
    let mouse = wiggle(&p, stop.clone());
    measure(&format!("controlling, mouse moving at {MOUSE_HZ} Hz"));
    stop.store(true, Ordering::Relaxed);
    mouse.join().unwrap();

    p.server.set_player(Some(Player::start(
        AudioSettings::default(),
        Box::new(|| {
            Ok(Box::new(NullOut {
                since: Instant::now(),
                end: 0,
            }) as Box<dyn AudioOut>)
        }),
    )));
    let heard = Arc::new(Mutex::new(()));
    let _keep = heard.clone();
    let _sharer = Sharer::start(
        AudioSettings::default(),
        Box::new(client.audio_sender().unwrap()),
        Box::new(|_mute| {
            Ok(Box::new(ToneIn {
                since: Instant::now(),
                made: 0,
            }) as Box<dyn AudioIn>)
        }),
    );
    wait_for("sound flowing", || p.server.status().peers[0].sound);
    measure("controlling, mouse still, sound streaming");

    let stop = Arc::new(AtomicBool::new(false));
    let mouse = wiggle(&p, stop.clone());
    measure(&format!("controlling, mouse at {MOUSE_HZ} Hz, sound streaming"));
    stop.store(true, Ordering::Relaxed);
    mouse.join().unwrap();
}

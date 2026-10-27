//! Sound end to end: a real server and client over loopback UDP, with fake
//! sound devices standing in for the speakers and the loopback capture.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use glide::audio::play::Player;
use glide::audio::share::Sharer;
use glide::audio::{AudioIn, AudioOut, AudioSettings, CHANNELS, SAMPLE_RATE};
use glide::input::Side;
use glide::platform::{Captured, EdgeHit};

/// A PC playing a 440 Hz tone, captured in real time.
struct ToneIn {
    since: Instant,
    made: usize,
}

impl AudioIn for ToneIn {
    fn read(&mut self, out: &mut Vec<f32>) -> anyhow::Result<()> {
        let due = (self.since.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as usize;
        for i in self.made..due {
            let s = (i as f32 / SAMPLE_RATE as f32 * 440.0 * std::f32::consts::TAU).sin() * 0.5;
            out.extend([s; CHANNELS]);
        }
        self.made = due.max(self.made);
        Ok(())
    }
    fn wait(&mut self, _max: Duration) {
        std::thread::sleep(Duration::from_millis(2));
    }
    fn stale(&mut self) -> bool {
        false
    }
}

/// Speakers that play in real time and remember what they were given.
/// Like a real device, running out just means silence: nothing is owed.
struct RecordingOut {
    heard: Arc<Mutex<Vec<f32>>>,
    since: Instant,
    /// Where (in frames since `since`) the queued sound ends.
    end: usize,
}

impl RecordingOut {
    fn now(&self) -> usize {
        (self.since.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as usize
    }
}

impl AudioOut for RecordingOut {
    fn wait(&mut self, max: Duration) {
        std::thread::sleep(max.min(Duration::from_millis(5)));
    }
    fn queued(&mut self) -> anyhow::Result<usize> {
        Ok(self.end.saturating_sub(self.now()))
    }
    fn write(&mut self, samples: &[f32]) -> anyhow::Result<()> {
        self.end = self.end.max(self.now()) + samples.len() / CHANNELS;
        self.heard.lock().unwrap().extend_from_slice(samples);
        Ok(())
    }
    fn stale(&mut self) -> bool {
        false
    }
}

#[test]
fn a_pcs_sound_plays_on_the_sharing_pc() {
    let p = connected_pair(classic());
    let heard = Arc::new(Mutex::new(Vec::new()));
    let h = heard.clone();
    p.server.set_player(Some(Player::start(
        AudioSettings::default(),
        Box::new(move || {
            Ok(Box::new(RecordingOut {
                heard: h.clone(),
                since: Instant::now(),
                end: 0,
            }) as Box<dyn AudioOut>)
        }),
    )));
    let client = p.client.as_ref().unwrap();
    let sharer = Sharer::start(
        AudioSettings::default(),
        Box::new(client.audio_sender().unwrap()),
        Box::new(|_mute| {
            Ok(Box::new(ToneIn {
                since: Instant::now(),
                made: 0,
            }) as Box<dyn AudioIn>)
        }),
    );

    wait_for("the tone to play on the server", || {
        heard.lock().unwrap().iter().filter(|s| s.abs() > 0.2).count() > SAMPLE_RATE / 10
    });
    wait_for("the UI to show sound from the client", || {
        p.server.status().peers[0].sound
    });
    assert!(sharer.sending());

    // A second of play has no dropouts: once the tone starts, it never goes
    // quiet for more than a zero crossing.
    std::thread::sleep(Duration::from_secs(1));
    let samples = heard.lock().unwrap().clone();
    let start = samples.iter().position(|s| s.abs() > 0.2).unwrap() / CHANNELS;
    let left: Vec<f32> = samples.iter().step_by(CHANNELS).copied().collect();
    let mut quiet_run = 0;
    let mut worst = 0;
    for s in &left[start..] {
        quiet_run = if s.abs() < 0.05 { quiet_run + 1 } else { 0 };
        worst = worst.max(quiet_run);
    }
    assert!(left.len() - start > SAMPLE_RATE, "a full second played");
    assert!(worst < SAMPLE_RATE / 1000, "a gap of {worst} samples");

    // The mouse is unaffected while sound streams the other way.
    p.server.on_captured(Captured::Edge(EdgeHit {
        side: Side::Right,
        frac: 0.5,
    }));
    wait_for("mouse crossing", || client.status().active);

    // Switched off on the sharing PC: the client stops recording.
    p.server.set_audio(AudioSettings {
        play: false,
        ..AudioSettings::default()
    });
    wait_for("the client to stop sending", || !sharer.sending());
    drop(sharer);
}

/// Hook-to-injection time for 1000 single mouse moves.
fn mouse_latency(p: &Pair) -> (Duration, Duration) {
    p.client_desk.clear();
    let mut lat = Vec::new();
    for i in 0..1000 {
        std::thread::sleep(Duration::from_millis(2));
        let t0 = Instant::now();
        p.server
            .on_captured(Captured::Input(glide::input::InputEvent::Motion { dx: 1, dy: 0 }));
        wait_for("motion", || p.client_desk.injected.lock().unwrap().len() > i);
        lat.push(p.client_desk.injected.lock().unwrap()[i].0 - t0);
    }
    lat.sort();
    (lat[lat.len() / 2], lat[lat.len() * 99 / 100])
}

/// `cargo test --release --test sound -- --ignored --nocapture`
#[test]
#[ignore = "benchmark; run in release"]
fn sound_does_not_slow_the_mouse() {
    let p = connected_pair(classic());
    p.server.on_captured(Captured::Edge(EdgeHit {
        side: Side::Right,
        frac: 0.5,
    }));
    let client = p.client.as_ref().unwrap();
    wait_for("enter", || client.status().active);
    let (quiet50, quiet99) = mouse_latency(&p);

    let heard = Arc::new(Mutex::new(Vec::new()));
    let h = heard.clone();
    p.server.set_player(Some(Player::start(
        AudioSettings::default(),
        Box::new(move || {
            Ok(Box::new(RecordingOut {
                heard: h.clone(),
                since: Instant::now(),
                end: 0,
            }) as Box<dyn AudioOut>)
        }),
    )));
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
    let (sound50, sound99) = mouse_latency(&p);
    assert!(p.server.status().peers[0].sound, "sound kept flowing throughout");

    eprintln!("mouse, no sound:   p50 {quiet50:?}  p99 {quiet99:?}");
    eprintln!("mouse, with sound: p50 {sound50:?}  p99 {sound99:?}");
    assert!(sound50 < quiet50 * 2 + Duration::from_micros(50));
}

/// Typing while sound streams: every key arrives, in order.
#[test]
fn typing_works_while_sound_streams() {
    let p = connected_pair(classic());
    let client = p.client.as_ref().unwrap();
    p.server.set_player(Some(Player::start(
        AudioSettings::default(),
        Box::new(|| {
            Ok(Box::new(RecordingOut {
                heard: Arc::new(Mutex::new(Vec::new())),
                since: Instant::now(),
                end: 0,
            }) as Box<dyn AudioOut>)
        }),
    )));
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
    p.server.on_captured(Captured::Edge(EdgeHit {
        side: Side::Right,
        frac: 0.5,
    }));
    wait_for("enter", || client.status().active);

    p.client_desk.clear();
    let mut sent = Vec::new();
    for i in 0..400u16 {
        let code = 16 + i % 20; // letters
        for down in [true, false] {
            let ev = glide::input::InputEvent::Key { code, down };
            assert!(p.server.on_captured(Captured::Input(ev)), "swallowed on the sharing PC");
            sent.push(ev);
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    wait_for("every key", || p.client_desk.events().len() >= sent.len());
    assert_eq!(p.client_desk.events(), sent);
    assert!(p.server.status().peers[0].sound, "sound still flowing");
}

/// Muting a device on the sharing PC silences it; unmuting brings it back.
#[test]
fn a_device_can_be_muted_from_the_sharing_pc() {
    let p = connected_pair(classic());
    let client = p.client.as_ref().unwrap();
    let heard = Arc::new(Mutex::new(Vec::new()));
    let h = heard.clone();
    p.server.set_player(Some(Player::start(
        AudioSettings::default(),
        Box::new(move || {
            Ok(Box::new(RecordingOut {
                heard: h.clone(),
                since: Instant::now(),
                end: 0,
            }) as Box<dyn AudioOut>)
        }),
    )));
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
    wait_for("sound playing", || p.server.status().peers[0].sound);

    p.server.set_device_muted(0, true);
    assert!(p.server.status().peers[0].muted);
    wait_for("muted", || !p.server.status().peers[0].sound);
    std::thread::sleep(Duration::from_millis(300)); // let what was buffered finish
    heard.lock().unwrap().clear();
    std::thread::sleep(Duration::from_millis(300));
    let loud = heard.lock().unwrap().iter().filter(|s| s.abs() > 0.05).count();
    assert_eq!(loud, 0, "nothing from the muted PC");

    p.server.set_device_muted(0, false);
    wait_for("playing again", || p.server.status().peers[0].sound);
}

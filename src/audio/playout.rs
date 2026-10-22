//! The buffer between the network and the speakers, one per sending PC.
//!
//! Packets don't arrive evenly over Wi-Fi, so a little sound is kept in
//! hand before playing: enough to ride out a late packet, not so much that
//! it lags. That amount adapts. It starts small, grows after a dropout
//! (an audible gap means the network is jumpier than assumed), and slowly
//! shrinks again while things are steady.
//!
//! The two PCs' sound cards also run at very slightly different speeds,
//! so over minutes the buffer would creep up (lag) or run dry (crackles).
//! The level settled at once playing starts is the reference; when it
//! drifts away from that, playback runs 0.2% faster or slower (inaudible)
//! until it's back.
//!
//! A lost packet is filled in by Opus's concealment; a packet that turns
//! up after its slot has played is dropped.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::codec::Decoder;
use super::{frames_in_ms, CHANNELS, FRAME_SAMPLES};

/// What's kept in hand at first, and the least it ever shrinks to. Sound
/// arrives 10 ms at a time and the speakers take it 10 ms at a time, so
/// 20 ms is the bare minimum; a little over leaves room for a late packet.
const START_TARGET: usize = frames_in_ms(30);
const MIN_TARGET: usize = frames_in_ms(20);
const MAX_TARGET: usize = frames_in_ms(150);
/// Growth after a dropout / shrinking after a steady spell.
const GROW_BY: usize = frames_in_ms(10);
const SHRINK_BY: usize = frames_in_ms(5);
const STEADY_FOR: usize = frames_in_ms(30_000);
/// The level may wander this far from its reference before playback speed
/// is nudged (and the nudge then lasts until it's back at the reference).
const DRIFT_BAND: usize = frames_in_ms(3);
const NUDGE: f64 = 0.002;
/// Pulls (~10 ms each) to measure the reference level over.
const SETTLE_PULLS: usize = 50;
/// More than this beyond the target (a burst after a stall): skip ahead.
const OVERFLOW: usize = frames_in_ms(80);
/// Up to this many lost packets in a row are concealed. A bigger gap is
/// the sender pausing (it stops sending during silence), not loss.
const MAX_CONCEAL: u32 = 5;
/// Quieter than this is silence (about -80 dB).
const SILENT: f32 = 1e-4;
/// A sender heard nothing from for this long is gone.
pub const IDLE_AFTER: Duration = Duration::from_secs(3);

pub struct Playout {
    stream: Option<u32>,
    decoder: Decoder,
    next_seq: u32,
    /// Interleaved stereo, decoded and waiting to be played.
    pcm: VecDeque<f32>,
    /// Building back up to `target` before playing (at the start, after a gap).
    priming: bool,
    /// How much to keep in hand, in frames.
    target: usize,
    /// Frames played since the last dropout.
    steady: usize,
    /// Smoothed buffer level, in frames.
    level: f64,
    /// The level when playback settled; drift is measured against it.
    reference: Option<f64>,
    /// Pulls since playback (re)started.
    pulls: usize,
    /// Playback speed: 1.0, or nudged while correcting drift.
    ratio: f64,
    /// Read position within the first buffered frame (0..1), for the
    /// speed nudges.
    pos: f64,
    /// The newest decoded audio was silence (so running dry is just the
    /// sound ending, not a dropout).
    tail_silent: bool,
    last_packet: Option<Instant>,
    frame: Vec<f32>,
}

impl Playout {
    pub fn new() -> anyhow::Result<Playout> {
        Ok(Playout {
            stream: None,
            decoder: Decoder::new()?,
            next_seq: 0,
            pcm: VecDeque::new(),
            priming: true,
            target: START_TARGET,
            steady: 0,
            level: 0.0,
            reference: None,
            pulls: 0,
            ratio: 1.0,
            pos: 0.0,
            tail_silent: true,
            last_packet: None,
            frame: vec![0.0; FRAME_SAMPLES],
        })
    }

    /// Frames decoded and waiting.
    pub fn buffered(&self) -> usize {
        self.pcm.len() / CHANNELS
    }

    pub fn target(&self) -> usize {
        self.target
    }

    /// Nothing heard from this sender for a while.
    pub fn idle(&self, now: Instant) -> bool {
        self.last_packet.is_none_or(|t| now.duration_since(t) >= IDLE_AFTER)
    }

    /// Sound is coming out (or about to).
    pub fn active(&self, now: Instant) -> bool {
        self.last_packet
            .is_some_and(|t| now.duration_since(t) < Duration::from_secs(1))
            && !self.tail_silent
    }

    /// One packet from the network.
    pub fn push(&mut self, stream: u32, seq: u32, packet: &[u8], now: Instant) {
        if self.stream != Some(stream) {
            // A new sender run (it restarted): start clean.
            if let Ok(d) = Decoder::new() {
                self.decoder = d;
            }
            self.stream = Some(stream);
            self.next_seq = seq;
            self.pcm.clear();
            self.priming = true;
            self.pos = 0.0;
        }
        let gap = seq.wrapping_sub(self.next_seq);
        if gap > u32::MAX / 2 {
            return; // late (its slot already played) or a duplicate
        }
        if (1..=MAX_CONCEAL).contains(&gap) {
            for _ in 0..gap {
                self.decode(None);
            }
        }
        self.decode(Some(packet));
        self.next_seq = seq.wrapping_add(1);
        self.last_packet = Some(now);

        if self.buffered() > self.target + OVERFLOW {
            let extra = self.buffered() - self.target;
            self.pcm.drain(..extra * CHANNELS);
            self.pos = 0.0;
        }
    }

    fn decode(&mut self, packet: Option<&[u8]>) {
        if self.decoder.decode(packet, &mut self.frame).is_err() {
            // Corrupt: conceal it like a lost one.
            let _ = self.decoder.decode(None, &mut self.frame);
        }
        self.tail_silent = self.frame.iter().all(|s| s.abs() < SILENT);
        self.pcm.extend(&self.frame);
    }

    /// Add this sender's next `out.len() / 2` frames into `out`, scaled by
    /// `gain` (several senders mix into one output).
    pub fn pull(&mut self, out: &mut [f32], gain: f32) {
        let frames = out.len() / CHANNELS;
        if self.priming {
            if self.buffered() < self.target {
                return;
            }
            self.priming = false;
            self.level = self.buffered() as f64;
            self.reference = None;
            self.pulls = 0;
            self.ratio = 1.0;
        }

        self.level += (self.buffered() as f64 - self.level) * 0.02;
        self.pulls += 1;
        if self.pulls == SETTLE_PULLS {
            self.reference = Some(self.level);
        }
        if let Some(r) = self.reference {
            let band = DRIFT_BAND as f64;
            self.ratio = if self.level > r + band {
                1.0 + NUDGE // falling behind: catch up
            } else if self.level < r - band {
                1.0 - NUDGE // running low: stretch
            } else if (self.ratio > 1.0 && self.level <= r) || (self.ratio < 1.0 && self.level >= r) {
                1.0 // back at the reference
            } else {
                self.ratio
            };
        }
        let ratio = self.ratio;
        if ratio == 1.0 && self.pos != 0.0 {
            // Back to normal speed: snap to a whole sample (inaudible).
            if self.pos >= 0.5 {
                self.pcm.drain(..CHANNELS.min(self.pcm.len()));
            }
            self.pos = 0.0;
        }

        for i in 0..frames {
            if self.buffered() == 0 {
                self.ran_dry();
                return;
            }
            // Between this sample and the next while nudging (or this one
            // alone if the next hasn't arrived yet).
            let blend = self.pos > 0.0 && self.buffered() > 1;
            for c in 0..CHANNELS {
                let a = self.pcm[c];
                let s = if blend {
                    let b = self.pcm[CHANNELS + c];
                    a + (b - a) * self.pos as f32
                } else {
                    a
                };
                out[i * CHANNELS + c] += s * gain;
            }
            self.pos += ratio;
            while self.pos >= 1.0 {
                self.pcm.drain(..CHANNELS);
                self.pos -= 1.0;
            }
        }

        self.steady += frames;
        if self.steady >= STEADY_FOR {
            self.steady = 0;
            let shrunk = self.target.saturating_sub(SHRINK_BY).max(MIN_TARGET);
            // Lower the reference too, so the nudging actually trims the lag.
            if let Some(r) = &mut self.reference {
                *r -= (self.target - shrunk) as f64;
            }
            self.target = shrunk;
        }
    }

    fn ran_dry(&mut self) {
        self.priming = true;
        self.pos = 0.0;
        self.steady = 0;
        if !self.tail_silent {
            // A real dropout: keep more in hand from now on.
            self.target = (self.target + GROW_BY).min(MAX_TARGET);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::codec::{Encoder, MAX_PACKET};
    use crate::audio::{FRAME, SAMPLE_RATE};

    /// One encoded frame of a loud tone (reused for every packet: the
    /// decoder doesn't mind, and encoding in debug builds is slow).
    fn loud_packet() -> Vec<u8> {
        let mut enc = Encoder::new().unwrap();
        let pcm: Vec<f32> = (0..FRAME)
            .flat_map(|i| {
                let s = (i as f32 / SAMPLE_RATE as f32 * 1000.0 * std::f32::consts::TAU).sin() * 0.5;
                [s, s]
            })
            .collect();
        let mut out = [0u8; MAX_PACKET];
        let mut n = 0;
        for _ in 0..5 {
            n = enc.encode(&pcm, &mut out).unwrap();
        }
        out[..n].to_vec()
    }

    fn pull(p: &mut Playout) -> Vec<f32> {
        let mut out = vec![0.0; FRAME_SAMPLES];
        p.pull(&mut out, 1.0);
        out
    }

    fn loud(out: &[f32]) -> bool {
        out.iter().any(|s| s.abs() > 0.05)
    }

    #[test]
    fn waits_for_a_little_sound_in_hand_then_plays() {
        let pkt = loud_packet();
        let now = Instant::now();
        let mut p = Playout::new().unwrap();
        p.push(1, 0, &pkt, now);
        p.push(1, 1, &pkt, now);
        assert!(!loud(&pull(&mut p)), "still building up");
        p.push(1, 2, &pkt, now);
        assert!(p.buffered() >= START_TARGET);
        assert!(loud(&pull(&mut p)));
        // Steady: one in, one out, never a gap.
        for seq in 3..200 {
            p.push(1, seq, &pkt, now);
            assert!(loud(&pull(&mut p)), "packet {seq}");
        }
        assert_eq!(p.target(), START_TARGET, "no dropouts, no growth");
    }

    #[test]
    fn a_lost_packet_is_concealed_and_a_late_one_dropped() {
        let pkt = loud_packet();
        let now = Instant::now();
        let mut p = Playout::new().unwrap();
        p.push(1, 0, &pkt, now);
        p.push(1, 2, &pkt, now); // 1 was lost
        assert_eq!(p.buffered(), 3 * FRAME, "packet 1 was filled in");
        p.push(1, 1, &pkt, now); // ...and turns up late
        assert_eq!(p.buffered(), 3 * FRAME, "too late, dropped");
        p.push(1, 2, &pkt, now);
        assert_eq!(p.buffered(), 3 * FRAME, "duplicate dropped");
    }

    #[test]
    fn a_dropout_grows_the_buffer_but_sound_ending_does_not() {
        let pkt = loud_packet();
        let now = Instant::now();
        let mut p = Playout::new().unwrap();
        for seq in 0..3 {
            p.push(1, seq, &pkt, now);
        }
        for _ in 0..4 {
            pull(&mut p); // the 4th runs dry mid-sound
        }
        assert_eq!(p.target(), START_TARGET + GROW_BY);

        // A sender ends with silence before pausing.
        let mut enc = Encoder::new().unwrap();
        let mut q = Playout::new().unwrap();
        let mut buf = [0u8; MAX_PACKET];
        for seq in 0..6 {
            let n = enc.encode(&vec![0.0; FRAME_SAMPLES], &mut buf).unwrap();
            q.push(1, seq, &buf[..n], now);
        }
        for _ in 0..10 {
            pull(&mut q);
        }
        assert_eq!(q.target(), START_TARGET, "silence running out isn't a dropout");
    }

    #[test]
    fn a_sender_restart_starts_clean() {
        let pkt = loud_packet();
        let now = Instant::now();
        let mut p = Playout::new().unwrap();
        for seq in 100..105 {
            p.push(1, seq, &pkt, now);
        }
        p.push(2, 0, &pkt, now);
        assert_eq!(p.buffered(), FRAME, "old run's sound discarded, seq 0 accepted");
    }

    #[test]
    fn clock_drift_never_builds_up_lag() {
        let pkt = loud_packet();
        let now = Instant::now();
        let mut p = Playout::new().unwrap();
        // The sender's clock runs 0.1% fast (far worse than real hardware):
        // an extra packet every 1000. Two simulated minutes, then 5 s more.
        let mut seq = 0;
        for i in 0..12_500 {
            p.push(1, seq, &pkt, now);
            seq += 1;
            if i % 1000 == 999 && i < 12_000 {
                p.push(1, seq, &pkt, now);
                seq += 1;
            }
            pull(&mut p);
        }
        let lag = p.buffered();
        assert!(
            lag <= p.target() + FRAME + DRIFT_BAND,
            "lag {lag} frames, target {}",
            p.target()
        );

        // And 0.1% slow: it stretches instead of crackling.
        let mut q = Playout::new().unwrap();
        let mut seq = 0;
        let mut dry = 0;
        for i in 0..12_000 {
            if i % 1000 != 999 {
                q.push(1, seq, &pkt, now);
                seq += 1;
            }
            let before = q.target();
            pull(&mut q);
            if q.target() > before {
                dry += 1;
            }
        }
        assert_eq!(dry, 0, "ran dry {dry} times");
    }

    #[test]
    fn a_burst_after_a_stall_skips_ahead() {
        let pkt = loud_packet();
        let now = Instant::now();
        let mut p = Playout::new().unwrap();
        for seq in 0..40 {
            p.push(1, seq, &pkt, now);
        }
        assert!(
            p.buffered() <= START_TARGET + OVERFLOW,
            "never more than ~100 ms behind"
        );
    }
}

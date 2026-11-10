//! Smoothing motion over a jittery network.
//!
//! A busy Wi-Fi holds packets back for tens of milliseconds and then delivers
//! them all at once: injected as they arrive, the cursor freezes, then jumps.
//! The server stamps each motion packet with when it sent it, and this plays
//! the movement back out at that pace, a fixed delay later: a jitter buffer,
//! like a call's audio.
//!
//! The delay adapts. It covers most of the recent lateness (the 98th
//! percentile of how much later than the quickest packets they arrive), but
//! never more than [`MAX_DELAY_US`]: one 60 Hz frame. On a steady link the
//! lateness is ~0, so nothing is held back. Rises are followed quickly (a
//! burst of jitter), falls slowly (so one quiet moment doesn't drop it just
//! before the next burst).
//!
//! Pure data structure: times are microseconds on the caller's clock.

use std::collections::VecDeque;

/// The most movement is ever held back.
pub const MAX_DELAY_US: i64 = 16_000;
/// How far back the quickest packet is remembered. The two PCs' clocks drift
/// apart by far less than the jitter over this long.
const WINDOW_US: i64 = 2_000_000;
const BUCKET_US: i64 = 250_000;
/// Recent lateness samples the delay is chosen from.
const SAMPLES: usize = 256;
/// The delay is re-chosen every this many packets.
const RETARGET_EVERY: u32 = 16;
const PERCENTILE: f64 = 0.98;
const RISE: f64 = 0.3;
const FALL: f64 = 0.02;

#[derive(Default)]
pub struct Smoother {
    /// The last sender time seen, unwrapped to 64 bits.
    last_sent: Option<i64>,
    /// (bucket start, quickest transit in it), oldest first. Transit is
    /// arrival minus sender time: both clocks' offset plus the trip.
    quickest: VecDeque<(i64, i64)>,
    lateness: VecDeque<i64>,
    since_retarget: u32,
    delay: f64,
    /// (when to inject, dx, dy), in order.
    queue: VecDeque<(i64, i32, i32)>,
}

impl Smoother {
    /// The current delay, in µs.
    #[cfg(test)]
    pub fn delay_us(&self) -> i64 {
        self.delay as i64
    }

    /// Movement sent at `sent` (sender's clock, µs, wrapping) that arrived
    /// at `now` (ours).
    pub fn push(&mut self, sent: u32, now: i64, dx: i32, dy: i32) {
        let sent = self.unwrap(sent);
        let transit = now - sent;
        self.note_transit(now, transit);
        let base = self.base().unwrap_or(transit);
        self.note_lateness(transit - base);

        let last_due = self.queue.back().map_or(i64::MIN, |q| q.0);
        let due = (sent + base + self.delay as i64).max(last_due).min(now + MAX_DELAY_US);
        self.queue.push_back((due, dx, dy));
    }

    /// When the next held movement is due, if any.
    pub fn next_due(&self) -> Option<i64> {
        self.queue.front().map(|q| q.0)
    }

    /// The movement due by `now`.
    pub fn take_due(&mut self, now: i64) -> (i32, i32) {
        let mut out = (0i32, 0i32);
        while let Some(&(due, dx, dy)) = self.queue.front() {
            if due > now {
                break;
            }
            self.queue.pop_front();
            out = (out.0.saturating_add(dx), out.1.saturating_add(dy));
        }
        out
    }

    /// Everything held, now (a click must land where the cursor should be).
    pub fn take_all(&mut self) -> (i32, i32) {
        self.take_due(i64::MAX)
    }

    /// Drop anything held (control left this PC). What was learned about the
    /// network stays.
    pub fn clear(&mut self) {
        self.queue.clear();
    }

    fn unwrap(&mut self, sent: u32) -> i64 {
        let v = match self.last_sent {
            None => sent as i64,
            Some(last) => last + sent.wrapping_sub(last as u32) as i32 as i64,
        };
        self.last_sent = Some(v);
        v
    }

    fn note_transit(&mut self, now: i64, transit: i64) {
        match self.quickest.back_mut() {
            Some((start, min)) if now - *start < BUCKET_US => *min = (*min).min(transit),
            _ => self.quickest.push_back((now, transit)),
        }
        while self.quickest.front().is_some_and(|b| now - b.0 > WINDOW_US) {
            self.quickest.pop_front();
        }
    }

    fn base(&self) -> Option<i64> {
        self.quickest.iter().map(|b| b.1).min()
    }

    fn note_lateness(&mut self, late: i64) {
        if self.lateness.len() == SAMPLES {
            self.lateness.pop_front();
        }
        self.lateness.push_back(late.max(0));
        self.since_retarget += 1;
        if self.since_retarget < RETARGET_EVERY {
            return;
        }
        self.since_retarget = 0;
        let mut sorted: Vec<i64> = self.lateness.iter().copied().collect();
        sorted.sort_unstable();
        let at = ((sorted.len() - 1) as f64 * PERCENTILE) as usize;
        let target = sorted[at].min(MAX_DELAY_US) as f64;
        let rate = if target > self.delay { RISE } else { FALL };
        self.delay += (target - self.delay) * rate;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sends 1 px every `gap` µs, each arriving `trip(i)` µs later, and
    /// returns (when, how far) for every injection, polling every 100 µs.
    fn run(packets: u32, gap: i64, trip: impl Fn(u32) -> i64) -> (Smoother, Vec<(i64, i32)>) {
        let mut s = Smoother::default();
        let mut arrivals: Vec<(i64, u32)> = (0..packets).map(|i| (i as i64 * gap + trip(i), i)).collect();
        arrivals.sort();
        let end = arrivals.last().unwrap().0 + MAX_DELAY_US + 1000;
        let mut out = Vec::new();
        let mut next = 0;
        let mut now = 0;
        while now <= end {
            while next < arrivals.len() && arrivals[next].0 <= now {
                let i = arrivals[next].1;
                // A clock offset between the PCs, and the server's clock wrapping.
                let sent = (i as i64 * gap) as u32;
                s.push(sent.wrapping_add(u32::MAX - 5_000_000), now, 1, 0);
                next += 1;
            }
            let (dx, _) = s.take_due(now);
            if dx != 0 {
                out.push((now, dx));
            }
            now += 100;
        }
        (s, out)
    }

    /// The longest stretch without movement, after the first `skip` µs.
    fn longest_freeze(out: &[(i64, i32)], skip: i64) -> i64 {
        out.windows(2)
            .filter(|w| w[0].0 >= skip)
            .map(|w| w[1].0 - w[0].0)
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn a_steady_link_holds_nothing_back() {
        let (s, out) = run(2000, 1000, |_| 3000);
        assert_eq!(out.iter().map(|o| o.1).sum::<i32>(), 2000);
        assert!(s.delay_us() < 200, "delay {}", s.delay_us());
        // Each packet goes out as soon as it arrives.
        assert!(out.iter().all(|o| o.1 == 1));
    }

    #[test]
    fn bursts_come_out_evenly() {
        // Every 50 ms the network stalls for 12 ms and then delivers
        // everything at once: unsmoothed, a 12 ms freeze then a 12 px jump.
        let stall = |i: u32| {
            let t = (i % 50) as i64 * 1000;
            if t < 12_000 {
                3000 + 12_000 - t
            } else {
                3000
            }
        };
        let (s, out) = run(3000, 1000, stall);
        assert_eq!(out.iter().map(|o| o.1).sum::<i32>(), 3000, "nothing lost");
        // Once it has learned the network (~0.5 s), the cursor keeps moving.
        let settled = 1_000_000;
        assert!(
            longest_freeze(&out, settled) <= 2000,
            "froze {} µs",
            longest_freeze(&out, settled)
        );
        let biggest = out.iter().filter(|o| o.0 >= settled).map(|o| o.1).max().unwrap();
        assert!(biggest <= 3, "jumped {biggest} px");
        assert!(
            s.delay_us() > 9_000 && s.delay_us() <= MAX_DELAY_US,
            "delay {}",
            s.delay_us()
        );
    }

    #[test]
    fn never_holds_back_more_than_a_frame() {
        // Stalls far longer than the cap: they still jump, but nothing waits
        // longer than the cap.
        let stall = |i: u32| {
            if i % 100 < 60 {
                3000 + (60 - (i % 100) as i64) * 1000
            } else {
                3000
            }
        };
        let mut s = Smoother::default();
        for i in 0..2000u32 {
            let now = i as i64 * 1000 + stall(i);
            s.push(i * 1000, now, 1, 0);
            assert!(s.next_due().unwrap() <= now + MAX_DELAY_US);
            s.take_due(now);
        }
        assert!(s.delay_us() <= MAX_DELAY_US);
    }

    #[test]
    fn take_all_and_clear() {
        let mut s = Smoother::default();
        // Learn a jittery link first.
        for i in 0..512u32 {
            let late = if i % 2 == 0 { 0 } else { 10_000 };
            s.push(i * 1000, i as i64 * 1000 + late, 0, 0);
            s.take_all();
        }
        s.push(600_000, 600_000, 3, 4);
        assert!(s.next_due().is_some(), "held back");
        assert_eq!(s.take_all(), (3, 4));
        assert_eq!(s.next_due(), None);
        s.push(601_000, 601_000, 1, 1);
        s.clear();
        assert_eq!(s.take_all(), (0, 0));
    }
}

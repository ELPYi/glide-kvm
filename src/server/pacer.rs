//! Mouse-motion pacing.
//!
//! Gaming mice report 1,000–8,000 times a second. Sending each report as its
//! own packet floods Wi-Fi and makes the receiving PC inject moves faster
//! than Windows can process them during a window drag. Instead, motion is
//! summed and sent at most once per interval, chosen per link:
//!
//! - [`FAST_INTERVAL`] (0.25 ms) on wired-class links: barely any added delay.
//! - [`MOTION_INTERVAL`] (1 ms) otherwise: Wi-Fi does better with fewer,
//!   fuller packets than with a flood of tiny ones.
//!
//! The first movement after a pause always goes out immediately. Pure data
//! structure; the server decides when to send.

use std::time::{Duration, Instant};

/// Default: up to 1,000 motion packets per second.
pub const MOTION_INTERVAL: Duration = Duration::from_millis(1);
/// Wired-class links: up to 4,000 per second.
pub const FAST_INTERVAL: Duration = Duration::from_micros(250);

#[derive(Debug)]
pub struct MotionBuf {
    dx: i32,
    dy: i32,
    last_sent: Option<Instant>,
    interval: Duration,
}

impl Default for MotionBuf {
    fn default() -> Self {
        MotionBuf {
            dx: 0,
            dy: 0,
            last_sent: None,
            interval: MOTION_INTERVAL,
        }
    }
}

impl MotionBuf {
    pub fn set_interval(&mut self, interval: Duration) {
        self.interval = interval;
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn add(&mut self, dx: i32, dy: i32) {
        self.dx = self.dx.saturating_add(dx);
        self.dy = self.dy.saturating_add(dy);
    }

    pub fn pending(&self) -> bool {
        self.dx != 0 || self.dy != 0
    }

    /// Enough time has passed since the last send to send again.
    pub fn ready(&self, now: Instant) -> bool {
        self.last_sent.is_none_or(|t| now.duration_since(t) >= self.interval)
    }

    /// When the pending motion may go out.
    pub fn due_at(&self) -> Option<Instant> {
        self.last_sent.map(|t| t + self.interval)
    }

    /// Take the accumulated motion for sending at `now`.
    pub fn take(&mut self, now: Instant) -> Option<(i32, i32)> {
        if !self.pending() {
            return None;
        }
        let out = (self.dx, self.dy);
        self.dx = 0;
        self.dy = 0;
        self.last_sent = Some(now);
        Some(out)
    }

    /// Drop anything pending (input left this client).
    pub fn clear(&mut self) {
        self.dx = 0;
        self.dy = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_motion_goes_immediately_then_paced() {
        let t0 = Instant::now();
        let mut m = MotionBuf::default();
        assert!(m.ready(t0));
        m.add(3, -1);
        assert_eq!(m.take(t0), Some((3, -1)));

        // Bursts within the interval accumulate...
        let t1 = t0 + Duration::from_micros(125);
        m.add(2, 0);
        m.add(2, 1);
        assert!(!m.ready(t1));
        assert_eq!(m.due_at(), Some(t0 + MOTION_INTERVAL));
        // ...and go out together, losing nothing.
        assert!(m.ready(t0 + MOTION_INTERVAL));
        assert_eq!(m.take(t0 + MOTION_INTERVAL), Some((4, 1)));
        assert_eq!(m.take(t0 + MOTION_INTERVAL), None);
    }

    #[test]
    fn fast_links_send_more_often() {
        let t0 = Instant::now();
        let mut m = MotionBuf::default();
        m.set_interval(FAST_INTERVAL);
        m.add(1, 0);
        m.take(t0);
        m.add(1, 0);
        assert!(m.ready(t0 + FAST_INTERVAL), "0.25 ms later is fine on a wired link");
        assert!(!m.ready(t0 + Duration::from_micros(100)));
    }

    #[test]
    fn clear_drops_pending() {
        let mut m = MotionBuf::default();
        m.add(5, 5);
        m.clear();
        assert!(!m.pending());
    }
}

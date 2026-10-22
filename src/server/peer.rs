//! Per-client network state on the server.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::config::ClientEntry;
use crate::input::InputBlock;
use crate::net::crypto::ReplayWindow;
use crate::net::protocol::ReliableEv;
use crate::net::reliable::Outbox;
use crate::status::PeerStatus;

/// No pong for this long = disconnected.
pub const ALIVE_TIMEOUT: Duration = Duration::from_millis(1000);
const OUTBOX_CAPACITY: usize = 4096;
/// Round trips kept for the latency stats (~1.6 s of heartbeats).
const RTT_SAMPLES: usize = 64;
/// Below this typical round trip, the link is wired-class and gets faster
/// mouse updates (see `pacer`).
const FAST_LINK_MS: f32 = 1.5;

pub struct Peer {
    pub entry: ClientEntry,
    /// Where we send. From DNS, then pinned to wherever pongs come from.
    pub addr: Option<SocketAddr>,
    /// The address we last asked the OS to give network priority to.
    pub prioritized: Option<SocketAddr>,
    pub last_pong: Option<Instant>,
    /// For logging connect/disconnect transitions once.
    pub was_alive: bool,
    /// Identifies one run of the client; a change means it restarted.
    pub instance: u64,
    pub replay: ReplayWindow,
    pub outbox: Outbox<ReliableEv>,
    pub last_heartbeat: Option<Instant>,
    /// Latest report: it can't accept input (from every pong).
    pub blocked: Option<InputBlock>,
    rtt: RttStats,
}

impl Peer {
    pub fn new(entry: ClientEntry) -> Self {
        Peer {
            entry,
            addr: None,
            prioritized: None,
            last_pong: None,
            was_alive: false,
            instance: 0,
            replay: ReplayWindow::default(),
            outbox: Outbox::new(OUTBOX_CAPACITY),
            last_heartbeat: None,
            blocked: None,
            rtt: RttStats::default(),
        }
    }

    pub fn alive(&self) -> bool {
        self.last_pong.is_some_and(|t| t.elapsed() < ALIVE_TIMEOUT)
    }

    /// The client restarted: its counters and expected sequence start over.
    pub fn reset_session(&mut self, instance: u64) {
        self.instance = instance;
        self.replay = ReplayWindow::default();
        self.outbox.reset();
        self.rtt = RttStats::default();
    }

    pub fn record_rtt(&mut self, sample_ms: f32) {
        self.rtt.push(sample_ms);
    }

    /// Wired-class latency: safe to send mouse updates more often.
    pub fn fast_link(&self) -> bool {
        self.rtt.median().is_some_and(|m| m < FAST_LINK_MS)
    }

    pub fn status(&self) -> PeerStatus {
        let connected = self.alive();
        PeerStatus {
            name: self.entry.name.clone(),
            address: self.entry.address.clone(),
            side: self.entry.side,
            resolved: self.addr,
            connected,
            rtt_ms: self.rtt.median().filter(|_| connected),
            rtt_worst_ms: self.rtt.worst().filter(|_| connected),
            blocked: self.blocked.filter(|_| connected),
            sound: false, // filled in by the server, which owns the player
        }
    }
}

/// Recent round-trip times: the typical value (median) and the spikes
/// (95th percentile), which is what makes a mouse feel laggy on Wi-Fi.
#[derive(Default)]
struct RttStats {
    samples: VecDeque<f32>,
}

impl RttStats {
    fn push(&mut self, ms: f32) {
        if self.samples.len() == RTT_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(ms);
    }

    fn percentile(&self, p: f32) -> Option<f32> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted: Vec<f32> = self.samples.iter().copied().collect();
        sorted.sort_by(f32::total_cmp);
        let i = ((sorted.len() - 1) as f32 * p).round() as usize;
        Some(sorted[i])
    }

    fn median(&self) -> Option<f32> {
        self.percentile(0.5)
    }

    fn worst(&self) -> Option<f32> {
        self.percentile(0.95)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Side;

    fn peer() -> Peer {
        Peer::new(ClientEntry {
            name: "x".into(),
            address: "x".into(),
            side: Side::Left,
        })
    }

    #[test]
    fn rtt_stats_show_typical_and_spikes() {
        let mut s = RttStats::default();
        assert_eq!(s.median(), None);
        for _ in 0..19 {
            s.push(5.0);
        }
        s.push(40.0); // one Wi-Fi hiccup
        assert_eq!(s.median(), Some(5.0), "a spike doesn't move the typical value");
        for _ in 0..3 {
            s.push(40.0);
        }
        assert_eq!(s.worst(), Some(40.0), "repeated spikes show up as the worst case");
    }

    #[test]
    fn stats_keep_only_recent_samples() {
        let mut s = RttStats::default();
        for _ in 0..RTT_SAMPLES {
            s.push(50.0);
        }
        for _ in 0..RTT_SAMPLES {
            s.push(1.0);
        }
        assert_eq!(s.worst(), Some(1.0));
    }

    #[test]
    fn fast_link_detection() {
        let mut p = peer();
        assert!(!p.fast_link(), "unknown = assume slow");
        for _ in 0..10 {
            p.record_rtt(0.4);
        }
        assert!(p.fast_link());
        p.reset_session(7);
        assert!(!p.fast_link());
        for _ in 0..10 {
            p.record_rtt(5.5);
        }
        assert!(!p.fast_link(), "Wi-Fi");
    }
}

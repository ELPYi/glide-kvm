//! Ordered, acknowledged delivery on top of UDP: just enough TCP-like
//! behaviour for keys, clicks and scrolls, without TCP's head-of-line delays
//! for mouse motion (which bypasses this entirely).
//!
//! - [`Outbox`] (sender): numbers messages, keeps them until acked, and says
//!   which ones are due for retransmission.
//! - [`Inbox`] (receiver): buffers out-of-order arrivals and releases them in
//!   sequence; reports the cumulative ack point.
//!
//! Both are pure data structures (no sockets, no clocks of their own), so
//! they are tested directly.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

/// Sequence numbers start at 1; an ack of 0 means "nothing yet".
const FIRST_SEQ: u32 = 1;

pub struct Outbox<T> {
    next_seq: u32,
    pending: VecDeque<Pending<T>>,
    capacity: usize,
}

struct Pending<T> {
    seq: u32,
    msg: T,
    last_sent: Instant,
}

impl<T: Clone> Outbox<T> {
    pub fn new(capacity: usize) -> Self {
        Outbox {
            next_seq: FIRST_SEQ,
            pending: VecDeque::new(),
            capacity,
        }
    }

    /// Queue a message (already sent once by the caller at `now`); returns its seq.
    /// If the receiver has been unreachable long enough to fill the queue, the
    /// oldest message is dropped.
    pub fn push(&mut self, msg: T, now: Instant) -> u32 {
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.pending.len() == self.capacity {
            self.pending.pop_front();
        }
        self.pending.push_back(Pending {
            seq,
            msg,
            last_sent: now,
        });
        seq
    }

    /// Cumulative ack: drop everything up to and including `seq`.
    pub fn ack(&mut self, seq: u32) {
        while self.pending.front().is_some_and(|p| p.seq <= seq) {
            self.pending.pop_front();
        }
    }

    /// Messages not acked within `after` of their last send; marks them resent at `now`.
    pub fn due(&mut self, now: Instant, after: Duration) -> Vec<(u32, T)> {
        self.pending
            .iter_mut()
            .filter(|p| now.duration_since(p.last_sent) >= after)
            .map(|p| {
                p.last_sent = now;
                (p.seq, p.msg.clone())
            })
            .collect()
    }

    /// When the next retransmission falls due, if anything is waiting.
    pub fn next_due(&self, after: Duration) -> Option<Instant> {
        self.pending.iter().map(|p| p.last_sent + after).min()
    }

    /// Start over (the receiver restarted and expects seq 1 again).
    pub fn reset(&mut self) {
        self.next_seq = FIRST_SEQ;
        self.pending.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

pub struct Inbox<T> {
    next: u32,
    buffer: BTreeMap<u32, T>,
    capacity: usize,
}

impl<T> Inbox<T> {
    pub fn new(capacity: usize) -> Self {
        Inbox {
            next: FIRST_SEQ,
            buffer: BTreeMap::new(),
            capacity,
        }
    }

    /// Accept a message; returns every message now deliverable, in order.
    /// Duplicates and already-delivered seqs return nothing.
    pub fn accept(&mut self, seq: u32, msg: T) -> Vec<T> {
        if seq >= self.next && self.buffer.len() < self.capacity {
            self.buffer.insert(seq, msg);
        }
        let mut ready = Vec::new();
        while let Some(m) = self.buffer.remove(&self.next) {
            ready.push(m);
            self.next += 1;
        }
        ready
    }

    /// The cumulative ack to send: highest seq delivered in order.
    pub fn ack_point(&self) -> u32 {
        self.next - 1
    }

    pub fn reset(&mut self) {
        self.next = FIRST_SEQ;
        self.buffer.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn outbox_retransmits_until_acked() {
        let t0 = Instant::now();
        let mut out = Outbox::new(16);
        assert_eq!(out.push("a", t0), 1);
        assert_eq!(out.push("b", t0), 2);

        assert!(out.due(t0 + 10 * MS, 30 * MS).is_empty(), "too early");
        assert_eq!(out.due(t0 + 30 * MS, 30 * MS), vec![(1, "a"), (2, "b")]);
        assert!(out.due(t0 + 40 * MS, 30 * MS).is_empty(), "just resent");

        out.ack(1);
        assert_eq!(out.due(t0 + 60 * MS, 30 * MS), vec![(2, "b")]);
        out.ack(2);
        assert!(out.is_empty());
        assert_eq!(out.next_due(30 * MS), None, "nothing to wake up for");
    }

    #[test]
    fn next_due_is_the_earliest_retransmission() {
        let t0 = Instant::now();
        let mut out = Outbox::new(16);
        out.push("a", t0);
        out.push("b", t0 + 10 * MS);
        assert_eq!(out.next_due(30 * MS), Some(t0 + 30 * MS));
        out.ack(1);
        assert_eq!(out.next_due(30 * MS), Some(t0 + 40 * MS));
    }

    #[test]
    fn outbox_drops_oldest_when_full_and_resets() {
        let t0 = Instant::now();
        let mut out = Outbox::new(2);
        out.push(1, t0);
        out.push(2, t0);
        out.push(3, t0);
        let seqs: Vec<u32> = out
            .due(t0 + Duration::from_secs(1), MS)
            .into_iter()
            .map(|x| x.0)
            .collect();
        assert_eq!(seqs, vec![2, 3]);
        out.reset();
        assert!(out.is_empty());
        assert_eq!(out.push(9, t0), 1);
    }

    #[test]
    fn inbox_reorders_and_dedups() {
        let mut inb = Inbox::new(16);
        assert_eq!(inb.ack_point(), 0);
        assert!(inb.accept(2, 'b').is_empty(), "waits for 1");
        assert_eq!(inb.ack_point(), 0);
        assert_eq!(inb.accept(1, 'a'), vec!['a', 'b']);
        assert_eq!(inb.ack_point(), 2);
        assert!(inb.accept(1, 'a').is_empty(), "duplicate");
        assert!(inb.accept(2, 'b').is_empty(), "duplicate");
        assert_eq!(inb.accept(3, 'c'), vec!['c']);
        inb.reset();
        assert_eq!(inb.accept(1, 'x'), vec!['x']);
    }
}

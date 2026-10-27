//! Moving one clipboard (a few bytes to tens of MB) over UDP: cut into
//! chunks, a window of them in flight, the receiver acking cumulatively
//! ("everything below `next` arrived"). When acks stop advancing the sender
//! resends just the first missing chunk; each following ack that still
//! stops short of what was sent points at the next hole, resent at once.
//! Pure: time is passed in.

use std::time::{Duration, Instant};

/// Payload per packet (stays well under any path's MTU after headers).
pub const CHUNK: usize = 1100;
/// Chunks in flight before an ack (~70 KB).
const WINDOW: u32 = 64;
/// No ack progress for this long: resend from the first missing chunk.
const RESEND_AFTER: Duration = Duration::from_millis(150);
/// No progress at all for this long: give up (the other PC is gone).
const GIVE_UP_AFTER: Duration = Duration::from_secs(10);

pub struct Outgoing {
    pub id: u32,
    chunks: Vec<Vec<u8>>,
    /// Every chunk below this has arrived.
    acked: u32,
    /// Every chunk below this has been sent (at least once).
    sent: u32,
    /// A lost chunk to send again before anything new.
    resend: Option<u32>,
    /// Recovering from a loss until everything sent before it is acked:
    /// each ack that still stops short points at the next hole.
    recover_to: Option<u32>,
    /// When the receiver last made progress (or the transfer started).
    progress: Instant,
    /// When the resend timer was last (re)started.
    timer: Instant,
}

impl Outgoing {
    pub fn new(id: u32, data: &[u8], now: Instant) -> Outgoing {
        let chunks = if data.is_empty() {
            vec![Vec::new()]
        } else {
            data.chunks(CHUNK).map(<[u8]>::to_vec).collect()
        };
        Outgoing {
            id,
            chunks,
            acked: 0,
            sent: 0,
            resend: None,
            recover_to: None,
            progress: now,
            timer: now,
        }
    }

    pub fn total(&self) -> u32 {
        self.chunks.len() as u32
    }

    pub fn chunk(&self, index: u32) -> &[u8] {
        &self.chunks[index as usize]
    }

    /// The next chunk to send now, if any (call repeatedly, at the pace
    /// the caller wants).
    pub fn next_to_send(&mut self, now: Instant) -> Option<u32> {
        if self.sent > self.acked && now.duration_since(self.timer) >= RESEND_AFTER {
            // Acks stopped: the first missing chunk was lost (or its ack).
            self.resend = Some(self.acked);
            self.recover_to = Some(self.sent);
            self.timer = now;
        }
        if let Some(i) = self.resend.take() {
            return Some(i);
        }
        if self.sent < self.total().min(self.acked + WINDOW) {
            self.sent += 1;
            Some(self.sent - 1)
        } else {
            None
        }
    }

    pub fn ack(&mut self, next: u32, now: Instant) {
        if next <= self.acked {
            return;
        }
        self.acked = next.min(self.total());
        self.sent = self.sent.max(self.acked);
        self.progress = now;
        self.timer = now;
        match self.recover_to {
            // Still short of what was sent before the loss: another hole.
            Some(to) if self.acked < to => self.resend = Some(self.acked),
            _ => self.recover_to = None,
        }
    }

    pub fn done(&self) -> bool {
        self.acked >= self.total()
    }

    pub fn stalled(&self, now: Instant) -> bool {
        now.duration_since(self.progress) >= GIVE_UP_AFTER
    }
}

pub struct Incoming {
    pub id: u32,
    chunks: Vec<Option<Vec<u8>>>,
    /// Every chunk below this has arrived.
    next: u32,
}

impl Incoming {
    /// `None` if `total` is implausible.
    pub fn new(id: u32, total: u32) -> Option<Incoming> {
        let max = (super::MAX_SIZE / CHUNK + 2) as u32;
        (1..=max).contains(&total).then(|| Incoming {
            id,
            chunks: vec![None; total as usize],
            next: 0,
        })
    }

    pub fn total(&self) -> u32 {
        self.chunks.len() as u32
    }

    /// Store a chunk. Returns the cumulative ack to send.
    pub fn add(&mut self, index: u32, data: &[u8]) -> u32 {
        if let Some(slot) = self.chunks.get_mut(index as usize) {
            if slot.is_none() && data.len() <= CHUNK {
                *slot = Some(data.to_vec());
            }
        }
        while self.chunks.get(self.next as usize).is_some_and(Option::is_some) {
            self.next += 1;
        }
        self.next
    }

    pub fn complete(&self) -> bool {
        self.next >= self.total()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.chunks.into_iter().flatten().flatten().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a transfer through a link that drops every `drop_every`th packet
    /// (both ways). Returns what arrived and how many sends it took.
    fn transfer(data: &[u8], drop_every: usize) -> (Vec<u8>, usize) {
        let mut packets = 0usize;
        transfer_with(data, move || {
            packets += 1;
            packets.is_multiple_of(drop_every)
        })
    }

    /// Same, with `lost()` deciding each packet's fate.
    fn transfer_with(data: &[u8], mut lost: impl FnMut() -> bool) -> (Vec<u8>, usize) {
        let mut now = Instant::now();
        let mut out = Outgoing::new(9, data, now);
        let mut inc = Incoming::new(9, out.total()).unwrap();
        let mut sends = 0;
        while !out.done() {
            assert!(!out.stalled(now), "stalled");
            while let Some(i) = out.next_to_send(now) {
                sends += 1;
                if lost() {
                    continue; // chunk lost
                }
                let ack = inc.add(i, out.chunk(i));
                if !lost() {
                    out.ack(ack, now);
                }
            }
            now += Duration::from_millis(50);
        }
        assert!(inc.complete());
        (inc.into_bytes(), sends)
    }

    #[test]
    fn arrives_whole_despite_losses() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let (got, sends) = transfer(&data, 1_000_000);
        assert_eq!(got, data);
        assert_eq!(sends, data.len().div_ceil(CHUNK), "no loss, no resends");

        for drop_every in [3, 7, 64, 65] {
            let (got, sends) = transfer(&data, drop_every);
            assert_eq!(got, data, "one packet in {drop_every} lost");
            assert!(sends < data.len().div_ceil(CHUNK) * 3, "{sends} sends");
        }
    }

    #[test]
    fn survives_heavy_random_loss() {
        let data: Vec<u8> = (0..300_000u32).map(|i| (i % 249) as u8).collect();
        let mut seed = 12345u32;
        let (got, _) = transfer_with(&data, || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16).is_multiple_of(5) // 20% of packets lost, both ways
        });
        assert_eq!(got, data);
    }

    #[test]
    fn tiny_and_empty() {
        assert_eq!(transfer(b"hi", 1_000_000).0, b"hi");
        assert_eq!(transfer(b"", 1_000_000).0, b"");
    }

    #[test]
    fn rejects_absurd_sizes() {
        assert!(Incoming::new(1, 0).is_none());
        assert!(Incoming::new(1, u32::MAX).is_none());
    }
}

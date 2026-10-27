//! The clipboard worker: one background thread per running glide. Local
//! copies go out to every other PC; clipboards arriving from one PC are put
//! on this one and (on the sharing PC) passed on to the rest.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;

use super::transfer::{Incoming, Outgoing};
use super::{ClipData, MAX_SIZE};
use crate::log;

/// Chunks sent per tick while transferring (~4 MB/s): fast for a screenshot,
/// gentle enough that a mouse packet never waits behind a long burst.
const CHUNKS_PER_TICK: usize = 4;
const TICK: Duration = Duration::from_millis(1);
const IDLE_WAKE: Duration = Duration::from_millis(500);

/// One clipboard message, in either direction.
#[derive(Debug, Clone, PartialEq)]
pub enum ClipMsg {
    Chunk {
        id: u32,
        index: u32,
        total: u32,
        data: Vec<u8>,
    },
    Ack {
        id: u32,
        next: u32,
    },
}

/// How the worker reaches the other PCs.
pub trait ClipNet: Send {
    /// The PCs to share with right now.
    fn peers(&self) -> Vec<SocketAddr>;
    fn send(&self, to: SocketAddr, msg: ClipMsg);
}

/// This PC's clipboard.
pub trait Clipboard: Send {
    fn set(&self, data: ClipData);
}

/// Called with each copy made on this PC.
pub type OnCopy = Box<dyn Fn(ClipData) + Send>;

/// Starts watching this PC's clipboard, calling `on_copy` for every change
/// made by another app (never for glide's own `set`).
pub type OpenClipboard = Box<dyn FnOnce(OnCopy) -> Result<Box<dyn Clipboard>> + Send>;

enum Event {
    Copied(ClipData),
    Net(SocketAddr, ClipMsg),
}

/// Feeds packets from the network thread to the worker. Never blocks.
#[derive(Clone)]
pub struct ClipInbox(SyncSender<Event>);

impl ClipInbox {
    pub fn push(&self, from: SocketAddr, msg: ClipMsg) {
        let _ = self.0.try_send(Event::Net(from, msg));
    }
}

pub struct ClipSync {
    tx: SyncSender<Event>,
    enabled: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ClipSync {
    pub fn start(enabled: bool, net: Box<dyn ClipNet>, open: OpenClipboard) -> ClipSync {
        let (tx, rx) = mpsc::sync_channel(1024);
        let enabled = Arc::new(AtomicBool::new(enabled));
        let running = Arc::new(AtomicBool::new(true));
        let (e, r, copies) = (enabled.clone(), running.clone(), tx.clone());
        let thread = std::thread::Builder::new()
            .name("glide-clipboard".into())
            .spawn(move || {
                let on_copy = Box::new(move |d: ClipData| {
                    let _ = copies.try_send(Event::Copied(d));
                });
                let board = match open(on_copy) {
                    Ok(b) => Some(b),
                    Err(e) => {
                        log!("clipboard sharing is off: {e:#}");
                        None
                    }
                };
                Worker {
                    net,
                    board,
                    enabled: e,
                    out: HashMap::new(),
                    inc: HashMap::new(),
                    next_id: crate::now_micros() as u32,
                    last: 0,
                }
                .run(&rx, &r);
            })
            .ok();
        ClipSync {
            tx,
            enabled,
            running,
            thread,
        }
    }

    pub fn inbox(&self) -> ClipInbox {
        ClipInbox(self.tx.clone())
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }
}

impl Drop for ClipSync {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Worker {
    net: Box<dyn ClipNet>,
    board: Option<Box<dyn Clipboard>>,
    enabled: Arc<AtomicBool>,
    out: HashMap<SocketAddr, Outgoing>,
    inc: HashMap<SocketAddr, Incoming>,
    next_id: u32,
    /// Hash of the clipboard last sent or received, so the same content
    /// isn't bounced back and forth.
    last: u64,
}

fn digest(d: &ClipData) -> u64 {
    let mut h = DefaultHasher::new();
    d.hash(&mut h);
    h.finish()
}

impl Worker {
    fn run(&mut self, rx: &Receiver<Event>, running: &AtomicBool) {
        while running.load(Ordering::Relaxed) {
            let wait = if self.out.is_empty() { IDLE_WAKE } else { TICK };
            match rx.recv_timeout(wait) {
                Ok(ev) => {
                    self.handle(ev);
                    while let Ok(ev) = rx.try_recv() {
                        self.handle(ev);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.send_some();
        }
    }

    fn handle(&mut self, ev: Event) {
        let on = self.enabled.load(Ordering::Relaxed);
        match ev {
            Event::Copied(d) => {
                if on {
                    self.share(&d, None);
                }
            }
            Event::Net(from, ClipMsg::Ack { id, next }) => {
                if let Some(o) = self.out.get_mut(&from).filter(|o| o.id == id) {
                    o.ack(next, Instant::now());
                }
            }
            Event::Net(from, ClipMsg::Chunk { id, index, total, data }) => {
                if !on {
                    return;
                }
                if self.inc.get(&from).is_none_or(|i| i.id != id || i.total() != total) {
                    // A new clipboard from that PC replaces any half-received one.
                    match Incoming::new(id, total) {
                        Some(i) => self.inc.insert(from, i),
                        None => return,
                    };
                }
                let inc = self.inc.get_mut(&from).expect("inserted above");
                let next = inc.add(index, &data);
                self.net.send(from, ClipMsg::Ack { id, next });
                if inc.complete() {
                    let bytes = self.inc.remove(&from).expect("present").into_bytes();
                    match ClipData::decode(&bytes) {
                        Ok(d) => {
                            if let Some(b) = &self.board {
                                b.set(d.clone());
                            }
                            self.share(&d, Some(from));
                        }
                        Err(e) => log!("{e:#}"),
                    }
                }
            }
        }
    }

    /// Send `d` to every PC except `from` (where it came from).
    fn share(&mut self, d: &ClipData, from: Option<SocketAddr>) {
        let h = digest(d);
        if d.is_empty() || d.size() > MAX_SIZE || (from.is_none() && h == self.last) {
            return;
        }
        let peers: Vec<SocketAddr> = self.net.peers().into_iter().filter(|p| Some(*p) != from).collect();
        if peers.is_empty() {
            return;
        }
        self.last = h;
        let wire = d.encode();
        let now = Instant::now();
        for p in peers {
            self.next_id = self.next_id.wrapping_add(1);
            self.out.insert(p, Outgoing::new(self.next_id, &wire, now));
        }
    }

    fn send_some(&mut self) {
        let now = Instant::now();
        self.out.retain(|_, o| !o.done() && !o.stalled(now));
        for (&to, o) in self.out.iter_mut() {
            for _ in 0..CHUNKS_PER_TICK {
                let Some(index) = o.next_to_send(now) else { break };
                self.net.send(
                    to,
                    ClipMsg::Chunk {
                        id: o.id,
                        index,
                        total: o.total(),
                        data: o.chunk(index).to_vec(),
                    },
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Two PCs, A and B, wired back to back (every 5th packet lost).
    struct Wire {
        me: SocketAddr,
        other: Arc<Mutex<Option<ClipInbox>>>,
        sent: Mutex<usize>,
    }

    impl ClipNet for Wire {
        fn peers(&self) -> Vec<SocketAddr> {
            vec!["10.0.0.2:1".parse().unwrap()]
        }
        fn send(&self, _to: SocketAddr, msg: ClipMsg) {
            let mut n = self.sent.lock().unwrap();
            *n += 1;
            if !(*n).is_multiple_of(5) {
                if let Some(i) = self.other.lock().unwrap().as_ref() {
                    i.push(self.me, msg);
                }
            }
        }
    }

    #[derive(Clone, Default)]
    struct Board {
        set: Arc<Mutex<Vec<ClipData>>>,
        on_copy: Arc<Mutex<Option<OnCopy>>>,
    }

    impl Clipboard for Board {
        fn set(&self, data: ClipData) {
            self.set.lock().unwrap().push(data);
        }
    }

    impl Board {
        fn copy(&self, d: ClipData) {
            (self.on_copy.lock().unwrap().as_ref().unwrap())(d);
        }
        fn opener(&self) -> OpenClipboard {
            let b = self.clone();
            Box::new(move |on_copy| {
                *b.on_copy.lock().unwrap() = Some(on_copy);
                Ok(Box::new(b) as Box<dyn Clipboard>)
            })
        }
    }

    fn wait_until(mut f: impl FnMut() -> bool) {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_copy_on_one_pc_lands_on_the_other() {
        let (to_a, to_b) = (Arc::new(Mutex::new(None)), Arc::new(Mutex::new(None)));
        let wire = |me: &str, other: &Arc<Mutex<Option<ClipInbox>>>| {
            Box::new(Wire {
                me: me.parse().unwrap(),
                other: other.clone(),
                sent: Mutex::new(0),
            })
        };
        let (board_a, board_b) = (Board::default(), Board::default());
        let a = ClipSync::start(true, wire("10.0.0.1:1", &to_b), board_a.opener());
        let b = ClipSync::start(true, wire("10.0.0.2:1", &to_a), board_b.opener());
        *to_a.lock().unwrap() = Some(a.inbox());
        *to_b.lock().unwrap() = Some(b.inbox());
        wait_until(|| board_a.on_copy.lock().unwrap().is_some() && board_b.on_copy.lock().unwrap().is_some());

        let picture: Vec<u8> = (0..3_000_000u32).map(|i| (i % 253) as u8 ^ (i >> 12) as u8).collect();
        let d = ClipData {
            text: Some("copied on A".into()),
            image: Some(picture),
            ..Default::default()
        };
        board_a.copy(d.clone());
        wait_until(|| board_b.set.lock().unwrap().len() == 1);
        assert_eq!(board_b.set.lock().unwrap()[0], d);

        // Turned off on B: nothing arrives.
        b.set_enabled(false);
        board_a.copy(ClipData {
            text: Some("second".into()),
            ..Default::default()
        });
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(board_b.set.lock().unwrap().len(), 1);
    }
}

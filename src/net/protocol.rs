//! Wire messages (plaintext, before encryption).
//!
//! Hand-rolled little-endian encoding: tiny, allocation-light, and with no
//! serde overhead on the hot path. Every message starts with a one-byte tag.
//! Changing a layout here is a protocol break: bump the `glide-v4` label in
//! `crypto.rs` so old and new builds refuse each other cleanly. Adding is
//! not: decoders ignore trailing bytes and unknown tags, so a field appended
//! to the end (read as optional) or a new message keeps old builds working.

use crate::clip::sync::ClipMsg;
use crate::input::{Button, InputBlock, InputEvent, PointerFeel, Side};

/// Server -> client.
#[derive(Debug, Clone, PartialEq)]
pub enum ServerMsg {
    /// Sent every 25-200 ms; the client echoes `sent_micros` back for RTT.
    /// `wants_sound`: send this PC your sound (appended in 0.4.7; older
    /// servers don't send it, which reads as no).
    Heartbeat { sent_micros: u64, wants_sound: bool },
    /// Unreliable: a lost motion packet is just a skipped pixel or two.
    Motion { dx: i32, dy: i32 },
    /// Delivered in order, acked and retransmitted (see [`super::reliable`]).
    Reliable { seq: u32, ev: ReliableEv },
    /// Part of a clipboard, or an ack for one (see `crate::clip`).
    Clip(ClipMsg),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ReliableEv {
    /// Client becomes active; place its cursor on `side` at `frac` along that
    /// edge. `feel`: pointer settings to mirror while active (None = keep its own).
    Enter {
        side: Side,
        frac: f32,
        feel: Option<PointerFeel>,
    },
    /// Client becomes inactive; release everything held.
    Leave,
    /// Show a brief on-screen confirmation on the PC you're using.
    Notice(Notice),
    /// Key / button / wheel (never motion).
    Input(InputEvent),
}

/// Confirmations shown on screen (see `Desktop::notify`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    Locked = 1,
    Unlocked = 2,
}

impl Notice {
    fn from_u8(v: u8) -> Option<Notice> {
        match v {
            1 => Some(Notice::Locked),
            2 => Some(Notice::Unlocked),
            _ => None,
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            Notice::Locked => "Switching locked",
            Notice::Unlocked => "Switching unlocked",
        }
    }
}

/// Client -> server.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientMsg {
    /// Reply to a heartbeat. `instance` changes whenever the client restarts,
    /// telling the server to restart its reliable sequence numbering.
    /// `blocked`: this PC can't accept input right now (see [`InputBlock`]).
    Pong {
        instance: u64,
        echo_micros: u64,
        blocked: Option<InputBlock>,
    },
    /// Cumulative: every reliable message up to and including `seq` arrived.
    Ack { seq: u32 },
    /// The cursor is being pushed off this client's `side` edge.
    EdgeLeave { side: Side, frac: f32 },
    /// "I'm here." Sent by a client that knows the server's address before any
    /// session exists. Outbound traffic makes the client's own firewall accept
    /// the server's replies, so no inbound rule is needed.
    Knock,
    /// 10 ms of this PC's sound, Opus-compressed (see `crate::audio`).
    /// `stream` changes when the client restarts; `seq` counts frames.
    Audio { stream: u32, seq: u32, opus: Vec<u8> },
    /// Part of a clipboard, or an ack for one (see `crate::clip`).
    Clip(ClipMsg),
}

struct W(Vec<u8>);
impl W {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn f32(&mut self, v: f32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
}

struct R<'a>(&'a [u8]);
impl R<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (head, rest) = self.0.split_at_checked(N)?;
        self.0 = rest;
        head.try_into().ok()
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take::<1>()?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take()?))
    }
    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_le_bytes(self.take()?))
    }
    fn bool(&mut self) -> Option<bool> {
        Some(self.u8()? != 0)
    }
    fn side(&mut self) -> Option<Side> {
        Side::from_u8(self.u8()?)
    }
}

fn write_clip(w: &mut W, m: &ClipMsg) {
    match m {
        ClipMsg::Chunk { id, index, total, data } => {
            w.u8(1).u32(*id).u32(*index).u32(*total);
            w.0.extend_from_slice(data);
        }
        ClipMsg::Ack { id, next } => {
            w.u8(2).u32(*id).u32(*next);
        }
    }
}

fn read_clip(r: &mut R) -> Option<ClipMsg> {
    Some(match r.u8()? {
        1 => ClipMsg::Chunk {
            id: r.u32()?,
            index: r.u32()?,
            total: r.u32()?,
            data: r.0.to_vec(),
        },
        2 => ClipMsg::Ack {
            id: r.u32()?,
            next: r.u32()?,
        },
        _ => return None,
    })
}

fn write_input(w: &mut W, ev: &InputEvent) {
    match *ev {
        InputEvent::Key { code, down } => {
            w.u8(1).u16(code).u8(down as u8);
        }
        InputEvent::Button { button, down } => {
            w.u8(2).u8(button.to_u8()).u8(down as u8);
        }
        InputEvent::Wheel { dx, dy } => {
            w.u8(3).i32(dx).i32(dy);
        }
        InputEvent::Motion { dx, dy } => {
            w.u8(4).i32(dx).i32(dy);
        }
    }
}

fn read_input(r: &mut R) -> Option<InputEvent> {
    Some(match r.u8()? {
        1 => InputEvent::Key {
            code: r.u16()?,
            down: r.bool()?,
        },
        2 => InputEvent::Button {
            button: Button::from_u8(r.u8()?)?,
            down: r.bool()?,
        },
        3 => InputEvent::Wheel {
            dx: r.i32()?,
            dy: r.i32()?,
        },
        4 => InputEvent::Motion {
            dx: r.i32()?,
            dy: r.i32()?,
        },
        _ => return None,
    })
}

impl ServerMsg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::with_capacity(16));
        match self {
            ServerMsg::Heartbeat {
                sent_micros,
                wants_sound,
            } => {
                w.u8(0x01).u64(*sent_micros).u8(*wants_sound as u8);
            }
            ServerMsg::Motion { dx, dy } => {
                w.u8(0x02).i32(*dx).i32(*dy);
            }
            ServerMsg::Reliable { seq, ev } => {
                w.u8(0x03).u32(*seq);
                match ev {
                    ReliableEv::Enter { side, frac, feel } => {
                        w.u8(0x10).u8(side.to_u8()).f32(*frac);
                        match feel {
                            Some(f) => w.u8(1).u8(f.speed).u8(f.acceleration as u8),
                            None => w.u8(0),
                        };
                    }
                    ReliableEv::Leave => {
                        w.u8(0x11);
                    }
                    ReliableEv::Notice(n) => {
                        w.u8(0x13).u8(*n as u8);
                    }
                    ReliableEv::Input(ev) => {
                        w.u8(0x12);
                        write_input(&mut w, ev);
                    }
                }
            }
            ServerMsg::Clip(m) => {
                w.u8(0x04);
                write_clip(&mut w, m);
            }
        }
        w.0
    }

    pub fn decode(buf: &[u8]) -> Option<ServerMsg> {
        let mut r = R(buf);
        Some(match r.u8()? {
            0x01 => ServerMsg::Heartbeat {
                sent_micros: r.u64()?,
                wants_sound: r.u8().is_some_and(|b| b != 0),
            },
            0x02 => ServerMsg::Motion {
                dx: r.i32()?,
                dy: r.i32()?,
            },
            0x03 => {
                let seq = r.u32()?;
                let ev = match r.u8()? {
                    0x10 => ReliableEv::Enter {
                        side: r.side()?,
                        frac: r.f32()?,
                        feel: match r.u8()? {
                            0 => None,
                            _ => Some(PointerFeel {
                                speed: r.u8()?,
                                acceleration: r.bool()?,
                            }),
                        },
                    },
                    0x11 => ReliableEv::Leave,
                    0x13 => ReliableEv::Notice(Notice::from_u8(r.u8()?)?),
                    0x12 => ReliableEv::Input(read_input(&mut r)?),
                    _ => return None,
                };
                ServerMsg::Reliable { seq, ev }
            }
            0x04 => ServerMsg::Clip(read_clip(&mut r)?),
            _ => return None,
        })
    }
}

impl ClientMsg {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = W(Vec::with_capacity(20));
        match self {
            ClientMsg::Pong {
                instance,
                echo_micros,
                blocked,
            } => {
                w.u8(0x21)
                    .u64(*instance)
                    .u64(*echo_micros)
                    .u8(blocked.map_or(0, InputBlock::to_u8));
            }
            ClientMsg::Ack { seq } => {
                w.u8(0x22).u32(*seq);
            }
            ClientMsg::EdgeLeave { side, frac } => {
                w.u8(0x23).u8(side.to_u8()).f32(*frac);
            }
            ClientMsg::Knock => {
                w.u8(0x24);
            }
            ClientMsg::Audio { stream, seq, opus } => {
                w.u8(0x25).u32(*stream).u32(*seq);
                w.0.extend_from_slice(opus);
            }
            ClientMsg::Clip(m) => {
                w.u8(0x26);
                write_clip(&mut w, m);
            }
        }
        w.0
    }

    pub fn decode(buf: &[u8]) -> Option<ClientMsg> {
        let mut r = R(buf);
        Some(match r.u8()? {
            0x21 => ClientMsg::Pong {
                instance: r.u64()?,
                echo_micros: r.u64()?,
                blocked: InputBlock::from_u8(r.u8()?),
            },
            0x22 => ClientMsg::Ack { seq: r.u32()? },
            0x23 => ClientMsg::EdgeLeave {
                side: r.side()?,
                frac: r.f32()?,
            },
            0x24 => ClientMsg::Knock,
            0x25 => ClientMsg::Audio {
                stream: r.u32()?,
                seq: r.u32()?,
                opus: r.0.to_vec(),
            },
            0x26 => ClientMsg::Clip(read_clip(&mut r)?),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_messages_roundtrip() {
        let msgs = [
            ServerMsg::Heartbeat {
                sent_micros: 1_700_000_000_000_000,
                wants_sound: true,
            },
            ServerMsg::Heartbeat {
                sent_micros: 5,
                wants_sound: false,
            },
            ServerMsg::Motion { dx: -5, dy: 1200 },
            ServerMsg::Reliable {
                seq: 7,
                ev: ReliableEv::Enter {
                    side: Side::Left,
                    frac: 0.25,
                    feel: None,
                },
            },
            ServerMsg::Reliable {
                seq: 13,
                ev: ReliableEv::Notice(Notice::Locked),
            },
            ServerMsg::Reliable {
                seq: 12,
                ev: ReliableEv::Enter {
                    side: Side::Top,
                    frac: 1.0,
                    feel: Some(PointerFeel {
                        speed: 7,
                        acceleration: true,
                    }),
                },
            },
            ServerMsg::Reliable {
                seq: 8,
                ev: ReliableEv::Leave,
            },
            ServerMsg::Reliable {
                seq: 9,
                ev: ReliableEv::Input(InputEvent::Key { code: 30, down: true }),
            },
            ServerMsg::Reliable {
                seq: 10,
                ev: ReliableEv::Input(InputEvent::Button {
                    button: Button::Forward,
                    down: false,
                }),
            },
            ServerMsg::Reliable {
                seq: 11,
                ev: ReliableEv::Input(InputEvent::Wheel { dx: 0, dy: -120 }),
            },
            ServerMsg::Clip(ClipMsg::Chunk {
                id: 5,
                index: 2,
                total: 3,
                data: vec![9; 1100],
            }),
            ServerMsg::Clip(ClipMsg::Ack { id: 5, next: 3 }),
        ];
        for m in msgs {
            assert_eq!(ServerMsg::decode(&m.encode()), Some(m));
        }
    }

    #[test]
    fn client_messages_roundtrip() {
        let msgs = [
            ClientMsg::Pong {
                instance: 123456789,
                echo_micros: 42,
                blocked: None,
            },
            ClientMsg::Pong {
                instance: 1,
                echo_micros: 2,
                blocked: Some(InputBlock::SecureScreen),
            },
            ClientMsg::Ack { seq: 99 },
            ClientMsg::EdgeLeave {
                side: Side::Bottom,
                frac: 0.5,
            },
            ClientMsg::Knock,
            ClientMsg::Audio {
                stream: 77,
                seq: 4_000_000_000,
                opus: vec![1, 2, 3, 250],
            },
            ClientMsg::Clip(ClipMsg::Chunk {
                id: u32::MAX,
                index: 0,
                total: 1,
                data: Vec::new(),
            }),
            ClientMsg::Clip(ClipMsg::Ack { id: 1, next: 0 }),
        ];
        for m in msgs {
            assert_eq!(ClientMsg::decode(&m.encode()), Some(m));
        }
    }

    #[test]
    fn older_builds_still_understand_each_other() {
        // A 0.4.6 server's heartbeat (no sound byte) reads as "no sound".
        let mut old = vec![0x01];
        old.extend_from_slice(&9u64.to_le_bytes());
        assert_eq!(
            ServerMsg::decode(&old),
            Some(ServerMsg::Heartbeat {
                sent_micros: 9,
                wants_sound: false
            })
        );
    }

    #[test]
    fn truncated_or_unknown_is_rejected() {
        assert_eq!(ServerMsg::decode(&[0x02, 1]), None);
        assert_eq!(ServerMsg::decode(&[]), None);
        assert_eq!(ServerMsg::decode(&[0xFF]), None);
        assert_eq!(ClientMsg::decode(&[0x22, 1, 2]), None);
    }
}

//! Networking building blocks, independent of the OS and of server/client roles.
//!
//! - [`protocol`]: the messages and their byte encoding
//! - [`crypto`]: packet encryption, authentication and replay protection
//! - [`reliable`]: ordered, acknowledged delivery on top of UDP
//! - [`addr`]: resolving typed addresses; listing this PC's own
//! - [`pairing`]: connecting a new PC with a 6-digit code (SPAKE2)

pub mod addr;
pub mod crypto;
pub mod pairing;
pub mod protocol;
pub mod reliable;

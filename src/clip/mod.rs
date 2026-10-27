//! Shared clipboard: copy on one PC, paste on another.
//!
//! Whenever the clipboard changes on any PC, its text, formatted text (HTML)
//! and image are sent to the others, which put them on their own clipboard.
//! The sharing PC relays between clients. Things password managers mark as
//! private are never sent (see the platform layer).
//!
//! - [`ClipData`]: what's copied, and its compressed wire form.
//! - [`transfer`]: **pure** chunked transfer over the existing UDP link
//!   (windowed, cumulative acks, resends what's lost).
//! - [`sync`]: the background worker tying the OS clipboard to the network.
//!
//! It shares the socket with the mouse, so sending is paced (a few chunks per
//! millisecond) and never bursts ahead of a mouse packet for long.

pub mod sync;
pub mod transfer;

use anyhow::Result;

/// Clipboards bigger than this (uncompressed) aren't shared.
pub const MAX_SIZE: usize = 64 << 20;

/// What's on the clipboard, in the formats that are shared.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ClipData {
    pub text: Option<String>,
    /// Windows' "HTML Format" (formatted text from browsers and Office).
    pub html: Option<Vec<u8>>,
    /// A picture, as a Windows DIB (bitmap header + pixels).
    pub image: Option<Vec<u8>>,
}

const TAG_TEXT: u8 = 1;
const TAG_HTML: u8 = 2;
const TAG_IMAGE: u8 = 3;

impl ClipData {
    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.html.is_none() && self.image.is_none()
    }

    pub fn size(&self) -> usize {
        self.text.as_ref().map_or(0, String::len)
            + self.html.as_ref().map_or(0, Vec::len)
            + self.image.as_ref().map_or(0, Vec::len)
    }

    /// Compressed wire form: tagged, length-prefixed parts, then deflate.
    pub fn encode(&self) -> Vec<u8> {
        let mut raw = Vec::with_capacity(self.size() + 16);
        let mut part = |tag: u8, bytes: &[u8]| {
            raw.push(tag);
            raw.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            raw.extend_from_slice(bytes);
        };
        if let Some(t) = &self.text {
            part(TAG_TEXT, t.as_bytes());
        }
        if let Some(h) = &self.html {
            part(TAG_HTML, h);
        }
        if let Some(i) = &self.image {
            part(TAG_IMAGE, i);
        }
        // Level 1: fast, and screenshots still shrink several times over.
        miniz_oxide::deflate::compress_to_vec(&raw, 1)
    }

    pub fn decode(wire: &[u8]) -> Result<ClipData> {
        let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(wire, MAX_SIZE + 64)
            .map_err(|e| anyhow::anyhow!("clipboard data is corrupt ({e:?})"))?;
        let mut d = ClipData::default();
        let mut rest = raw.as_slice();
        while let [tag, a, b, c, e, tail @ ..] = rest {
            let len = u32::from_le_bytes([*a, *b, *c, *e]) as usize;
            anyhow::ensure!(tail.len() >= len, "clipboard data is truncated");
            let (bytes, next) = tail.split_at(len);
            match *tag {
                TAG_TEXT => d.text = Some(String::from_utf8_lossy(bytes).into_owned()),
                TAG_HTML => d.html = Some(bytes.to_vec()),
                TAG_IMAGE => d.image = Some(bytes.to_vec()),
                _ => {} // a format from a newer version: skip it
            }
            rest = next;
        }
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let d = ClipData {
            text: Some("héllo ✓".into()),
            html: Some(b"Version:0.9\r\n<b>hi</b>".to_vec()),
            image: Some(vec![7; 100_000]),
        };
        let wire = d.encode();
        assert!(wire.len() < 2_000, "compressed: {} bytes", wire.len());
        assert_eq!(ClipData::decode(&wire).unwrap(), d);

        let text_only = ClipData {
            text: Some("x".into()),
            ..Default::default()
        };
        assert_eq!(ClipData::decode(&text_only.encode()).unwrap(), text_only);
        assert!(ClipData::decode(b"not deflate").is_err());
    }
}

//! Opus, the codec Discord and Zoom use: 10 ms of CD-quality stereo in
//! about 160 bytes. The low-delay mode adds only 2.5 ms of its own.

use anyhow::{bail, Result};
use unsafe_libopus::{
    opus_decode_float, opus_decoder_create, opus_decoder_destroy, opus_encode_float, opus_encoder_create,
    opus_encoder_ctl, opus_encoder_destroy, OpusDecoder, OpusEncoder, OPUS_APPLICATION_RESTRICTED_LOWDELAY, OPUS_OK,
    OPUS_SET_BITRATE_REQUEST, OPUS_SET_COMPLEXITY_REQUEST,
};

use super::{CHANNELS, FRAME, FRAME_SAMPLES, SAMPLE_RATE};

/// Plenty for music; ~16 KB/s.
const BITRATE: i32 = 128_000;
/// 0-10. Lower is lighter on the PC's CPU; 5 is still near-transparent.
const COMPLEXITY: i32 = 5;
/// Opus never needs more than this for one 10 ms frame at our bitrate.
pub const MAX_PACKET: usize = 1000;

pub struct Encoder(*mut OpusEncoder);

// The state is plain memory owned by this value; only used from one thread at a time.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new() -> Result<Encoder> {
        let mut err = 0;
        let st = unsafe {
            opus_encoder_create(
                SAMPLE_RATE as i32,
                CHANNELS as i32,
                OPUS_APPLICATION_RESTRICTED_LOWDELAY,
                &mut err,
            )
        };
        if st.is_null() || err != OPUS_OK {
            bail!("couldn't start the Opus encoder (error {err})");
        }
        unsafe {
            opus_encoder_ctl!(st, OPUS_SET_BITRATE_REQUEST, BITRATE);
            opus_encoder_ctl!(st, OPUS_SET_COMPLEXITY_REQUEST, COMPLEXITY);
        }
        Ok(Encoder(st))
    }

    /// Compress one frame ([`FRAME_SAMPLES`] interleaved samples) into `out`.
    pub fn encode(&mut self, pcm: &[f32], out: &mut [u8]) -> Result<usize> {
        assert_eq!(pcm.len(), FRAME_SAMPLES);
        let n = unsafe { opus_encode_float(self.0, pcm.as_ptr(), FRAME as i32, out.as_mut_ptr(), out.len() as i32) };
        if n < 0 {
            bail!("Opus encode failed (error {n})");
        }
        Ok(n as usize)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { opus_encoder_destroy(self.0) };
    }
}

pub struct Decoder(*mut OpusDecoder);

unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Result<Decoder> {
        let mut err = 0;
        let st = unsafe { opus_decoder_create(SAMPLE_RATE as i32, CHANNELS as i32, &mut err) };
        if st.is_null() || err != OPUS_OK {
            bail!("couldn't start the Opus decoder (error {err})");
        }
        Ok(Decoder(st))
    }

    /// Decode one packet into `out` ([`FRAME_SAMPLES`] long). `None` = the
    /// packet was lost: Opus makes up a plausible 10 ms instead of a click.
    pub fn decode(&mut self, packet: Option<&[u8]>, out: &mut [f32]) -> Result<()> {
        assert_eq!(out.len(), FRAME_SAMPLES);
        let (ptr, len) = match packet {
            Some(p) => (p.as_ptr(), p.len() as i32),
            None => (std::ptr::null(), 0),
        };
        let n = unsafe { opus_decode_float(self.0, ptr, len, out.as_mut_ptr(), FRAME as i32, 0) };
        if n < 0 {
            bail!("Opus decode failed (error {n})");
        }
        // A sender using longer frames would overflow `out`; Opus refuses
        // instead (buffer too small), so `n` is at most FRAME here.
        out[n as usize * CHANNELS..].fill(0.0);
        Ok(())
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { opus_decoder_destroy(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 440 Hz tone in the left ear, 660 Hz in the right.
    pub fn tone(frame: usize) -> Vec<f32> {
        (0..FRAME)
            .flat_map(|i| {
                let t = (frame * FRAME + i) as f32 / SAMPLE_RATE as f32;
                let l = (t * 440.0 * std::f32::consts::TAU).sin() * 0.5;
                let r = (t * 660.0 * std::f32::consts::TAU).sin() * 0.5;
                [l, r]
            })
            .collect()
    }

    #[test]
    fn sound_survives_the_round_trip() {
        let mut enc = Encoder::new().unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut packet = [0u8; MAX_PACKET];
        let mut out = vec![0.0; FRAME_SAMPLES];
        let mut sizes = Vec::new();
        let mut energy = 0.0;
        for f in 0..50 {
            let n = enc.encode(&tone(f), &mut packet).unwrap();
            sizes.push(n);
            dec.decode(Some(&packet[..n]), &mut out).unwrap();
            if f > 5 {
                energy += out.iter().map(|s| s * s).sum::<f32>();
            }
        }
        let avg = sizes.iter().sum::<usize>() / sizes.len();
        assert!(
            (100..=300).contains(&avg),
            "~160 bytes per 10 ms at 128 kbps, got {avg}"
        );
        // A 0.5-amplitude sine has mean square 0.125.
        let mean_square = energy / (44 * FRAME_SAMPLES) as f32;
        assert!((0.09..0.16).contains(&mean_square), "{mean_square}");
    }

    #[test]
    fn a_lost_packet_is_filled_in() {
        let mut enc = Encoder::new().unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut packet = [0u8; MAX_PACKET];
        let mut out = vec![0.0; FRAME_SAMPLES];
        for f in 0..10 {
            let n = enc.encode(&tone(f), &mut packet).unwrap();
            dec.decode(Some(&packet[..n]), &mut out).unwrap();
        }
        dec.decode(None, &mut out).unwrap();
        assert!(out.iter().any(|s| s.abs() > 0.01), "concealment continues the tone");
    }
}

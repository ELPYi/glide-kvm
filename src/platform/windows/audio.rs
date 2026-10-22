//! Sound devices via WASAPI, Windows' own audio API.
//!
//! - [`Loopback`]: what this PC is playing, captured on its way to the
//!   default speakers/headset (the same mechanism recording apps use).
//!   It can mute the speakers meanwhile: loopback hears the sound before
//!   the volume and mute are applied, so it still gets everything.
//! - [`Speaker`]: plays sound on the default speakers/headset.
//!
//! Both use 48 kHz stereo float and let Windows convert to and from the
//! device's own format. Both follow the default device: when it changes
//! (headphones plugged in), `stale()` says so and the caller reopens.

use std::time::Duration;

use anyhow::{Context, Result};
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVEFORMATEXTENSIBLE_0,
};
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use crate::audio::{AudioIn, AudioOut, CHANNELS, SAMPLE_RATE};

/// 100 ns units.
const fn hns(ms: i64) -> i64 {
    ms * 10_000
}

/// Loopback buffer: how much Windows holds for us between reads. Doesn't
/// add delay (sound is read as soon as each 10 ms chunk is ready).
const CAPTURE_BUFFER: i64 = hns(100);
/// Playback buffer. Also no added delay: we only keep ~15 ms queued in it.
const RENDER_BUFFER: i64 = hns(50);

fn format() -> WAVEFORMATEXTENSIBLE {
    let block = (CHANNELS * 4) as u16;
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: CHANNELS as u16,
            nSamplesPerSec: SAMPLE_RATE as u32,
            nAvgBytesPerSec: SAMPLE_RATE as u32 * u32::from(block),
            nBlockAlign: block,
            wBitsPerSample: 32,
            cbSize: (size_of::<WAVEFORMATEXTENSIBLE>() - size_of::<WAVEFORMATEX>()) as u16,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 {
            wValidBitsPerSample: 32,
        },
        dwChannelMask: 0x3, // front left | front right
        SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
    }
}

/// The default speakers/headset, and its id (to notice it changing).
fn default_output() -> Result<(IMMDevice, String)> {
    unsafe {
        // Each audio thread joins COM once; repeating is harmless.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let devices: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).context("Windows audio isn't available")?;
        let device = devices
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .context("no speakers or headset found")?;
        let id = device_id(&device)?;
        Ok((device, id))
    }
}

fn device_id(device: &IMMDevice) -> Result<String> {
    unsafe {
        let p: PWSTR = device.GetId()?;
        let id = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as _));
        Ok(id)
    }
}

/// The default device is no longer `id` (or there's none).
fn changed_from(id: &str) -> bool {
    default_output().map_or(true, |(_, now)| now != id)
}

fn init(device: &IMMDevice, flags: u32, buffer: i64) -> Result<IAudioClient> {
    unsafe {
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let fmt = format();
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                flags | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                buffer,
                0,
                &fmt as *const WAVEFORMATEXTENSIBLE as *const WAVEFORMATEX,
                None,
            )
            .context("the sound device refused the stream")?;
        Ok(client)
    }
}

pub struct Loopback {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    /// Signalled when sound is ready; `None` = poll (if Windows refused
    /// event mode for loopback).
    event: Option<HANDLE>,
    device: String,
    /// Set if we muted the speakers (unmuted again when done).
    muted: Option<IAudioEndpointVolume>,
}

impl Loopback {
    /// `mute`: silence this PC's speakers while capturing.
    pub fn open(mute: bool) -> Result<Loopback> {
        let (device, id) = default_output()?;
        // Event mode: the thread sleeps until sound is ready, and right
        // through silence (no wake-ups at all while nothing plays).
        let evented = unsafe { CreateEventW(None, false, false, None) }
            .ok()
            .and_then(|event| {
                let client = init(
                    &device,
                    AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                    CAPTURE_BUFFER,
                )
                .and_then(|c| unsafe { c.SetEventHandle(event) }.map(|_| c).map_err(Into::into));
                match client {
                    Ok(c) => Some((c, event)),
                    Err(_) => {
                        unsafe {
                            let _ = CloseHandle(event);
                        }
                        None
                    }
                }
            });
        let (client, event) = match evented {
            Some((c, e)) => (c, Some(e)),
            None => (init(&device, AUDCLNT_STREAMFLAGS_LOOPBACK, CAPTURE_BUFFER)?, None),
        };
        let mut lb = Loopback {
            capture: unsafe { client.GetService()? },
            client,
            event,
            device: id,
            muted: None,
        };
        unsafe { lb.client.Start()? };
        if mute {
            lb.muted = mute_speakers(&device);
        }
        Ok(lb)
    }
}

/// Mute `device` unless it already is; returns it if we did.
fn mute_speakers(device: &IMMDevice) -> Option<IAudioEndpointVolume> {
    unsafe {
        let vol: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
        if vol.GetMute().ok()?.as_bool() {
            return None; // already muted by the user: leave it to them
        }
        vol.SetMute(true, std::ptr::null()).ok()?;
        Some(vol)
    }
}

impl AudioIn for Loopback {
    fn read(&mut self, out: &mut Vec<f32>) -> Result<()> {
        unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let n = frames as usize * CHANNELS;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    out.resize(out.len() + n, 0.0);
                } else {
                    out.extend_from_slice(std::slice::from_raw_parts(data as *const f32, n));
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
        Ok(())
    }

    fn wait(&mut self, max: Duration) {
        match self.event {
            Some(e) => unsafe {
                WaitForSingleObject(e, max.as_millis() as u32);
            },
            // Sound arrives in 10 ms chunks; checking more often catches each sooner.
            None => super::precise_sleep(Duration::from_millis(2).min(max)),
        }
    }

    fn stale(&mut self) -> bool {
        changed_from(&self.device)
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            if let Some(vol) = self.muted.take() {
                let _ = vol.SetMute(false, std::ptr::null());
            }
            if let Some(e) = self.event.take() {
                let _ = CloseHandle(e);
            }
        }
    }
}

pub struct Speaker {
    client: IAudioClient,
    render: IAudioRenderClient,
    event: HANDLE,
    buffer_frames: usize,
    device: String,
}

impl Speaker {
    pub fn open() -> Result<Speaker> {
        let (device, id) = default_output()?;
        let client = init(&device, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, RENDER_BUFFER)?;
        unsafe {
            let event = CreateEventW(None, false, false, None)?;
            let speaker = (|| -> Result<Speaker> {
                client.SetEventHandle(event)?;
                let render: IAudioRenderClient = client.GetService()?;
                let buffer_frames = client.GetBufferSize()? as usize;
                client.Start()?;
                Ok(Speaker {
                    client: client.clone(),
                    render,
                    event,
                    buffer_frames,
                    device: id,
                })
            })();
            if speaker.is_err() {
                let _ = CloseHandle(event);
            }
            speaker
        }
    }
}

impl AudioOut for Speaker {
    fn wait(&mut self, max: Duration) {
        unsafe {
            let _ = WaitForSingleObject(self.event, max.as_millis() as u32) == WAIT_OBJECT_0;
        }
    }

    fn queued(&mut self) -> Result<usize> {
        Ok(unsafe { self.client.GetCurrentPadding()? } as usize)
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        let frames = (samples.len() / CHANNELS).min(self.buffer_frames.saturating_sub(self.queued()?));
        if frames == 0 {
            return Ok(());
        }
        unsafe {
            let buf = self.render.GetBuffer(frames as u32)?;
            std::ptr::copy_nonoverlapping(samples.as_ptr(), buf as *mut f32, frames * CHANNELS);
            self.render.ReleaseBuffer(frames as u32, 0)?;
        }
        Ok(())
    }

    fn stale(&mut self) -> bool {
        changed_from(&self.device)
    }
}

impl Drop for Speaker {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            let _ = CloseHandle(self.event);
        }
    }
}

pub fn open_loopback(mute: bool) -> Result<Box<dyn AudioIn>> {
    Ok(Box::new(Loopback::open(mute)?))
}

pub fn open_speaker() -> Result<Box<dyn AudioOut>> {
    Ok(Box::new(Speaker::open()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn muted_now() -> bool {
        let (device, _) = default_output().unwrap();
        let vol: IAudioEndpointVolume = unsafe { device.Activate(CLSCTX_ALL, None).unwrap() };
        unsafe { vol.GetMute().unwrap().as_bool() }
    }

    /// Real devices: plays an inaudibly quiet tone (-80 dB) with the
    /// speakers muted and checks the loopback still hears it, and that the
    /// speakers are unmuted afterwards. `cargo test -- --ignored loopback`
    #[test]
    #[ignore = "uses the real sound device (and mutes it for a moment)"]
    fn loopback_hears_what_this_pc_plays_even_muted() {
        let was_muted = muted_now();
        let mut speaker = Speaker::open().unwrap();
        let mut loopback = Loopback::open(true).unwrap();
        assert!(loopback.event.is_some(), "event mode (sleeps through silence)");
        assert!(muted_now(), "speakers muted while capturing");
        let amp = 1e-4f32;
        let mut phase = 0usize;
        let mut heard: Vec<f32> = Vec::new();
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(600) {
            speaker.wait(Duration::from_millis(20));
            let free = crate::audio::frames_in_ms(30).saturating_sub(speaker.queued().unwrap());
            let tone: Vec<f32> = (0..free)
                .flat_map(|i| {
                    let s = ((phase + i) as f32 * 0.06).sin() * amp;
                    [s, s]
                })
                .collect();
            phase += free;
            speaker.write(&tone).unwrap();
            loopback.wait(Duration::ZERO);
            loopback.read(&mut heard).unwrap();
        }
        let peak = heard.iter().fold(0f32, |m, s| m.max(s.abs()));
        eprintln!("captured {} frames, peak {peak:e}", heard.len() / 2);
        assert!(
            heard.len() / 2 > crate::audio::frames_in_ms(300),
            "loopback delivers sound"
        );
        assert!(peak > amp * 0.3, "the quiet tone was captured (peak {peak:e})");
        drop(loopback);
        assert_eq!(muted_now(), was_muted, "speakers back as they were");
    }
}

#[cfg(test)]
mod event_probe {
    use super::*;
    use std::time::Instant;

    /// Event mode wakes as each ~10 ms chunk is ready (not late, not in bursts).
    #[test]
    #[ignore = "uses the real sound device"]
    fn loopback_wakes_for_each_chunk() {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = stop.clone();
        let player = std::thread::spawn(move || {
            let mut speaker = Speaker::open().unwrap();
            let mut phase = 0usize;
            while !s.load(std::sync::atomic::Ordering::Relaxed) {
                speaker.wait(Duration::from_millis(20));
                let free = crate::audio::frames_in_ms(30).saturating_sub(speaker.queued().unwrap());
                let tone: Vec<f32> = (0..free)
                    .flat_map(|i| [((phase + i) as f32 * 0.06).sin() * 1e-4; 2])
                    .collect();
                phase += free;
                speaker.write(&tone).unwrap();
            }
        });
        let mut loopback = Loopback::open(false).unwrap();
        let mut buf = Vec::new();
        let start = Instant::now();
        let mut last = Instant::now();
        let mut gaps = Vec::new();
        while start.elapsed() < Duration::from_millis(800) {
            loopback.wait(Duration::from_millis(100));
            let before = buf.len();
            loopback.read(&mut buf).unwrap();
            if buf.len() > before {
                gaps.push(last.elapsed().as_secs_f32() * 1000.0);
                last = Instant::now();
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        player.join().unwrap();
        gaps.sort_by(f32::total_cmp);
        let median = gaps[gaps.len() / 2];
        let worst = gaps[gaps.len() * 95 / 100];
        eprintln!(
            "{} wakes with sound; median {median:.1} ms, p95 {worst:.1} ms",
            gaps.len()
        );
        assert!(gaps.len() > 50, "woke for most chunks");
        assert!(median < 12.0 && worst < 25.0);
    }
}

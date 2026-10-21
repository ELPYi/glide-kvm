//! Pointer speed and acceleration ("Enhance pointer precision").
//!
//! Injected relative mouse moves go through the *receiving* PC's pointer
//! ballistics. If that PC has acceleration on and the sharing PC doesn't, the
//! cursor feels floaty or "smoothed". So while controlled, a client
//! temporarily adopts the sharing PC's settings, and restores its own after.
//! Changes are applied without saving (fWinIni = 0), so even a crash can't
//! make them permanent: signing out restores the user's settings.

use std::ffi::c_void;
use std::sync::Mutex;

use windows_sys::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, SPI_GETMOUSE, SPI_GETMOUSESPEED, SPI_SETMOUSE, SPI_SETMOUSESPEED,
};

use crate::input::PointerFeel;

/// Windows' own defaults when "Enhance pointer precision" is on.
const ACCEL_ON: [i32; 3] = [6, 10, 1];
const ACCEL_OFF: [i32; 3] = [0, 0, 0];

/// Exact settings to put back, captured before the first change.
#[derive(Clone, Copy)]
struct Raw {
    speed: u32,
    mouse: [i32; 3],
}

static ORIGINAL: Mutex<Option<Raw>> = Mutex::new(None);

fn read_raw() -> Option<Raw> {
    let mut speed: u32 = 0;
    let mut mouse = [0i32; 3];
    let ok = unsafe {
        SystemParametersInfoW(SPI_GETMOUSESPEED, 0, &mut speed as *mut u32 as *mut c_void, 0) != 0
            && SystemParametersInfoW(SPI_GETMOUSE, 0, mouse.as_mut_ptr() as *mut c_void, 0) != 0
    };
    ok.then_some(Raw { speed, mouse })
}

fn write_raw(r: Raw) {
    unsafe {
        // SPI_SETMOUSESPEED takes the value itself in the pointer argument.
        SystemParametersInfoW(SPI_SETMOUSESPEED, 0, r.speed as usize as *mut c_void, 0);
        let mut mouse = r.mouse;
        SystemParametersInfoW(SPI_SETMOUSE, 0, mouse.as_mut_ptr() as *mut c_void, 0);
    }
}

pub fn current() -> Option<PointerFeel> {
    read_raw().map(|r| PointerFeel {
        speed: r.speed.clamp(1, 20) as u8,
        acceleration: r.mouse[2] != 0,
    })
}

/// `Some`: adopt `feel` (remembering the original once). `None`: restore.
pub fn set(feel: Option<PointerFeel>) {
    let mut original = ORIGINAL.lock().unwrap_or_else(|e| e.into_inner());
    match feel {
        Some(f) => {
            if original.is_none() {
                *original = read_raw();
            }
            write_raw(Raw {
                speed: f.speed.clamp(1, 20) as u32,
                mouse: if f.acceleration { ACCEL_ON } else { ACCEL_OFF },
            });
        }
        None => {
            if let Some(r) = original.take() {
                write_raw(r);
            }
        }
    }
}

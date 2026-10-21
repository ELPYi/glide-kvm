//! Sub-millisecond sleeps. `std::thread::sleep` on Windows rounds up to the
//! system timer tick (often 15.6 ms), far too coarse for pacing mouse
//! packets. A high-resolution waitable timer (Windows 10 1803+) wakes within
//! ~0.5 ms without changing the global timer resolution.

use std::ptr::null;
use std::time::Duration;

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Threading::{CreateWaitableTimerExW, SetWaitableTimer, WaitForSingleObject, INFINITE};

const CREATE_WAITABLE_TIMER_HIGH_RESOLUTION: u32 = 0x2;
const TIMER_ALL_ACCESS: u32 = 0x1F0003;

/// Raw handle wrapper so it can live in a thread-local.
struct Timer(HANDLE);

thread_local! {
    static TIMER: Option<Timer> = unsafe {
        let h = CreateWaitableTimerExW(null(), null(), CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS);
        (!h.is_null()).then_some(Timer(h))
    };
}

pub fn precise_sleep(d: Duration) {
    if d.is_zero() {
        return;
    }
    let done = TIMER.with(|t| match t {
        Some(Timer(h)) => unsafe {
            // Negative = relative, in 100 ns units.
            let due = -((d.as_nanos() / 100).max(1) as i64);
            SetWaitableTimer(*h, &due, 0, None, null(), 0) != 0 && WaitForSingleObject(*h, INFINITE) == 0
        },
        None => false,
    });
    if !done {
        std::thread::sleep(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn sleeps_about_a_millisecond() {
        let t0 = Instant::now();
        for _ in 0..10 {
            precise_sleep(Duration::from_millis(1));
        }
        let per = t0.elapsed() / 10;
        assert!(per >= Duration::from_micros(900), "{per:?}");
        assert!(per < Duration::from_millis(4), "too coarse: {per:?}");
    }
}

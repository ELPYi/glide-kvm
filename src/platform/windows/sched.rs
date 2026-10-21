//! CPU scheduling for latency-critical threads.
//!
//! On a busy PC (a game, a build) ordinary threads queue for a CPU core, and
//! a mouse packet can wait tens of milliseconds to be handled. Registering a
//! thread with MMCSS (the Multimedia Class Scheduler, used by pro-audio
//! apps) lifts it into a priority band above normal work, so it runs as soon
//! as its packet or input event arrives. MMCSS still reserves a share of CPU
//! for everything else, so a runaway thread can't freeze the PC.

use windows_sys::Win32::System::Threading::{
    AvSetMmThreadCharacteristicsW, AvSetMmThreadPriority, GetCurrentProcess, GetCurrentThread, SetPriorityClass,
    SetThreadPriority, AVRT_PRIORITY_CRITICAL, HIGH_PRIORITY_CLASS, THREAD_PRIORITY_TIME_CRITICAL,
};

use super::wide;

/// Call at the top of a thread that handles input or mouse packets.
pub fn boost_latency_thread() {
    unsafe {
        // Highest normal priority first, as a fallback if MMCSS is unavailable.
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        let task = wide("Pro Audio");
        let mut index = 0u32;
        let handle = AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index);
        if !handle.is_null() {
            AvSetMmThreadPriority(handle, AVRT_PRIORITY_CRITICAL);
        }
        // The handle is intentionally never reverted: the registration ends
        // with the thread.
    }
}

/// Raise the whole process above normal applications.
pub fn raise_process_priority() {
    unsafe {
        SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
    }
}

//! Minimal logging: every line goes to stderr and into a bounded in-memory
//! buffer that the app's Logs view reads.
//!
//! Use the [`log!`](crate::log) macro rather than calling [`push`] directly.

use std::collections::VecDeque;
use std::sync::Mutex;

const CAPACITY: usize = 500;

static BUFFER: Mutex<VecDeque<LogLine>> = Mutex::new(VecDeque::new());

#[derive(Clone, Debug)]
pub struct LogLine {
    /// Local wall-clock time, "HH:MM:SS".
    pub time: String,
    pub text: String,
}

/// Log a formatted line, like `println!`.
#[macro_export]
macro_rules! log {
    ($($t:tt)*) => {
        $crate::logging::push(format!($($t)*))
    };
}

pub fn push(text: String) {
    let (h, m, s) = crate::platform::local_time();
    let line = LogLine {
        time: format!("{h:02}:{m:02}:{s:02}"),
        text,
    };
    eprintln!("[{}] {}", line.time, line.text);
    let mut buf = BUFFER.lock().unwrap_or_else(|e| e.into_inner());
    if buf.len() == CAPACITY {
        buf.pop_front();
    }
    buf.push_back(line);
}

/// Oldest first.
pub fn recent() -> Vec<LogLine> {
    BUFFER
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect()
}

pub fn clear() {
    BUFFER.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

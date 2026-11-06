//! Controlling the sign-in screen: the lock screen, UAC prompts and the
//! other screens Windows shows on its protected "Winlogon" desktop.
//!
//! No ordinary program can send input there, even one running as
//! administrator. A program running as SYSTEM, on that desktop, can. So:
//!
//! - **The service** (`glide-secure.exe --service`, [`service`]) runs as
//!   SYSTEM and keeps one helper running in the session at the screen. It
//!   has no network code.
//! - **The helper** (`glide-secure.exe --helper`, [`helper`]) runs as SYSTEM
//!   in that session, follows whichever desktop has the input, and injects
//!   what glide hands it over a local pipe.
//! - **glide** (as the controlled PC) keeps doing all the networking. While
//!   the protected desktop is up it sends input to the helper ([`link`])
//!   instead of injecting it itself, so the PC you're at keeps control.
//!
//! Only programs running as administrator can open the pipe (anything else
//! could otherwise click "Yes" on a UAC prompt for itself), so this needs
//! glide's admin mode. glide checks the pipe really is the SYSTEM helper's
//! before sending anything (your PIN goes through it).
//!
//! The service runs from its own copy in `C:\Program Files\glide`, which
//! only administrators can change ([`install`]).

pub mod helper;
pub mod install;
pub mod link;
pub mod service;

use std::io::Write as _;
use std::path::PathBuf;

use crate::input::InputEvent;
use crate::net::protocol::{decode_input, encode_input};

/// The Windows service's name.
pub const SERVICE_NAME: &str = "glide-signin";
const SERVICE_DISPLAY_NAME: &str = "glide sign-in screen";
const SERVICE_DESCRIPTION: &str =
    "Lets the PC sharing its keyboard and mouse with glide type on this PC's lock screen and UAC prompts.";
/// Changes whenever requests or replies change shape.
const PROTOCOL: u8 = 1;

/// The helper's pipe for a session. Pipe names are machine-wide, so each
/// session's helper has its own.
fn pipe_name(session: u32) -> String {
    format!(r"\\.\pipe\glide-signin-{session}")
}

/// What glide asks of the helper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Request {
    /// Just report (cursor position, which desktop).
    Status,
    Inject(InputEvent),
    /// Put the cursor at this point (physical pixels).
    Warp {
        x: i32,
        y: i32,
    },
}

impl Request {
    fn encode(&self) -> Vec<u8> {
        match self {
            Request::Status => vec![PROTOCOL, 0],
            Request::Inject(ev) => {
                let mut out = vec![PROTOCOL, 1];
                out.extend(encode_input(ev));
                out
            }
            Request::Warp { x, y } => {
                let mut out = vec![PROTOCOL, 2];
                out.extend(x.to_le_bytes());
                out.extend(y.to_le_bytes());
                out
            }
        }
    }

    fn decode(buf: &[u8]) -> Option<Request> {
        let (&[version, kind], rest) = buf.split_first_chunk::<2>()?;
        if version != PROTOCOL {
            return None;
        }
        Some(match kind {
            0 => Request::Status,
            1 => Request::Inject(decode_input(rest)?),
            2 => {
                let (x, rest) = rest.split_first_chunk::<4>()?;
                let (y, _) = rest.split_first_chunk::<4>()?;
                Request::Warp {
                    x: i32::from_le_bytes(*x),
                    y: i32::from_le_bytes(*y),
                }
            }
            _ => return None,
        })
    }
}

/// The helper's answer to every request.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reply {
    /// The normal desktop has the input (not the sign-in screen).
    pub on_default_desktop: bool,
    pub cursor: Option<(i32, i32)>,
}

const REPLY_LEN: usize = 12;

impl Reply {
    fn encode(&self) -> [u8; REPLY_LEN] {
        let mut out = [0u8; REPLY_LEN];
        out[0] = PROTOCOL;
        out[1] = self.on_default_desktop as u8;
        out[2] = self.cursor.is_some() as u8;
        let (x, y) = self.cursor.unwrap_or_default();
        out[4..8].copy_from_slice(&x.to_le_bytes());
        out[8..12].copy_from_slice(&y.to_le_bytes());
        out
    }

    fn decode(buf: &[u8]) -> Option<Reply> {
        let buf: &[u8; REPLY_LEN] = buf.try_into().ok()?;
        if buf[0] != PROTOCOL {
            return None;
        }
        let x = i32::from_le_bytes(buf[4..8].try_into().ok()?);
        let y = i32::from_le_bytes(buf[8..12].try_into().ok()?);
        Some(Reply {
            on_default_desktop: buf[1] != 0,
            cursor: (buf[2] != 0).then_some((x, y)),
        })
    }
}

/// `glide-secure.exe` entry point.
pub fn main(args: &[String]) -> i32 {
    let result = match args.first().map(String::as_str) {
        Some("--service") => service::run(),
        Some("--helper") => helper::run(),
        _ => {
            eprintln!("glide-secure is started by Windows as the \"{SERVICE_DISPLAY_NAME}\" service.");
            return 2;
        }
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            log(&format!("{e:#}"));
            1
        }
    }
}

/// The service's and helper's log, next to the service's program file (they
/// have no window). Kept short: started afresh past 256 KB.
fn log(line: &str) {
    let Some(path) = std::env::current_exe()
        .ok()
        .map(|p| p.with_file_name("glide-secure.log"))
    else {
        return;
    };
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let (h, m, s) = super::system::local_time();
        let _ = writeln!(f, "{h:02}:{m:02}:{s:02} [{}] {line}", std::process::id());
    }
}

/// Where the service's copy lives: `C:\Program Files\glide`.
fn install_dir() -> Option<PathBuf> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath};
    unsafe {
        let mut path = std::ptr::null_mut();
        if SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, std::ptr::null_mut(), &mut path) != 0 {
            return None;
        }
        let len = (0..).take_while(|&i| *path.add(i) != 0).count();
        let dir = String::from_utf16_lossy(std::slice::from_raw_parts(path, len));
        CoTaskMemFree(path.cast());
        Some(PathBuf::from(dir).join("glide"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Button;

    #[test]
    fn requests_round_trip() {
        for r in [
            Request::Status,
            Request::Inject(InputEvent::Key { code: 30, down: true }),
            Request::Inject(InputEvent::Motion { dx: -4, dy: 9 }),
            Request::Inject(InputEvent::Button {
                button: Button::Left,
                down: false,
            }),
            Request::Warp { x: -1920, y: 1079 },
        ] {
            assert_eq!(Request::decode(&r.encode()), Some(r));
        }
        assert_eq!(Request::decode(&[PROTOCOL + 1, 0]), None, "other versions refused");
        assert_eq!(Request::decode(&[PROTOCOL]), None);
        assert_eq!(Request::decode(&[PROTOCOL, 9]), None);
    }

    #[test]
    fn replies_round_trip() {
        for r in [
            Reply {
                on_default_desktop: false,
                cursor: Some((-5, 700)),
            },
            Reply {
                on_default_desktop: true,
                cursor: None,
            },
        ] {
            assert_eq!(Reply::decode(&r.encode()), Some(r));
        }
        assert_eq!(Reply::decode(&[PROTOCOL; 3]), None);
    }

    #[test]
    fn program_files_is_found() {
        let dir = install_dir().expect("Program Files");
        assert!(dir.ends_with("glide"), "{}", dir.display());
        assert!(dir.parent().unwrap().exists());
    }
}

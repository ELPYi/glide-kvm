//! glide's end of the pipe to the sign-in screen helper.

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_IO_PENDING, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::{
    GetTokenInformation, IsWellKnownSid, TokenUser, WinLocalSystemSid, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::Pipes::{
    GetNamedPipeServerProcessId, SetNamedPipeHandleState, TransactNamedPipe, PIPE_READMODE_MESSAGE,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenProcess, OpenProcessToken, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

use super::{pipe_name, Reply, Request, REPLY_LEN};
use crate::log;

/// A request the helper hasn't answered in this long means it's stuck:
/// give up on it rather than hold up glide.
const ANSWER_WITHIN: u32 = 250; // ms
/// After failing to reach the helper, wait this long before trying again
/// (it's checked every 100 ms while the sign-in screen is up).
const RETRY_AFTER: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Link {
    pipe: Option<Pipe>,
    retry_at: Option<Instant>,
    /// Logged once per change, not on every attempt.
    reachable: Option<bool>,
}

impl Link {
    /// Send `request`; `None` if the helper can't be reached.
    pub fn request(&mut self, request: Request) -> Option<Reply> {
        if self.pipe.is_none() {
            if self.retry_at.is_some_and(|t| Instant::now() < t) {
                return None;
            }
            match Pipe::connect() {
                Ok(p) => self.pipe = Some(p),
                Err(why) => {
                    self.retry_at = Some(Instant::now() + RETRY_AFTER);
                    self.note(false, &why);
                    return None;
                }
            }
        }
        let reply = self.pipe.as_ref().and_then(|p| p.transact(&request));
        match reply {
            Some(_) => self.note(true, ""),
            None => {
                self.pipe = None;
                self.retry_at = Some(Instant::now() + RETRY_AFTER);
                self.note(false, "it stopped answering");
            }
        }
        reply
    }

    fn note(&mut self, reachable: bool, why: &str) {
        if self.reachable != Some(reachable) {
            self.reachable = Some(reachable);
            if reachable {
                log!("sign-in screen: connected to glide's helper");
            } else {
                log!("sign-in screen: can't reach glide's helper ({why})");
            }
        }
    }
}

struct Pipe {
    handle: HANDLE,
    event: HANDLE,
}

// The handles are only used under the WinDesktop's lock.
unsafe impl Send for Pipe {}

impl Pipe {
    fn connect() -> Result<Pipe, String> {
        let mut session = 0u32;
        unsafe { ProcessIdToSessionId(std::process::id(), &mut session) };
        let name = super::super::wide(&pipe_name(session));
        unsafe {
            // Identification only: whoever is on the other end can't act as us.
            let handle = CreateFileW(
                name.as_ptr(),
                0x8000_0000 | 0x4000_0000, // GENERIC_READ | GENERIC_WRITE
                0,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                return Err(match GetLastError() {
                    2 => "it isn't running; is \"Control the sign-in screen\" on?".into(),
                    5 => "glide isn't running as administrator".into(),
                    e => format!("error {e}"),
                });
            }
            let pipe = Pipe {
                handle,
                event: CreateEventW(null(), 1, 0, null()),
            };
            // Your PIN goes through this pipe: make sure it's really the
            // helper (running as SYSTEM), not something that took its name.
            let mut server = 0u32;
            if GetNamedPipeServerProcessId(handle, &mut server) == 0 || !runs_as_system(server) {
                return Err("the pipe isn't glide's helper's; not using it".into());
            }
            let mode = PIPE_READMODE_MESSAGE;
            if pipe.event.is_null() || SetNamedPipeHandleState(handle, &mode, null(), null()) == 0 {
                return Err(format!("error {}", GetLastError()));
            }
            Ok(pipe)
        }
    }

    /// Send one request and wait (briefly) for the reply.
    fn transact(&self, request: &Request) -> Option<Reply> {
        let out = request.encode();
        let mut reply = [0u8; REPLY_LEN + 4];
        unsafe {
            let mut ov: OVERLAPPED = zeroed();
            ov.hEvent = self.event;
            let mut n = 0u32;
            let ok = TransactNamedPipe(
                self.handle,
                out.as_ptr().cast(),
                out.len() as u32,
                reply.as_mut_ptr().cast(),
                reply.len() as u32,
                &mut n,
                &mut ov,
            );
            if ok == 0 {
                if GetLastError() != ERROR_IO_PENDING {
                    return None;
                }
                if WaitForSingleObject(self.event, ANSWER_WITHIN) != 0 {
                    CancelIoEx(self.handle, &ov);
                    GetOverlappedResult(self.handle, &ov, &mut n, 1);
                    return None;
                }
                if GetOverlappedResult(self.handle, &ov, &mut n, 0) == 0 {
                    return None;
                }
            }
            Reply::decode(&reply[..n as usize])
        }
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
            if !self.event.is_null() {
                CloseHandle(self.event);
            }
        }
    }
}

/// Is process `pid` running as SYSTEM (LocalSystem)?
fn runs_as_system(pid: u32) -> bool {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return false;
        }
        let mut token: HANDLE = null_mut();
        let ok = OpenProcessToken(process, TOKEN_QUERY, &mut token) != 0;
        CloseHandle(process);
        if !ok {
            return false;
        }
        // TOKEN_USER plus the SID it points to.
        let mut buf = [0u64; 16];
        let mut len = 0u32;
        let got = GetTokenInformation(
            token,
            TokenUser,
            buf.as_mut_ptr().cast(),
            size_of::<[u64; 16]>() as u32,
            &mut len,
        ) != 0;
        CloseHandle(token);
        got && IsWellKnownSid((*(buf.as_ptr() as *const TOKEN_USER)).User.Sid, WinLocalSystemSid) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_system_processes() {
        assert!(!runs_as_system(std::process::id()), "tests don't run as SYSTEM");
        assert!(!runs_as_system(0));
    }

    #[test]
    fn no_helper_means_no_reply_and_no_hang() {
        // No helper runs for tests (and a real one wouldn't let a normal
        // test process in): requests fail fast and retries are spaced out.
        let mut link = Link::default();
        let t = Instant::now();
        if link.request(Request::Status).is_none() {
            assert!(link.request(Request::Status).is_none());
            assert!(t.elapsed() < Duration::from_millis(500));
            assert!(link.retry_at.is_some());
        }
    }
}

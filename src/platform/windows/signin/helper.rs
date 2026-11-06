//! The helper: runs as SYSTEM in the session at the screen (started by the
//! service), serves glide's requests over a pipe, and injects them into
//! whichever desktop has the input, the sign-in screen included.

use std::ptr::null_mut;
use std::time::{Duration, Instant};

use anyhow::{bail, Result};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows_sys::Win32::Storage::FileSystem::{
    FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_MESSAGE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_MESSAGE, PIPE_WAIT,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, SetThreadDesktop, HDESK, UOI_NAME,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;

use super::{log, pipe_name, Reply, Request};

/// Who may open the pipe: SYSTEM, and administrators, but only from a
/// program running as administrator (high integrity, "no write up"): a
/// normal program could otherwise answer UAC prompts by itself.
const PIPE_SECURITY: &str = "D:P(A;;GA;;;SY)(A;;GRGW;;;BA)S:(ML;;NWNR;;;HI)";
/// How often to look for the input moving to another desktop.
const DESKTOP_CHECK: Duration = Duration::from_millis(50);

pub fn run() -> Result<()> {
    super::super::init_dpi(); // physical pixels, like glide
    let mut session = 0u32;
    unsafe { ProcessIdToSessionId(std::process::id(), &mut session) };
    log(&format!("helper started in session {session}"));
    let mut desktop = InputDesktop::default();
    loop {
        let pipe = match create_pipe(session) {
            Ok(p) => p,
            Err(e) => {
                // Taken (another helper still closing, or something
                // pretending to be us): try again shortly.
                log(&format!("{e:#}"));
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        serve(pipe, &mut desktop);
        unsafe { CloseHandle(pipe) };
    }
}

fn create_pipe(session: u32) -> Result<HANDLE> {
    let sddl = super::super::wide(PIPE_SECURITY);
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut sd, null_mut()) == 0 {
            bail!("couldn't build the pipe's permissions ({})", GetLastError());
        }
        let attrs = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: sd,
            bInheritHandle: 0,
        };
        let name = super::super::wide(&pipe_name(session));
        let pipe = CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            64,
            64,
            0,
            &attrs,
        );
        let err = GetLastError();
        LocalFree(sd);
        if pipe == INVALID_HANDLE_VALUE {
            bail!("couldn't open the pipe ({err})");
        }
        Ok(pipe)
    }
}

/// One glide connection, until it goes away.
fn serve(pipe: HANDLE, desktop: &mut InputDesktop) {
    unsafe {
        // Returns false with ERROR_PIPE_CONNECTED if glide got in first.
        if ConnectNamedPipe(pipe, null_mut()) == 0 && GetLastError() != 535 {
            return;
        }
    }
    let mut buf = [0u8; 64];
    loop {
        let mut n = 0u32;
        let ok = unsafe { ReadFile(pipe, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n, null_mut()) };
        if ok == 0 {
            break; // glide closed it, or sent something too big
        }
        desktop.follow();
        if let Some(request) = Request::decode(&buf[..n as usize]) {
            match request {
                Request::Status => {}
                Request::Inject(ev) => super::super::desktop::inject_event(&ev),
                Request::Warp { x, y } => unsafe {
                    SetCursorPos(x, y);
                },
            }
        }
        let reply = Reply {
            on_default_desktop: desktop.is_default,
            cursor: super::super::desktop::cursor_pos().map(|p| (p.x, p.y)),
        }
        .encode();
        let mut written = 0u32;
        let ok = unsafe { WriteFile(pipe, reply.as_ptr(), reply.len() as u32, &mut written, null_mut()) };
        if ok == 0 {
            break;
        }
    }
    unsafe {
        FlushFileBuffers(pipe);
        DisconnectNamedPipe(pipe);
    }
}

/// Keeps this thread on the desktop that has the input, since input can
/// only be sent to (and the cursor read on) the thread's own desktop.
struct InputDesktop {
    handle: HDESK,
    is_default: bool,
    checked: Option<Instant>,
}

impl Default for InputDesktop {
    fn default() -> Self {
        InputDesktop {
            handle: null_mut(),
            is_default: false,
            checked: None,
        }
    }
}

impl InputDesktop {
    fn follow(&mut self) {
        if self.checked.is_some_and(|t| t.elapsed() < DESKTOP_CHECK) {
            return;
        }
        self.checked = Some(Instant::now());
        const GENERIC_ALL: u32 = 0x1000_0000;
        unsafe {
            let input = OpenInputDesktop(0, 0, GENERIC_ALL);
            if input.is_null() {
                return;
            }
            let name = desktop_name(input);
            if !self.handle.is_null() && name == desktop_name(self.handle) {
                CloseDesktop(input);
                return;
            }
            if SetThreadDesktop(input) == 0 {
                log(&format!("couldn't switch to the {name} desktop ({})", GetLastError()));
                CloseDesktop(input);
                return;
            }
            if !self.handle.is_null() {
                CloseDesktop(self.handle);
            }
            self.handle = input;
            self.is_default = name.eq_ignore_ascii_case("Default");
        }
    }
}

fn desktop_name(desk: HDESK) -> String {
    let mut name = [0u16; 64];
    let mut needed = 0u32;
    unsafe {
        GetUserObjectInformationW(
            desk as HANDLE,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            (name.len() * 2) as u32,
            &mut needed,
        );
    }
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf16_lossy(&name[..len])
}

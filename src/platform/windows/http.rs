//! Minimal HTTPS GET using Windows' built-in WinHTTP (system TLS and proxy
//! settings, no extra libraries). Follows redirects, which GitHub release
//! downloads use.

use std::mem::size_of;
use std::ptr::{null, null_mut};

use anyhow::{bail, Result};
use windows_sys::Win32::Networking::WinHttp::*;

use super::wide;

/// Closes a WinHTTP handle when dropped.
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn check(h: *mut core::ffi::c_void, what: &str) -> Result<Handle> {
    if h.is_null() {
        bail!("{what} failed (error {})", std::io::Error::last_os_error());
    }
    Ok(Handle(h))
}

/// GET `url` (https only) and return the body. Fails on non-200 responses.
pub fn http_get(url: &str) -> Result<Vec<u8>> {
    let Some(rest) = url.strip_prefix("https://") else {
        bail!("only https URLs are allowed")
    };
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let agent = wide(&format!("glide/{}", crate::VERSION));
    unsafe {
        let session = check(
            WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0),
            "WinHttpOpen",
        )?;
        // 15 s to resolve/connect/send, 60 s between received chunks.
        WinHttpSetTimeouts(session.0, 15_000, 15_000, 15_000, 60_000);
        let host_w = wide(host);
        let connect = check(WinHttpConnect(session.0, host_w.as_ptr(), 443, 0), "connecting")?;
        let (verb, path_w) = (wide("GET"), wide(path));
        let request = check(
            WinHttpOpenRequest(
                connect.0,
                verb.as_ptr(),
                path_w.as_ptr(),
                null(),
                null(),
                null(),
                WINHTTP_FLAG_SECURE,
            ),
            "opening the request",
        )?;
        if WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0
            || WinHttpReceiveResponse(request.0, null_mut()) == 0
        {
            bail!("couldn't reach {host} ({})", std::io::Error::last_os_error());
        }
        let mut status: u32 = 0;
        let mut len = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            null(),
            &mut status as *mut u32 as *mut _,
            &mut len,
            null_mut(),
        );
        if status != 200 {
            bail!("{host} answered HTTP {status}");
        }
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                bail!("download interrupted ({})", std::io::Error::last_os_error());
            }
            if available == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + available as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(request.0, body[start..].as_mut_ptr().cast(), available, &mut read) == 0 {
                bail!("download interrupted ({})", std::io::Error::last_os_error());
            }
            body.truncate(start + read as usize);
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_plain_http() {
        assert!(http_get("http://example.com/").is_err());
    }

    /// Needs internet; run with --ignored.
    #[test]
    #[ignore]
    fn fetches_over_https_with_redirects() {
        let body = http_get("https://github.com/rust-lang/rust/releases/latest").unwrap();
        assert!(!body.is_empty());
    }
}

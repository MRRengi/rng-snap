//! HTTPS download with progress and a SHA-256 check (WinHTTP, so it uses the
//! system's proxy settings and certificates). Used for the Cut out models.

use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use windows::Win32::Networking::WinHttp::*;
use windows::core::{HSTRING, PCWSTR, w};

struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn split(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let slash = rest.find('/')?;
    Some((&rest[..slash], &rest[slash..]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Downloads `url` to `dest` (via `dest.part`), adding the bytes read to
/// `done`. Fails unless the file's SHA-256 is `sha256`.
pub fn fetch(url: &str, dest: &Path, sha256: &str, done: &AtomicU64, cancel: &AtomicBool) -> Result<(), String> {
    let (host, path) = split(url).ok_or("bad url")?;
    let part = dest.with_extension("part");
    let r = (|| -> Result<(), String> {
        unsafe {
            let session = Handle(WinHttpOpen(w!("Snap"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
            if session.0.is_null() {
                return Err("network unavailable".into());
            }
            let conn = Handle(WinHttpConnect(session.0, &HSTRING::from(host), 443, 0));
            if conn.0.is_null() {
                return Err(format!("cannot reach {host}"));
            }
            let req = Handle(WinHttpOpenRequest(conn.0, w!("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE));
            if req.0.is_null() {
                return Err("cannot open the request".into());
            }
            WinHttpSendRequest(req.0, None, None, 0, 0, 0).map_err(|e| format!("send: {e}"))?;
            WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| format!("no answer: {e}"))?;
            let mut status = 0u32;
            let mut len = 4u32;
            let mut idx = 0u32;
            WinHttpQueryHeaders(req.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut status as *mut u32 as *mut _), &mut len, &mut idx).map_err(|e| format!("status: {e}"))?;
            if status != 200 {
                return Err(format!("server answered {status}"));
            }
            let mut file = std::fs::File::create(&part).map_err(|e| format!("cannot write {}: {e}", part.display()))?;
            let mut hasher = Sha256::new();
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                if cancel.load(Ordering::Acquire) {
                    return Err("cancelled".into());
                }
                let mut n = 0u32;
                WinHttpReadData(req.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut n).map_err(|e| format!("download interrupted: {e}"))?;
                if n == 0 {
                    break;
                }
                let chunk = &buf[..n as usize];
                hasher.update(chunk);
                file.write_all(chunk).map_err(|e| format!("cannot write: {e}"))?;
                done.fetch_add(n as u64, Ordering::AcqRel);
            }
            file.flush().map_err(|e| e.to_string())?;
            drop(file);
            let got = hex(&hasher.finalize());
            if !got.eq_ignore_ascii_case(sha256) {
                return Err(format!("the file looks damaged ({})", &got[..12]));
            }
            std::fs::rename(&part, dest).map_err(|e| format!("cannot finish {}: {e}", dest.display()))?;
            Ok(())
        }
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    r
}

#[cfg(test)]
mod tests {
    #[test]
    fn splits_urls() {
        assert_eq!(super::split("https://github.com/a/b/c.onnx"), Some(("github.com", "/a/b/c.onnx")));
        assert_eq!(super::split("http://x/y"), None);
    }
}

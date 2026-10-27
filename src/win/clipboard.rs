//! Clipboard output. The bitmap goes in immediately as CF_DIBV5; the "PNG"
//! format is promised with delayed rendering and only encoded if an app asks
//! for it, so copying a 4K capture costs one memcpy.

use crate::image::Frame;
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Gdi::{BI_BITFIELDS, BITMAPV5HEADER};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIBV5, CF_UNICODETEXT};
use windows::core::w;

pub fn png_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("PNG")) }
}

fn global_from(bytes: &[&[u8]]) -> Option<HGLOBAL> {
    let len: usize = bytes.iter().map(|b| b.len()).sum();
    unsafe {
        let g = GlobalAlloc(GMEM_MOVEABLE, len).ok()?;
        let p = GlobalLock(g) as *mut u8;
        if p.is_null() {
            return None;
        }
        let mut off = 0;
        for b in bytes {
            std::ptr::copy_nonoverlapping(b.as_ptr(), p.add(off), b.len());
            off += b.len();
        }
        let _ = GlobalUnlock(g);
        Some(g)
    }
}

fn open(owner: HWND) -> bool {
    for _ in 0..20 {
        if unsafe { OpenClipboard(Some(owner)) }.is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    false
}

fn dibv5(frame: &Frame) -> Option<HGLOBAL> {
    let (w, h) = (frame.width(), frame.height());
    let header = BITMAPV5HEADER {
        bV5Size: size_of::<BITMAPV5HEADER>() as u32,
        bV5Width: w as i32,
        bV5Height: h as i32, // bottom-up: the most widely understood layout
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: (w * h * 4) as u32,
        bV5RedMask: 0x00FF_0000,
        bV5GreenMask: 0x0000_FF00,
        bV5BlueMask: 0x0000_00FF,
        bV5AlphaMask: 0xFF00_0000,
        bV5CSType: 0x7352_4742, // LCS_sRGB
        bV5Intent: 4,           // LCS_GM_IMAGES
        ..Default::default()
    };
    let src = frame.opaque_bgra();
    let mut flipped = Vec::with_capacity(src.len());
    for row in src.chunks_exact(w * 4).rev() {
        flipped.extend_from_slice(row);
    }
    let head = unsafe { std::slice::from_raw_parts(&header as *const _ as *const u8, size_of::<BITMAPV5HEADER>()) };
    global_from(&[head, &flipped])
}

/// Puts the bitmap on the clipboard now and promises PNG on demand.
pub fn put_image(owner: HWND, frame: &Frame) -> bool {
    let Some(dib) = dibv5(frame) else { return false };
    if !open(owner) {
        return false;
    }
    unsafe {
        let _ = EmptyClipboard();
        let ok = SetClipboardData(CF_DIBV5.0 as u32, Some(HANDLE(dib.0))).is_ok();
        let _ = SetClipboardData(png_format(), None);
        let _ = CloseClipboard();
        ok
    }
}

/// Answer to WM_RENDERFORMAT: the clipboard is already open by the requester.
pub fn render_png(frame: &Frame) {
    let png = frame.encode_png();
    if let Some(g) = global_from(&[&png]) {
        unsafe {
            let _ = SetClipboardData(png_format(), Some(HANDLE(g.0)));
        }
    }
}

/// Answer to WM_RENDERALLFORMATS (we are exiting while still owning the clipboard).
pub fn render_all(owner: HWND, frame: &Frame) {
    if !open(owner) {
        return;
    }
    unsafe {
        if GetClipboardOwner().ok() == Some(owner) {
            render_png(frame);
        }
        let _ = CloseClipboard();
    }
}

pub fn put_text(owner: HWND, text: &str) -> bool {
    let utf16: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(utf16.as_ptr() as *const u8, utf16.len() * 2) };
    let Some(g) = global_from(&[bytes]) else { return false };
    if !open(owner) {
        return false;
    }
    unsafe {
        let _ = EmptyClipboard();
        let ok = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(g.0))).is_ok();
        let _ = CloseClipboard();
        ok
    }
}

//! Grabs the whole virtual desktop (all monitors) in one BitBlt.

use crate::geom::Rect;
use crate::image::Frame;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub fn virtual_desktop() -> Rect {
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        Rect::from_xywh(x, y, w, h)
    }
}

pub fn grab_desktop() -> Option<Frame> {
    let rect = virtual_desktop();
    let (w, h) = (rect.width(), rect.height());
    if w <= 0 || h <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(dib) = CreateDIBSection(Some(mem), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            return None;
        };
        let old = SelectObject(mem, dib.into());
        let ok = BitBlt(mem, 0, 0, w, h, Some(screen), rect.left, rect.top, SRCCOPY | CAPTUREBLT).is_ok();
        let _ = GdiFlush();
        let len = w as usize * h as usize * 4;
        let pixels = if ok { std::slice::from_raw_parts(bits as *const u8, len).to_vec() } else { Vec::new() };
        SelectObject(mem, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        ok.then_some(Frame { rect, pixels })
    }
}

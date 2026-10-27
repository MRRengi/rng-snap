use crate::geom::{Point, Rect};
use windows::Win32::Foundation::{POINT, RECT};
use windows::core::{HSTRING, PCWSTR};

pub fn to_rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

pub fn to_win_rect(r: Rect) -> RECT {
    RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

pub fn to_point(p: POINT) -> Point {
    Point::new(p.x, p.y)
}

/// Null-terminated UTF-16 for APIs that take raw buffers.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn h(s: &str) -> HSTRING {
    HSTRING::from(s)
}

pub fn pcwstr(buf: &[u16]) -> PCWSTR {
    PCWSTR(buf.as_ptr())
}

/// Signed x/y from an LPARAM of a mouse message.
pub fn lparam_point(lp: isize) -> Point {
    let x = (lp & 0xFFFF) as u16 as i16 as i32;
    let y = ((lp >> 16) & 0xFFFF) as u16 as i16 as i32;
    Point::new(x, y)
}

pub fn log(msg: &str) {
    if let Some(dir) = super::paths::data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("log.txt")) {
            let _ = writeln!(f, "{msg}");
        }
    }
}

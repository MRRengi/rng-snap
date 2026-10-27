//! Snapshot of the visible top-level windows (z-order, topmost first) plus
//! monitors, used as click-to-capture candidates.

use super::util::to_rect;
use crate::geom::Rect;
use windows::Win32::Foundation::{HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::BOOL;

fn frame_bounds(hwnd: HWND) -> Option<Rect> {
    unsafe {
        let mut r = RECT::default();
        if DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, size_of::<RECT>() as u32).is_ok() {
            return Some(to_rect(r));
        }
        GetWindowRect(hwnd, &mut r).ok()?;
        Some(to_rect(r))
    }
}

fn is_candidate(hwnd: HWND, own_pid: u32) -> bool {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == own_pid {
            return false;
        }
        let mut cloaked = 0u32;
        if DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4).is_ok() && cloaked != 0 {
            return false;
        }
        // The desktop itself spans every monitor; monitors are offered separately instead.
        let mut class = [0u16; 32];
        let n = GetClassNameW(hwnd, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if class == "Progman" || class == "WorkerW" {
            return false;
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        // Click-through layers (e.g. screen-recorder outlines, notification glow) are not real windows.
        if ex & WS_EX_TRANSPARENT.0 != 0 && ex & WS_EX_LAYERED.0 != 0 {
            return false;
        }
        true
    }
}

pub fn snapshot() -> Vec<Rect> {
    struct Ctx {
        own_pid: u32,
        out: Vec<Rect>,
    }
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> BOOL {
        let ctx = unsafe { &mut *(lp.0 as *mut Ctx) };
        if is_candidate(hwnd, ctx.own_pid) {
            if let Some(r) = frame_bounds(hwnd) {
                if r.width() > 1 && r.height() > 1 {
                    ctx.out.push(r);
                }
            }
        }
        BOOL(1)
    }
    unsafe extern "system" fn each_monitor(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        let out = unsafe { &mut *(lp.0 as *mut Vec<Rect>) };
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if unsafe { GetMonitorInfoW(m, &mut info) }.as_bool() {
            out.push(to_rect(info.rcMonitor));
        }
        BOOL(1)
    }
    let mut ctx = Ctx { own_pid: unsafe { GetCurrentProcessId() }, out: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut ctx as *mut _ as isize));
        let mut monitors: Vec<Rect> = Vec::new();
        let _ = EnumDisplayMonitors(None, None, Some(each_monitor), LPARAM(&mut monitors as *mut _ as isize));
        // Whole monitors last: hovering bare desktop offers the full screen.
        ctx.out.extend(monitors);
    }
    ctx.out
}

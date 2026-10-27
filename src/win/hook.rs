//! Low-level keyboard hook. Catches the capture hotkey even when another app
//! registered it and swallows it (so Scroll Lock never toggles). While the
//! overlay is open it also routes the overlay keys there, because a
//! background process is not always allowed to take keyboard focus.
//!
//! The hook runs on its own thread with its own message loop. If it lived on
//! the UI thread, a busy UI (freezing the screen right after the key-down)
//! could miss Windows' hook timeout, the key-up would be skipped, and the
//! next press would be ignored.

use super::{WM_APP_CAPTURE, WM_APP_KEY};
use crate::config::Hotkey;
use std::cell::Cell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::time::Instant;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static TARGET: AtomicIsize = AtomicIsize::new(0);
static OVERLAY: AtomicIsize = AtomicIsize::new(0);
static THREAD: AtomicU32 = AtomicU32::new(0);
static HOTKEY: OnceLock<Hotkey> = OnceLock::new();

/// A held key repeats every few tens of ms; silence this long means the
/// key-up was lost and the next down is a fresh press.
const STALE_MS: u128 = 1500;

thread_local! {
    static DOWN_SINCE: Cell<Option<Instant>> = const { Cell::new(None) };
}

fn down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000 != 0 }
}

fn mods_match(h: &Hotkey) -> bool {
    let ctrl = down(VK_CONTROL);
    let alt = down(VK_MENU);
    let shift = down(VK_SHIFT);
    let win = down(VK_LWIN) || down(VK_RWIN);
    ctrl == h.ctrl && alt == h.alt && shift == h.shift && win == h.win
}

/// Keys the overlay handles itself.
fn overlay_key(vk: u32) -> bool {
    [VK_ESCAPE, VK_RETURN, VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN, VK_C].iter().any(|k| k.0 as u32 == vk)
}

fn post(target: isize, msg: u32, wp: usize, lp: isize) {
    unsafe {
        let _ = PostMessageW(Some(HWND(target as *mut _)), msg, WPARAM(wp), LPARAM(lp));
    }
}

unsafe extern "system" fn proc_(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        let msg = wp.0 as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        let vk = info.vkCode;
        let overlay = OVERLAY.load(Ordering::Acquire);
        let mut swallow = false;
        if let Some(h) = HOTKEY.get() {
            if vk == h.key {
                let held = DOWN_SINCE.with(|d| d.get());
                let fresh = held.is_none_or(|t| t.elapsed().as_millis() > STALE_MS);
                if is_down && (held.is_some() || mods_match(h)) {
                    if fresh {
                        if overlay == 0 {
                            post(TARGET.load(Ordering::Acquire), WM_APP_CAPTURE, 0, 0);
                        }
                    }
                    DOWN_SINCE.with(|d| d.set(Some(Instant::now())));
                    swallow = true;
                } else if is_up && held.is_some() {
                    DOWN_SINCE.with(|d| d.set(None));
                    swallow = true;
                }
            }
        }
        if !swallow && overlay != 0 && overlay_key(vk) {
            if is_down {
                post(overlay, WM_APP_KEY, vk as usize, down(VK_SHIFT) as isize);
            }
            swallow = is_down || is_up;
        }
        if swallow {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

/// Starts the hook thread. Returns false if the hook could not be set.
pub fn install(target: HWND, hotkey: Hotkey) -> bool {
    TARGET.store(target.0 as isize, Ordering::Release);
    let _ = HOTKEY.set(hotkey);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || unsafe {
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc_), None, 0);
        THREAD.store(GetCurrentThreadId(), Ordering::Release);
        let _ = tx.send(hook.is_ok());
        let Ok(hook) = hook else { return };
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
        let _ = UnhookWindowsHookEx(hook);
    });
    rx.recv().unwrap_or(false)
}

pub fn set_overlay(hwnd: Option<HWND>) {
    OVERLAY.store(hwnd.map(|h| h.0 as isize).unwrap_or(0), Ordering::Release);
}

pub fn uninstall() {
    let t = THREAD.load(Ordering::Acquire);
    if t != 0 {
        unsafe {
            let _ = PostThreadMessageW(t, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

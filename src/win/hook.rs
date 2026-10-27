//! Low-level keyboard hook. Catches the capture hotkey even when another app
//! registered it and swallows it (so Scroll Lock never toggles). While the
//! overlay is open it also routes the overlay keys there, because a
//! background process is not always allowed to take keyboard focus.

use super::{WM_APP_CAPTURE, WM_APP_KEY};
use crate::config::Hotkey;
use std::cell::RefCell;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

struct HookState {
    hook: HHOOK,
    target: HWND,
    hotkey: Hotkey,
    overlay: Option<HWND>,
    hotkey_down: bool,
}

thread_local! {
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
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

unsafe extern "system" fn proc_(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        let msg = wp.0 as u32;
        let is_down = msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN;
        let is_up = msg == WM_KEYUP || msg == WM_SYSKEYUP;
        let swallow = STATE.with(|s| {
            let mut s = s.borrow_mut();
            let Some(s) = s.as_mut() else { return false };
            let vk = info.vkCode;
            if vk == s.hotkey.key {
                if is_down && (s.hotkey_down || mods_match(&s.hotkey)) {
                    if !s.hotkey_down {
                        s.hotkey_down = true;
                        if s.overlay.is_none() {
                            unsafe {
                                let _ = PostMessageW(Some(s.target), WM_APP_CAPTURE, WPARAM(0), LPARAM(0));
                            }
                        }
                    }
                    return true;
                }
                if is_up && s.hotkey_down {
                    s.hotkey_down = false;
                    return true;
                }
            }
            if let Some(ov) = s.overlay {
                if overlay_key(vk) {
                    if is_down {
                        let shift = down(VK_SHIFT) as isize;
                        unsafe {
                            let _ = PostMessageW(Some(ov), WM_APP_KEY, WPARAM(vk as usize), LPARAM(shift));
                        }
                    }
                    return is_down || is_up;
                }
            }
            false
        });
        if swallow {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

pub fn install(target: HWND, hotkey: Hotkey) -> bool {
    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc_), None, 0) };
    match hook {
        Ok(hook) => {
            STATE.with(|s| *s.borrow_mut() = Some(HookState { hook, target, hotkey, overlay: None, hotkey_down: false }));
            true
        }
        Err(_) => false,
    }
}

pub fn set_overlay(hwnd: Option<HWND>) {
    STATE.with(|s| {
        if let Some(s) = s.borrow_mut().as_mut() {
            s.overlay = hwnd;
        }
    });
}

pub fn uninstall() {
    STATE.with(|s| {
        if let Some(s) = s.borrow_mut().take() {
            unsafe {
                let _ = UnhookWindowsHookEx(s.hook);
            }
        }
    });
}

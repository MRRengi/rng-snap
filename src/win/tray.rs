use super::WM_APP_TRAY;
use super::util::wide;
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const CMD_CAPTURE: usize = 1;
pub const CMD_OPEN_FOLDER: usize = 2;
pub const CMD_AUTOSTART: usize = 3;
pub const CMD_EXIT: usize = 4;
pub const CMD_EDIT_VIDEO: usize = 5;

fn data(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_APP_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    let tip = wide("Rengi Snap");
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    nid
}

pub fn add(hwnd: HWND, icon: HICON) {
    let mut nid = data(hwnd, icon);
    unsafe {
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
    }
}

pub fn remove(hwnd: HWND) {
    let nid = data(hwnd, HICON::default());
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

/// Shows the tray menu and returns the chosen command (0 = none).
pub fn menu(hwnd: HWND, autostart: bool) -> usize {
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return 0 };
        let items: [(usize, &str, bool); 4] = [
            (CMD_CAPTURE, "Capture", false),
            (CMD_EDIT_VIDEO, "Edit a video...", false),
            (CMD_OPEN_FOLDER, "Open captures folder", false),
            (CMD_AUTOSTART, "Start with Windows", autostart),
        ];
        let labels: Vec<Vec<u16>> = items.iter().map(|i| wide(i.1)).collect();
        for (i, (id, _, checked)) in items.iter().enumerate() {
            let flags = if *checked { MF_STRING | MF_CHECKED } else { MF_STRING };
            let _ = AppendMenuW(m, flags, *id, windows::core::PCWSTR(labels[i].as_ptr()));
        }
        let _ = SetMenuDefaultItem(m, CMD_CAPTURE as u32, 0);
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        let exit = wide("Exit");
        let _ = AppendMenuW(m, MF_STRING, CMD_EXIT, windows::core::PCWSTR(exit.as_ptr()));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenuEx(m, (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN).0, p.x, p.y, hwnd, None);
        let _ = DestroyMenu(m);
        cmd.0 as usize
    }
}

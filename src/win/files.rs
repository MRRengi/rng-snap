//! Saving, opening, folder picking, autostart and the small popup menus.

use super::util::{h, wide};
use crate::config::capture_file_name;
use crate::image::Frame;
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PCWSTR, w};

pub fn save_png(frame: &Frame, folder: &Path, stamp: (u16, u16, u16, u16, u16, u16)) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(folder)?;
    let name = capture_file_name(stamp, |n| folder.join(n).exists());
    let path = folder.join(name);
    std::fs::write(&path, frame.encode_png())?;
    Ok(path)
}

pub fn open_path(path: &Path) {
    let p = h(&path.to_string_lossy());
    unsafe {
        ShellExecuteW(None, w!("open"), &p, None, None, SW_SHOWNORMAL);
    }
}

/// Opens Explorer at the folder, selecting `file` when given.
pub fn open_folder(folder: &Path, file: Option<&Path>) {
    let _ = std::fs::create_dir_all(folder);
    match file {
        Some(f) if f.exists() => {
            let args = h(&format!("/select,\"{}\"", f.display()));
            unsafe {
                ShellExecuteW(None, w!("open"), w!("explorer.exe"), &args, None, SW_SHOWNORMAL);
            }
        }
        _ => open_path(folder),
    }
}

/// Modern folder picker. `None` if cancelled.
pub fn pick_folder(owner: HWND, start: &Path) -> Option<PathBuf> {
    unsafe {
        let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let opts = dlg.GetOptions().ok()?;
        dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
        let _ = dlg.SetTitle(w!("Choose a folder for captures"));
        if start.exists() {
            if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&h(&start.to_string_lossy()), None) {
                let _ = dlg.SetFolder(&item);
            }
        }
        dlg.Show(Some(owner)).ok()?;
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

pub const MENU_CHOOSE: usize = 1000;

/// Folder dropdown: recent folders (current one checked) + "Choose folder...".
/// Returns an index into `folders`, `Some(MENU_CHOOSE)`, or `None`.
pub fn folder_menu(owner: HWND, at: (i32, i32), folders: &[String], current: &str) -> Option<usize> {
    unsafe {
        let m = CreatePopupMenu().ok()?;
        let labels: Vec<Vec<u16>> = folders.iter().map(|f| wide(&short_label(f, 60))).collect();
        for (i, f) in folders.iter().enumerate() {
            let checked = f.eq_ignore_ascii_case(current);
            let flags = if checked { MF_STRING | MF_CHECKED } else { MF_STRING };
            let _ = AppendMenuW(m, flags, i + 1, PCWSTR(labels[i].as_ptr()));
        }
        if !folders.is_empty() {
            let _ = AppendMenuW(m, MF_SEPARATOR, 0, None);
        }
        let choose = wide("Choose folder...");
        let _ = AppendMenuW(m, MF_STRING, MENU_CHOOSE, PCWSTR(choose.as_ptr()));
        let cmd = TrackPopupMenuEx(m, (TPM_RETURNCMD | TPM_LEFTALIGN | TPM_TOPALIGN).0, at.0, at.1, owner, None).0 as usize;
        let _ = DestroyMenu(m);
        match cmd {
            0 => None,
            MENU_CHOOSE => Some(MENU_CHOOSE),
            n => Some(n - 1),
        }
    }
}

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// `...\Projects\Screens` style label that fits `max` chars.
pub fn short_label(path: &str, max: usize) -> String {
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max {
        return path.to_string();
    }
    let tail: String = chars[chars.len() - (max - 3)..].iter().collect();
    match tail.find('\\') {
        Some(i) => format!("...{}", &tail[i..]),
        None => format!("...{tail}"),
    }
}

/// Last path component, for the compact panel label.
pub fn folder_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string_lossy().into_owned())
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = w!("RengiSnap");

pub fn autostart_enabled() -> bool {
    unsafe {
        let mut size = 0u32;
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, Some(&mut size)).is_ok()
    }
}

pub fn set_autostart(on: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_KEY, None, KEY_SET_VALUE, &mut key).is_err() {
            return;
        }
        if on {
            if let Ok(exe) = std::env::current_exe() {
                let cmd = wide(&format!("\"{}\"", exe.display()));
                let bytes = std::slice::from_raw_parts(cmd.as_ptr() as *const u8, cmd.len() * 2);
                let _ = RegSetValueExW(key, RUN_VALUE, None, REG_SZ, Some(bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, RUN_VALUE);
        }
        let _ = RegCloseKey(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(short_label("C:\\a", 60), "C:\\a");
        let long = "C:\\Users\\Maks\\Pictures\\Projects\\Very Long Folder Name\\Screens";
        let l = short_label(long, 30);
        assert!(l.chars().count() <= 30);
        assert!(l.starts_with("...\\"));
        assert!(l.ends_with("Screens"));
    }
}

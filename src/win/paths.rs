use std::path::PathBuf;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Shell::{FOLDERID_Pictures, FOLDERID_RoamingAppData, KNOWN_FOLDER_FLAG, SHGetKnownFolderPath};
use windows::core::GUID;

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KNOWN_FOLDER_FLAG(0), None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// `%APPDATA%\Rengi Snap`, or `RENGI_SNAP_HOME` when set (tests use it).
pub fn data_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("RENGI_SNAP_HOME") {
        return Some(PathBuf::from(dir));
    }
    let base = known_folder(&FOLDERID_RoamingAppData)?;
    let dir = base.join("Snap");
    // Settings from before the rename.
    let old = base.join("Rengi Snap");
    if !dir.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &dir);
    }
    Some(dir)
}

pub fn pictures_dir() -> PathBuf {
    known_folder(&FOLDERID_Pictures)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("Pictures")))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn local_stamp() -> (u16, u16, u16, u16, u16, u16) {
    let t = unsafe { GetLocalTime() };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

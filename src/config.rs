//! Settings stored in `%APPDATA%\Rengi Snap\config.toml`. Any bad or missing
//! value falls back to its default; the app never refuses to start over config.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MAX_RECENT: usize = 8;
/// Virtual-key code of Scroll Lock.
pub const VK_SCROLL: u32 = 0x91;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkey {
    /// Windows virtual-key code. Default: Scroll Lock (0x91 = 145).
    pub key: u32,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl Default for Hotkey {
    fn default() -> Self {
        Self { key: VK_SCROLL, ctrl: false, alt: false, shift: false, win: false }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkey: Hotkey,
    /// Magnifier with coordinates and pixel color while selecting.
    pub loupe: bool,
    /// Also save every capture to `folder` as PNG.
    pub auto_save: bool,
    /// Save folder. Empty = `Pictures\Rengi Snap`.
    pub folder: String,
    /// Most recent first.
    pub recent_folders: Vec<String>,
    /// How long the preview card stays (seconds). 0 = until closed.
    pub card_seconds: u32,
    /// "photo" or "video": the panel mode used last.
    pub mode: String,
    /// 30, 60, 120, or 0 = the monitor's refresh rate.
    pub video_fps: u32,
    /// "original", "1080p" or "720p".
    pub video_res: String,
    pub system_audio: bool,
    pub mic: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: Hotkey::default(),
            loupe: true,
            auto_save: false,
            folder: String::new(),
            recent_folders: Vec::new(),
            card_seconds: 6,
            mode: "photo".into(),
            video_fps: 60,
            video_res: "original".into(),
            system_audio: false,
            mic: false,
        }
    }
}

impl Config {
    /// Parses config text; anything unreadable becomes the default value.
    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        let Ok(t) = text.parse::<toml::Table>() else {
            return c;
        };
        fn get<T: serde::de::DeserializeOwned>(t: &toml::Table, k: &str, into: &mut T) {
            if let Some(v) = t.get(k).cloned().and_then(|v| v.try_into::<T>().ok()) {
                *into = v;
            }
        }
        if let Some(h) = t.get("hotkey").and_then(|v| v.as_table()) {
            get(h, "key", &mut c.hotkey.key);
            get(h, "ctrl", &mut c.hotkey.ctrl);
            get(h, "alt", &mut c.hotkey.alt);
            get(h, "shift", &mut c.hotkey.shift);
            get(h, "win", &mut c.hotkey.win);
        }
        get(&t, "loupe", &mut c.loupe);
        get(&t, "auto_save", &mut c.auto_save);
        get(&t, "folder", &mut c.folder);
        get(&t, "recent_folders", &mut c.recent_folders);
        get(&t, "card_seconds", &mut c.card_seconds);
        get(&t, "mode", &mut c.mode);
        get(&t, "video_fps", &mut c.video_fps);
        get(&t, "video_res", &mut c.video_res);
        get(&t, "system_audio", &mut c.system_audio);
        get(&t, "mic", &mut c.mic);
        c.sanitize();
        c
    }

    pub fn to_text(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    fn sanitize(&mut self) {
        if self.hotkey.key == 0 || self.hotkey.key > 0xFE {
            self.hotkey = Hotkey::default();
        }
        if self.card_seconds > 600 {
            self.card_seconds = 600;
        }
        if self.mode != "video" {
            self.mode = "photo".into();
        }
        if ![0, 30, 60, 120].contains(&self.video_fps) {
            self.video_fps = 60;
        }
        if !["original", "1080p", "720p"].contains(&self.video_res.as_str()) {
            self.video_res = "original".into();
        }
        let mut seen = Vec::new();
        self.recent_folders.retain(|f| {
            let k = f.trim().to_lowercase();
            if k.is_empty() || seen.contains(&k) {
                false
            } else {
                seen.push(k);
                true
            }
        });
        self.recent_folders.truncate(MAX_RECENT);
    }

    /// Makes `folder` the current save folder and moves it to the top of the recent list.
    pub fn use_folder(&mut self, folder: &str) {
        let key = folder.trim().to_lowercase();
        self.recent_folders.retain(|f| f.trim().to_lowercase() != key);
        self.recent_folders.insert(0, folder.to_string());
        self.recent_folders.truncate(MAX_RECENT);
        self.folder = folder.to_string();
    }

    /// The folder to save into, given the user's Pictures folder.
    pub fn save_folder(&self, pictures: &Path) -> PathBuf {
        if self.folder.trim().is_empty() {
            pictures.join("Rengi Snap")
        } else {
            PathBuf::from(&self.folder)
        }
    }
}

/// `Snap 2026-10-27 01-42-05.png`, or `... (2).png` and up if taken.
pub fn capture_file_name(stamp: (u16, u16, u16, u16, u16, u16), exists: impl Fn(&str) -> bool) -> String {
    stamped_name("Snap", "png", stamp, exists)
}

/// `{prefix} 2026-10-27 01-42-05.{ext}` with ` (n)` added if taken.
pub fn stamped_name(prefix: &str, ext: &str, stamp: (u16, u16, u16, u16, u16, u16), exists: impl Fn(&str) -> bool) -> String {
    let (y, mo, d, h, mi, s) = stamp;
    let base = format!("{prefix} {y:04}-{mo:02}-{d:02} {h:02}-{mi:02}-{s:02}");
    let first = format!("{base}.{ext}");
    if !exists(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{base} ({n}).{ext}"))
        .find(|name| !exists(name))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let c = Config::default();
        assert_eq!(Config::parse(&c.to_text()), c);
        assert_eq!(c.hotkey.key, VK_SCROLL);
        assert!(c.loupe);
    }

    #[test]
    fn video_settings() {
        let c = Config::parse("mode = \"video\"\nvideo_fps = 120\nvideo_res = \"720p\"\nmic = true");
        assert_eq!((c.mode.as_str(), c.video_fps, c.video_res.as_str(), c.mic), ("video", 120, "720p", true));
        let c = Config::parse("mode = \"x\"\nvideo_fps = 7\nvideo_res = \"8k\"");
        assert_eq!((c.mode.as_str(), c.video_fps, c.video_res.as_str()), ("photo", 60, "original"));
    }

    #[test]
    fn garbage_falls_back() {
        assert_eq!(Config::parse("this is not toml ]]["), Config::default());
        // One bad value only resets that value.
        let c = Config::parse("loupe = false\ncard_seconds = \"nope\"");
        assert!(!c.loupe);
        assert_eq!(c.card_seconds, 6);
        let c = Config::parse("[hotkey]\nkey = 0");
        assert_eq!(c.hotkey, Hotkey::default());
    }

    #[test]
    fn recent_folders_dedupe_and_cap() {
        let mut c = Config::default();
        for i in 0..12 {
            c.use_folder(&format!("C:\\f{i}"));
        }
        c.use_folder("C:\\F3");
        assert_eq!(c.recent_folders.len(), MAX_RECENT);
        assert_eq!(c.recent_folders[0], "C:\\F3");
        assert_eq!(c.recent_folders.iter().filter(|f| f.to_lowercase() == "c:\\f3").count(), 1);
        assert_eq!(c.folder, "C:\\F3");
    }

    #[test]
    fn save_folder_default() {
        let c = Config::default();
        assert_eq!(c.save_folder(Path::new("P")), Path::new("P").join("Rengi Snap"));
    }

    #[test]
    fn file_names_do_not_overwrite() {
        let stamp = (2026, 10, 27, 1, 42, 5);
        assert_eq!(capture_file_name(stamp, |_| false), "Snap 2026-10-27 01-42-05.png");
        let taken = ["Snap 2026-10-27 01-42-05.png", "Snap 2026-10-27 01-42-05 (2).png"];
        assert_eq!(capture_file_name(stamp, |n| taken.contains(&n)), "Snap 2026-10-27 01-42-05 (3).png");
    }
}

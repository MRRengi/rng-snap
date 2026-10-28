//! The latest captures in the save folder, for Home's Recent grid.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
    Gif,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: Kind,
    pub modified: SystemTime,
}

pub fn kind_of(path: &Path) -> Option<Kind> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" => Some(Kind::Image),
        "mp4" => Some(Kind::Video),
        "gif" => Some(Kind::Gif),
        _ => None,
    }
}

/// Newest first, at most `n`; unfinished files (`.part`) are skipped.
pub fn list(dir: &Path, n: usize) -> Vec<Entry> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<Entry> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let kind = kind_of(&path)?;
            let md = e.metadata().ok()?;
            if !md.is_file() || md.len() == 0 {
                return None;
            }
            Some(Entry { path, kind, modified: md.modified().ok()? })
        })
        .collect();
    v.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.path.cmp(&b.path)));
    v.truncate(n);
    v
}

/// "just now", "1 min ago", "3 hours ago", "yesterday", "5 days ago", "2 weeks ago".
pub fn ago(secs: u64) -> String {
    let m = secs / 60;
    let h = m / 60;
    let d = h / 24;
    match () {
        _ if secs < 45 => "just now".into(),
        _ if m < 2 => "1 min ago".into(),
        _ if m < 60 => format!("{m} min ago"),
        _ if h < 2 => "1 hour ago".into(),
        _ if h < 24 => format!("{h} hours ago"),
        _ if d < 2 => "yesterday".into(),
        _ if d < 14 => format!("{d} days ago"),
        _ if d < 60 => format!("{} weeks ago", d / 7),
        _ => format!("{} months ago", d / 30),
    }
}

/// Middle-ellipsis for long names: "Snap 2026-10-…-05.png".
pub fn shorten(name: &str, max: usize) -> String {
    let c: Vec<char> = name.chars().collect();
    if c.len() <= max || max < 5 {
        return name.to_string();
    }
    let tail = (max - 1) / 2;
    let head = max - 1 - tail;
    format!("{}\u{2026}{}", c[..head].iter().collect::<String>(), c[c.len() - tail..].iter().collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages() {
        assert_eq!(ago(3), "just now");
        assert_eq!(ago(70), "1 min ago");
        assert_eq!(ago(60 * 59), "59 min ago");
        assert_eq!(ago(3600 + 5), "1 hour ago");
        assert_eq!(ago(3600 * 5), "5 hours ago");
        assert_eq!(ago(3600 * 30), "yesterday");
        assert_eq!(ago(86400 * 4), "4 days ago");
        assert_eq!(ago(86400 * 21), "3 weeks ago");
        assert_eq!(ago(86400 * 90), "3 months ago");
    }

    #[test]
    fn lists_newest_captures_only() {
        let dir = std::env::temp_dir().join(format!("rengi-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (i, name) in ["a.png", "b.mp4", "c.gif", "notes.txt", "d.mp4.part", "e.PNG"].iter().enumerate() {
            let p = dir.join(name);
            std::fs::write(&p, b"x").unwrap();
            let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000 + i as u64 * 10);
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(t).unwrap();
        }
        std::fs::write(dir.join("empty.png"), b"").unwrap();
        let v = list(&dir, 10);
        let names: Vec<String> = v.iter().map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["e.PNG", "c.gif", "b.mp4", "a.png"]);
        assert_eq!(v[0].kind, Kind::Image);
        assert_eq!(v[1].kind, Kind::Gif);
        assert_eq!(v[2].kind, Kind::Video);
        assert_eq!(list(&dir, 2).len(), 2);
        assert!(list(&dir.join("missing"), 5).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shortens_in_the_middle() {
        assert_eq!(shorten("short.png", 20), "short.png");
        let s = shorten("Snap 2026-10-28 20-37-05.png", 16);
        assert_eq!(s.chars().count(), 16);
        assert!(s.starts_with("Snap 20"));
        assert!(s.ends_with("05.png"));
    }
}

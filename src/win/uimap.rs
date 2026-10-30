//! Test hook: when `RENGI_SNAP_HOME` is set, windows publish where their
//! controls are (screen pixels) to `ui.txt` there, one `name l t r b` per
//! line, so the end-to-end test clicks by name instead of by guessed pixels.

use crate::geom::Rect;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

thread_local! {
    static MAP: RefCell<BTreeMap<String, Rect>> = const { RefCell::new(BTreeMap::new()) };
    static FILE: Option<PathBuf> = std::env::var_os("RENGI_SNAP_HOME").map(|h| PathBuf::from(h).join("ui.txt"));
}

fn enabled() -> bool {
    FILE.with(|f| f.is_some())
}

fn write() {
    let text = MAP.with(|m| m.borrow().iter().map(|(k, r)| format!("{k} {} {} {} {}\n", r.left, r.top, r.right, r.bottom)).collect::<String>());
    FILE.with(|f| {
        if let Some(p) = f {
            let tmp = p.with_extension("tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(&tmp, p);
            }
        }
    });
}

pub fn put(name: &str, r: Rect) {
    if !enabled() {
        return;
    }
    let changed = MAP.with(|m| m.borrow_mut().insert(name.to_string(), r) != Some(r));
    if changed {
        write();
    }
}

/// Forgets every control of one window (`prefix.`).
pub fn clear(prefix: &str) {
    if !enabled() {
        return;
    }
    let p = format!("{prefix}.");
    let removed = MAP.with(|m| {
        let mut m = m.borrow_mut();
        let before = m.len();
        m.retain(|k, _| !k.starts_with(&p));
        before != m.len()
    });
    if removed {
        write();
    }
}

/// Sets exactly these controls under `prefix.` (others under it are dropped),
/// writing the file only when something changed.
pub fn replace(prefix: &str, items: &[(String, Rect)]) {
    if !enabled() {
        return;
    }
    let p = format!("{prefix}.");
    let changed = MAP.with(|m| {
        let mut m = m.borrow_mut();
        let old: Vec<(String, Rect)> = m.iter().filter(|(k, _)| k.starts_with(&p)).map(|(k, r)| (k.clone(), *r)).collect();
        let mut new = items.to_vec();
        new.sort_by(|a, b| a.0.cmp(&b.0));
        if old == new {
            return false;
        }
        m.retain(|k, _| !k.starts_with(&p));
        for (k, r) in new {
            m.insert(k, r);
        }
        true
    });
    if changed {
        write();
    }
}

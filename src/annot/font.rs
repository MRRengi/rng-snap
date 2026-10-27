//! Fonts for text marks and step numbers: Segoe UI from the Windows fonts
//! folder, with common fallbacks (DejaVu on Linux for the tests).

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

pub struct Fonts {
    pub text: FontArc,
    pub bold: FontArc,
}

fn font_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(w) = std::env::var_os("WINDIR") {
        v.push(PathBuf::from(w).join("Fonts"));
    }
    v.push(PathBuf::from("C:\\Windows\\Fonts"));
    v.push(PathBuf::from("/usr/share/fonts/truetype/dejavu"));
    v
}

fn load_first(names: &[&str]) -> Option<FontArc> {
    for dir in font_dirs() {
        for n in names {
            if let Ok(bytes) = std::fs::read(dir.join(n)) {
                if let Ok(f) = FontArc::try_from_vec(bytes) {
                    return Some(f);
                }
            }
        }
    }
    None
}

impl Fonts {
    pub fn load() -> Option<Fonts> {
        let text = load_first(&["seguisb.ttf", "segoeui.ttf", "arial.ttf", "DejaVuSans.ttf"])?;
        let bold = load_first(&["segoeuib.ttf", "arialbd.ttf", "DejaVuSans-Bold.ttf"]).unwrap_or_else(|| text.clone());
        Some(Fonts { text, bold })
    }
}

/// Width of one line of text in pixels.
pub fn line_width(font: &FontArc, size: f32, line: &str) -> f32 {
    let f = font.as_scaled(PxScale::from(size));
    let mut w = 0.0;
    let mut prev = None;
    for c in line.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            w += f.kern(p, id);
        }
        w += f.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Line height (ascent - descent + gap).
pub fn line_height(font: &FontArc, size: f32) -> f32 {
    let f = font.as_scaled(PxScale::from(size));
    f.height() + f.line_gap()
}

/// Bounding box of a (multi-line) text block placed with its top-left at `(x, y)`.
pub fn text_box(font: &FontArc, size: f32, text: &str, x: f32, y: f32) -> (f32, f32, f32, f32) {
    let lh = line_height(font, size);
    let lines: Vec<&str> = text.split('\n').collect();
    let w = lines.iter().map(|l| line_width(font, size, l)).fold(0.0f32, f32::max);
    (x, y, x + w.max(size * 0.3), y + lh * lines.len().max(1) as f32)
}

/// Fonts loaded once per process (None if no usable font was found).
pub fn shared() -> Option<Arc<Fonts>> {
    static F: OnceLock<Option<Arc<Fonts>>> = OnceLock::new();
    F.get_or_init(|| Fonts::load().map(Arc::new)).clone()
}

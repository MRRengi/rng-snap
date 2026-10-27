//! A row of tool buttons shared by the overlay strip and the editor toolbar.

use super::d2d::{self, Gfx, rect_f, rgba};
use super::overlay::ACCENT;
use crate::annot::{BlurMode, Color, PALETTE, Tool, icons};
use crate::geom::{Point, Rect};
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Btn {
    Tool(Tool),
    Color(usize),
    Custom,
    /// Current colour; opens the palette row (overlay strip).
    Palette,
    Width(usize),
    /// Cycles the three thicknesses (overlay strip).
    WidthCycle,
    BlurMode,
    Undo,
    Redo,
    Copy,
    Save,
    SaveTo,
    OpenEditor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    B(Btn),
    Sep,
    /// Pushes everything after it to the right edge (editor toolbar).
    Spring,
}

/// What the buttons show.
#[derive(Clone, Copy, Debug)]
pub struct StripState {
    pub tool: Tool,
    pub color: Color,
    pub width_idx: usize,
    pub blur_mode: BlurMode,
    pub can_undo: bool,
    pub can_redo: bool,
    pub custom: Color,
}

pub struct Fonts {
    pub label: IDWriteTextFormat,
    pub icon: IDWriteTextFormat,
}

pub struct Strip {
    pub items: Vec<(Btn, Rect)>,
    seps: Vec<i32>,
    pub rect: Rect,
    pub hover: Option<Btn>,
    pub press: Option<Btn>,
    scale: f32,
    icons: Vec<(Tool, ID2D1Bitmap)>,
    icon_rt: usize,
    fonts: Option<(f32, Fonts)>,
}

fn label(b: Btn) -> Option<(&'static str, &'static str)> {
    match b {
        Btn::Copy => Some(("\u{E8C8}", "Copy")),
        Btn::Save => Some(("\u{E74E}", "Save")),
        Btn::SaveTo => Some(("\u{E792}", "Save to")),
        Btn::OpenEditor => Some(("\u{E70F}", "Editor")),
        _ => None,
    }
}

impl Strip {
    pub fn new() -> Strip {
        Strip { items: Vec::new(), seps: Vec::new(), rect: Rect::default(), hover: None, press: None, scale: 1.0, icons: Vec::new(), icon_rt: 0, fonts: None }
    }

    pub fn fonts(&mut self, gfx: &Gfx, scale: f32) -> Option<&Fonts> {
        if self.fonts.as_ref().map(|f| f.0) != Some(scale) {
            let label = gfx.ui_font(13.0 * scale, false).ok()?;
            let icon = gfx.icon_font(15.0 * scale).ok()?;
            self.fonts = Some((scale, Fonts { label, icon }));
        }
        self.fonts.as_ref().map(|f| &f.1)
    }

    fn button_w(&mut self, gfx: &Gfx, b: Btn, s: f32) -> i32 {
        let unit = (34.0 * s).round() as i32;
        match b {
            Btn::Color(_) | Btn::Custom => (26.0 * s).round() as i32,
            Btn::Width(_) => (28.0 * s).round() as i32,
            Btn::BlurMode => (62.0 * s).round() as i32,
            _ => match label(b) {
                Some((_, text)) => {
                    let w = self.fonts(gfx, s).map(|f| gfx.measure(&f.label, text)).unwrap_or(40.0);
                    (w + 44.0 * s).round() as i32
                }
                None => unit,
            },
        }
    }

    /// Lays out the slots left to right starting at `origin`; `min_w` stretches
    /// springs so the row fills that width.
    pub fn layout(&mut self, gfx: &Gfx, slots: &[Slot], origin: Point, height: i32, min_w: i32, scale: f32) {
        self.scale = scale;
        let pad = (6.0 * scale).round() as i32;
        let sep_w = (13.0 * scale).round() as i32;
        let widths: Vec<i32> = slots
            .iter()
            .map(|s| match s {
                Slot::B(b) => self.button_w(gfx, *b, scale),
                Slot::Sep => sep_w,
                Slot::Spring => 0,
            })
            .collect();
        let natural: i32 = pad * 2 + widths.iter().sum::<i32>();
        let springs = slots.iter().filter(|s| matches!(s, Slot::Spring)).count() as i32;
        let extra = if springs > 0 { ((min_w - natural).max(0)) / springs } else { 0 };
        self.items.clear();
        self.seps.clear();
        let mut x = origin.x + pad;
        for (s, w) in slots.iter().zip(widths) {
            match s {
                Slot::B(b) => {
                    self.items.push((*b, Rect::new(x, origin.y + pad, x + w, origin.y + height - pad)));
                    x += w;
                }
                Slot::Sep => {
                    self.seps.push(x + w / 2);
                    x += w;
                }
                Slot::Spring => x += extra,
            }
        }
        self.rect = Rect::new(origin.x, origin.y, (x + pad).max(origin.x + min_w), origin.y + height);
    }

    pub fn hit(&self, p: Point) -> Option<Btn> {
        self.items.iter().find(|(_, r)| r.contains(p)).map(|(b, _)| *b)
    }

    pub fn translate(&mut self, dx: i32, dy: i32) {
        self.rect = self.rect.offset(dx, dy);
        for (_, r) in &mut self.items {
            *r = r.offset(dx, dy);
        }
        for s in &mut self.seps {
            *s += dx;
        }
    }

    fn icon(&mut self, rt: &ID2D1RenderTarget, t: Tool) -> Option<ID2D1Bitmap> {
        let key = windows::core::Interface::as_raw(rt) as usize;
        if key != self.icon_rt {
            self.icons.clear();
            self.icon_rt = key;
        }
        if let Some((_, b)) = self.icons.iter().find(|(x, _)| *x == t) {
            return Some(b.clone());
        }
        let n = (20.0 * self.scale).round().max(12.0) as u32;
        let b = d2d::bitmap_from_pixmap(rt, &icons::tool_icon(t, n)).ok()?;
        self.icons.push((t, b.clone()));
        Some(b)
    }

    /// Draws the strip; `off` converts strip coordinates to render-target ones.
    pub fn draw(&mut self, rt: &ID2D1RenderTarget, gfx: &Gfx, st: &StripState, off: (f32, f32), background: bool) {
        let s = self.scale;
        let Some(fonts) = self.fonts(gfx, s).map(|f| (f.label.clone(), f.icon.clone())) else { return };
        let (Ok(bg), Ok(border), Ok(fg), Ok(dim), Ok(sel), Ok(hov), Ok(sep), Ok(accent)) = (
            d2d::brush(rt, rgba(0x1E1E20, 0.97)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.10)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.95)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.32)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.15)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.07)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.12)),
            d2d::brush(rt, rgba(ACCENT, 1.0)),
        ) else {
            return;
        };
        let f = |r: Rect| rect_f(r.left as f32 + off.0, r.top as f32 + off.1, r.right as f32 + off.0, r.bottom as f32 + off.1);
        unsafe {
            if background {
                let r = f(self.rect);
                rt.FillRoundedRectangle(&d2d::round_rect(r, 10.0 * s), &bg);
                rt.DrawRoundedRectangle(&d2d::round_rect(rect_f(r.left + 0.5, r.top + 0.5, r.right - 0.5, r.bottom - 0.5), 10.0 * s), &border, 1.0, None);
            }
            for &x in &self.seps {
                let x = x as f32 + off.0;
                rt.FillRectangle(&rect_f(x, self.rect.top as f32 + off.1 + 12.0 * s, x + 1.0, self.rect.bottom as f32 + off.1 - 12.0 * s), &sep);
            }
        }
        let items = self.items.clone();
        for (b, r) in items {
            let lr = f(r);
            let (cx, cy) = ((lr.left + lr.right) / 2.0, (lr.top + lr.bottom) / 2.0);
            let enabled = match b {
                Btn::Undo => st.can_undo,
                Btn::Redo => st.can_redo,
                Btn::BlurMode => st.tool == Tool::Blur,
                _ => true,
            };
            let active = match b {
                Btn::Tool(t) => t == st.tool,
                Btn::Width(i) => i == st.width_idx,
                _ => false,
            };
            let rr = d2d::round_rect(lr, 7.0 * s);
            unsafe {
                if enabled && (active || self.press == Some(b)) {
                    rt.FillRoundedRectangle(&rr, &sel);
                } else if enabled && self.hover == Some(b) {
                    rt.FillRoundedRectangle(&rr, &hov);
                }
            }
            match b {
                Btn::Tool(t) => {
                    if let Some(bmp) = self.icon(rt, t) {
                        let n = (20.0 * s).round();
                        let d = rect_f((cx - n / 2.0).round(), (cy - n / 2.0).round(), (cx - n / 2.0).round() + n, (cy - n / 2.0).round() + n);
                        unsafe { rt.DrawBitmap(&bmp, Some(&d), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
                    }
                }
                Btn::Color(_) | Btn::Custom | Btn::Palette => {
                    let c = match b {
                        Btn::Color(i) => PALETTE[i],
                        Btn::Custom => st.custom,
                        _ => st.color,
                    };
                    let r = if b == Btn::Palette { 8.5 * s } else { 8.0 * s };
                    let e = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
                    if let Ok(cb) = d2d::brush(rt, rgba(c.hex(), 1.0)) {
                        unsafe {
                            rt.FillEllipse(&e, &cb);
                            rt.DrawEllipse(&e, &border, 1.0, None);
                        }
                    }
                    let picked = matches!(b, Btn::Color(i) if PALETTE[i] == st.color) || (b == Btn::Custom && st.custom == st.color && !PALETTE.contains(&st.color));
                    if picked {
                        let e2 = D2D1_ELLIPSE { point: e.point, radiusX: r + 3.0 * s, radiusY: r + 3.0 * s };
                        unsafe { rt.DrawEllipse(&e2, &fg, 1.6 * s, None) };
                    }
                    if b == Btn::Custom {
                        d2d::draw_text(rt, &fonts.0, "+", rect_f(cx - 4.0 * s, cy - 10.0 * s, cx + 8.0 * s, cy + 10.0 * s), if st.custom.luma() > 0.6 { &sel } else { &fg });
                    }
                }
                Btn::Width(_) | Btn::WidthCycle => {
                    let i = if let Btn::Width(i) = b { i } else { st.width_idx };
                    let r = [2.0, 3.5, 5.5][i.min(2)] * s;
                    let e = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: cx, Y: cy }, radiusX: r, radiusY: r };
                    unsafe { rt.FillEllipse(&e, &fg) };
                }
                Btn::BlurMode => {
                    let t = if st.blur_mode == BlurMode::Blur { "Blur" } else { "Pixels" };
                    d2d::draw_text(rt, &fonts.0, t, rect_f(lr.left + 10.0 * s, cy - 9.0 * s, lr.right, cy + 10.0 * s), if enabled { &fg } else { &dim });
                }
                Btn::Undo | Btn::Redo => {
                    let g = if b == Btn::Undo { "\u{E7A7}" } else { "\u{E7A6}" };
                    d2d::draw_text(rt, &fonts.1, g, lr, if enabled { &fg } else { &dim });
                }
                _ => {
                    if let Some((g, text)) = label(b) {
                        let x0 = lr.left + 10.0 * s;
                        let icon_c = if b == Btn::Copy { &accent } else { &fg };
                        d2d::draw_text(rt, &fonts.1, g, rect_f(x0, lr.top, x0 + 18.0 * s, lr.bottom), icon_c);
                        d2d::draw_text(rt, &fonts.0, text, rect_f(x0 + 24.0 * s, cy - 9.0 * s, lr.right, cy + 10.0 * s), &fg);
                    }
                }
            }
        }
    }
}

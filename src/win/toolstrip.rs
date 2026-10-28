//! A row of tool buttons shared by the overlay strip and the editor toolbars,
//! drawn as a Liquid Glass capsule.

use super::d2d::{self, Gfx, rect_f};
use super::theme::{self, Backdrop, Fader};
use crate::annot::{BlurMode, Color, PALETTE, Tool, icons};
use crate::geom::{Point, Rect};
use crate::palette::Rgba;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Btn {
    Tool(Tool),
    Color(usize),
    Custom,
    /// Current colour; opens the palette row (overlay strip).
    Palette,
    Width(usize),
    /// Cycles the three thicknesses.
    WidthCycle,
    BlurMode,
    Undo,
    Redo,
    Copy,
    Save,
    SaveTo,
    OpenEditor,
    /// Video editor: write the edited video.
    Export,
    /// Video editor: GIF of the edited video.
    Gif,
}

impl Btn {
    /// Stable number for hover animation.
    pub fn key(self) -> u64 {
        match self {
            Btn::Tool(t) => 100 + t as u64,
            Btn::Color(i) => 200 + i as u64,
            Btn::Width(i) => 300 + i as u64,
            Btn::Custom => 1,
            Btn::Palette => 2,
            Btn::WidthCycle => 3,
            Btn::BlurMode => 4,
            Btn::Undo => 5,
            Btn::Redo => 6,
            Btn::Copy => 7,
            Btn::Save => 8,
            Btn::SaveTo => 9,
            Btn::OpenEditor => 10,
            Btn::Export => 11,
            Btn::Gif => 12,
        }
    }

    /// Filled accent capsule: the one action the window exists for.
    fn prominent(self) -> bool {
        matches!(self, Btn::Copy | Btn::Export)
    }

    pub fn name(self) -> String {
        match self {
            Btn::Tool(t) => format!("tool.{}", t.name()),
            Btn::Color(i) => format!("color.{i}"),
            Btn::Width(i) => format!("width.{i}"),
            other => format!("{other:?}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    B(Btn),
    Sep,
    /// Pushes everything after it to the right edge.
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
    icons: Vec<(Tool, u32, ID2D1Bitmap)>,
    icon_rt: usize,
    fonts: Option<(f32, Fonts)>,
    fader: Fader,
    /// Hover fades in and out (windows with an animation timer).
    pub animate: bool,
}

fn label(b: Btn) -> Option<(&'static str, &'static str)> {
    match b {
        Btn::Copy => Some(("\u{E8C8}", "Copy")),
        Btn::Save => Some(("\u{E74E}", "Save")),
        Btn::SaveTo => Some(("\u{E792}", "Save to")),
        Btn::OpenEditor => Some(("\u{E70F}", "Editor")),
        Btn::Export => Some(("\u{EDE1}", "Export")),
        Btn::Gif => Some(("\u{E8B9}", "GIF")),
        _ => None,
    }
}

impl Strip {
    pub fn new() -> Strip {
        Strip {
            items: Vec::new(),
            seps: Vec::new(),
            rect: Rect::default(),
            hover: None,
            press: None,
            scale: 1.0,
            icons: Vec::new(),
            icon_rt: 0,
            fonts: None,
            fader: Fader::default(),
            animate: false,
        }
    }

    pub fn fonts(&mut self, gfx: &Gfx, scale: f32) -> Option<&Fonts> {
        if self.fonts.as_ref().map(|f| f.0) != Some(scale) {
            let label = gfx.ui_font(13.0 * scale, true).ok()?;
            let icon = gfx.icon_font(15.0 * scale).ok()?;
            self.fonts = Some((scale, Fonts { label, icon }));
        }
        self.fonts.as_ref().map(|f| &f.1)
    }

    /// Sets the hovered button; true when a fade has to run.
    pub fn set_hover(&mut self, b: Option<Btn>) -> bool {
        self.hover = b;
        self.fader.set(b.map(Btn::key));
        self.animate && self.fader.step()
    }

    /// Advances the hover fade; true while it still moves.
    pub fn step(&mut self) -> bool {
        self.fader.step()
    }

    fn hover_amount(&self, b: Btn) -> f32 {
        if self.animate {
            self.fader.get(b.key())
        } else if self.hover == Some(b) {
            1.0
        } else {
            0.0
        }
    }

    fn button_w(&mut self, gfx: &Gfx, b: Btn, s: f32, h: i32) -> i32 {
        let unit = h;
        match b {
            Btn::Color(_) | Btn::Custom => (26.0 * s).round() as i32,
            Btn::Width(_) => (28.0 * s).round() as i32,
            Btn::BlurMode => (70.0 * s).round() as i32,
            _ => match label(b) {
                Some((_, text)) => {
                    let w = self.fonts(gfx, s).map(|f| gfx.measure(&f.label, text)).unwrap_or(40.0);
                    (w + 46.0 * s).round() as i32
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
        let inner_h = height - pad * 2;
        let sep_w = (15.0 * scale).round() as i32;
        let widths: Vec<i32> = slots
            .iter()
            .map(|s| match s {
                Slot::B(b) => self.button_w(gfx, *b, scale, inner_h),
                Slot::Sep => sep_w,
                Slot::Spring => 0,
            })
            .collect();
        let side = (pad as f32 * 1.5).round() as i32;
        let natural: i32 = side * 2 + widths.iter().sum::<i32>();
        let springs = slots.iter().filter(|s| matches!(s, Slot::Spring)).count() as i32;
        let extra = if springs > 0 { ((min_w - natural).max(0)) / springs } else { 0 };
        self.items.clear();
        self.seps.clear();
        let mut x = origin.x + side;
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
        self.rect = Rect::new(origin.x, origin.y, (x + side).max(origin.x + min_w), origin.y + height);
    }

    /// Lays out at `scale`, shrinking (down to 75%) when the natural width
    /// doesn't fit `max_w`, then centres the row in `max_w` starting at `x0`.
    pub fn layout_fit(&mut self, gfx: &Gfx, slots: &[Slot], x0: i32, y: i32, height: i32, max_w: i32, scale: f32) {
        self.layout(gfx, slots, Point::new(0, y), height, 0, scale);
        let w = self.rect.width();
        if w > max_w && w > 0 {
            let k = (max_w as f32 / w as f32).max(0.75);
            self.layout(gfx, slots, Point::new(0, y), (height as f32 * k).round() as i32, 0, scale * k);
        }
        let x = x0 + ((max_w - self.rect.width()) / 2).max(0);
        self.translate(x, 0);
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

    /// Button rects in screen coordinates for the end-to-end test map.
    pub fn publish(&self, window: &str, to_screen: impl Fn(Rect) -> Rect) {
        for (b, r) in &self.items {
            super::uimap::put(&format!("{window}.{}", b.name()), to_screen(*r));
        }
    }

    fn icon(&mut self, rt: &ID2D1RenderTarget, t: Tool) -> Option<ID2D1Bitmap> {
        let key = windows::core::Interface::as_raw(rt) as usize;
        if key != self.icon_rt {
            self.icons.clear();
            self.icon_rt = key;
        }
        let n = (20.0 * self.scale).round().max(12.0) as u32;
        if let Some((_, _, b)) = self.icons.iter().find(|(x, sz, _)| *x == t && *sz == n) {
            return Some(b.clone());
        }
        let b = d2d::bitmap_from_pixmap(rt, &icons::tool_icon(t, n)).ok()?;
        self.icons.retain(|(x, _, _)| *x != t);
        self.icons.push((t, n, b.clone()));
        Some(b)
    }

    /// Draws the strip; `off` converts strip coordinates to render-target
    /// ones. `glass`: draw the capsule behind the buttons, blurring `back`.
    pub fn draw(&mut self, rt: &ID2D1RenderTarget, gfx: &Gfx, st: &StripState, off: (f32, f32), glass: Option<Option<&Backdrop>>) {
        let s = self.scale;
        let p = theme::pal();
        let Some(fonts) = self.fonts(gfx, s).map(|f| (f.label.clone(), f.icon.clone())) else { return };
        let f = |r: Rect| rect_f(r.left as f32 + off.0, r.top as f32 + off.1, r.right as f32 + off.0, r.bottom as f32 + off.1);
        let (Some(fg), Some(dim), Some(sep), Some(accent), Some(on_accent), Some(hair)) =
            (theme::brush(rt, p.label), theme::brush(rt, p.label3), theme::brush(rt, p.separator), theme::brush(rt, p.accent), theme::brush(rt, p.on_accent), theme::brush(rt, p.hairline))
        else {
            return;
        };
        if let Some(back) = glass {
            let r = f(self.rect);
            let radius = (r.bottom - r.top) / 2.0;
            theme::glass(rt, gfx, r, radius, s, back);
        }
        unsafe {
            for &x in &self.seps {
                let x = x as f32 + off.0;
                let h = (self.rect.height() as f32) * 0.24;
                rt.FillRectangle(&rect_f(x, self.rect.top as f32 + off.1 + h, x + 1.0, self.rect.bottom as f32 + off.1 - h), &sep);
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
            let pressed = self.press == Some(b);
            let hov = if enabled { self.hover_amount(b) } else { 0.0 };
            let radius = (lr.bottom - lr.top) / 2.0;
            // Press feedback: shrink a touch (Emil: scale 0.97 on :active).
            let shape = if pressed { theme::inset(lr, 1.0 * s) } else { lr };
            let rr = theme::round(shape, radius);
            unsafe {
                if b.prominent() {
                    let c = if pressed { theme::with_alpha(p.accent, 0.82) } else { p.accent };
                    if let Some(ab) = theme::brush(rt, c) {
                        rt.FillRoundedRectangle(&rr, &ab);
                    }
                    if hov > 0.0 {
                        if let Some(hb) = theme::brush(rt, Rgba(0xFFFFFF, 0.14 * hov)) {
                            rt.FillRoundedRectangle(&rr, &hb);
                        }
                    }
                } else if enabled && (active || pressed) {
                    // The raised glass thumb under the current tool.
                    if let Some(tb) = theme::brush(rt, p.thumb) {
                        rt.FillRoundedRectangle(&rr, &tb);
                    }
                    if !p.dark {
                        rt.DrawRoundedRectangle(&theme::round(theme::inset(shape, 0.5), radius), &hair, 1.0, None);
                    }
                } else if hov > 0.0 {
                    if let Some(hb) = theme::brush(rt, theme::with_alpha(p.hover, hov)) {
                        rt.FillRoundedRectangle(&rr, &hb);
                    }
                }
            }
            match b {
                Btn::Tool(t) => {
                    if let Some(bmp) = self.icon(rt, t) {
                        let n = (20.0 * s).round();
                        let d = rect_f((cx - n / 2.0).round(), (cy - n / 2.0).round(), (cx - n / 2.0).round() + n, (cy - n / 2.0).round() + n);
                        theme::tinted(rt, &bmp, d, if active { p.accent } else { p.label });
                    }
                }
                Btn::Color(_) | Btn::Custom | Btn::Palette => {
                    let c = match b {
                        Btn::Color(i) => PALETTE[i],
                        Btn::Custom => st.custom,
                        _ => st.color,
                    };
                    let rad = if b == Btn::Palette { 9.0 * s } else { 8.0 * s };
                    let e = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: cx, Y: cy }, radiusX: rad, radiusY: rad };
                    if let Ok(cb) = d2d::brush(rt, d2d::rgba(c.hex(), 1.0)) {
                        unsafe {
                            rt.FillEllipse(&e, &cb);
                            rt.DrawEllipse(&e, &hair, 1.0, None);
                        }
                    }
                    let picked = matches!(b, Btn::Color(i) if PALETTE[i] == st.color) || (b == Btn::Custom && st.custom == st.color && !PALETTE.contains(&st.color));
                    if picked {
                        let e2 = D2D1_ELLIPSE { point: e.point, radiusX: rad + 3.5 * s, radiusY: rad + 3.5 * s };
                        unsafe { rt.DrawEllipse(&e2, &fg, 1.8 * s, None) };
                    }
                    if b == Btn::Custom {
                        let ink = if st.custom.luma() > 0.6 { Rgba(0x000000, 0.8) } else { Rgba(0xFFFFFF, 1.0) };
                        if let Some(ib) = theme::brush(rt, ink) {
                            unsafe {
                                rt.FillRectangle(&rect_f(cx - 4.0 * s, cy - 0.75 * s, cx + 4.0 * s, cy + 0.75 * s), &ib);
                                rt.FillRectangle(&rect_f(cx - 0.75 * s, cy - 4.0 * s, cx + 0.75 * s, cy + 4.0 * s), &ib);
                            }
                        }
                    }
                }
                Btn::Width(_) | Btn::WidthCycle => {
                    let i = if let Btn::Width(i) = b { i } else { st.width_idx };
                    let rad = [2.0, 3.5, 5.5][i.min(2)] * s;
                    let e = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: cx, Y: cy }, radiusX: rad, radiusY: rad };
                    unsafe { rt.FillEllipse(&e, if active { &accent } else { &fg }) };
                }
                Btn::BlurMode => {
                    let t = if st.blur_mode == BlurMode::Blur { "Blur" } else { "Pixels" };
                    let br = if enabled { &fg } else { &dim };
                    d2d::draw_text(rt, &fonts.0, t, rect_f(lr.left + 12.0 * s, cy - 9.5 * s, lr.right, cy + 10.0 * s), br);
                    d2d::draw_text(rt, &fonts.1, "\u{E70D}", rect_f(lr.right - 22.0 * s, lr.top, lr.right - 8.0 * s, lr.bottom), br);
                }
                Btn::Undo | Btn::Redo => {
                    let g = if b == Btn::Undo { "\u{E7A7}" } else { "\u{E7A6}" };
                    d2d::draw_text(rt, &fonts.1, g, lr, if enabled { &fg } else { &dim });
                }
                _ => {
                    if let Some((g, text)) = label(b) {
                        let x0 = lr.left + 14.0 * s;
                        let ink = if b.prominent() { &on_accent } else { &fg };
                        d2d::draw_text(rt, &fonts.1, g, rect_f(x0, lr.top, x0 + 16.0 * s, lr.bottom), ink);
                        d2d::draw_text(rt, &fonts.0, text, rect_f(x0 + 23.0 * s, cy - 9.5 * s, lr.right, cy + 10.0 * s), ink);
                    }
                }
            }
        }
    }
}

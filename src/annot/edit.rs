//! Editing state machine for a set of annotations: tools, selection,
//! moving, handles, text entry, crop and undo/redo. Pure logic; the overlay
//! and the editor window feed it mouse and key events in image coordinates.

use super::font::Fonts;
use super::render::bbox;
use super::{BlurMode, Color, Item, P, PALETTE, STEP_RADII, Shape, TEXT_SIZES, Tool, WIDTHS, norm, to_rect};
use crate::geom::Rect;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Delete,
    Escape,
    Enter,
    Backspace,
    Undo,
    Redo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    None,
    /// Something on screen changed (draft, selection, caret).
    Redraw,
    /// The image itself changed: worth copying to the clipboard.
    Committed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    A,
    B,
    /// 0 = top-left, 1 = top-right, 2 = bottom-right, 3 = bottom-left.
    Corner(u8),
}

#[derive(Clone, Debug, PartialEq)]
struct Snap {
    items: Vec<Item>,
    crop: Option<Rect>,
}

#[derive(Clone, Debug)]
enum Drag {
    None,
    Create,
    Move { last: P, before: Snap, moved: bool },
    Handle { h: Handle, before: Snap, moved: bool },
    Crop,
}

#[derive(Clone, Debug)]
pub struct TextEdit {
    /// Index the item came from when re-editing existing text.
    pub idx: Option<usize>,
    pub at: P,
    pub text: String,
    pub size: f32,
    pub color: Color,
    before: Snap,
}

pub struct Canvas {
    pub items: Vec<Item>,
    pub crop: Option<Rect>,
    /// Where drawing is allowed (image bounds, or the frame on the overlay).
    pub bounds: Rect,
    pub tool: Tool,
    pub color: Color,
    pub width_idx: usize,
    pub blur_mode: BlurMode,
    /// UI scale (DPI) applied to new strokes, text and steps.
    pub scale: f32,
    pub selected: Option<usize>,
    pub draft: Option<Item>,
    pub text: Option<TextEdit>,
    pub crop_draft: Option<(P, P)>,
    /// Bumped on every visible change; renderers cache on it.
    pub version: u64,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    drag: Drag,
    fonts: Option<Arc<Fonts>>,
}

fn seg_dist(p: P, a: P, b: P) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-6 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.dist(P::new(a.x + t * dx, a.y + t * dy))
}

/// Snaps `b` to 45 degree steps around `a`.
fn snap45(a: P, b: P) -> P {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    let ang = dy.atan2(dx);
    let step = std::f32::consts::FRAC_PI_4;
    let s = (ang / step).round() * step;
    P::new(a.x + len * s.cos(), a.y + len * s.sin())
}

fn square(a: P, b: P) -> P {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let m = dx.abs().max(dy.abs());
    P::new(a.x + m * dx.signum(), a.y + m * dy.signum())
}

impl Canvas {
    pub fn new(bounds: Rect, scale: f32, fonts: Option<Arc<Fonts>>) -> Canvas {
        Canvas {
            items: Vec::new(),
            crop: None,
            bounds,
            tool: Tool::Arrow,
            color: PALETTE[0],
            width_idx: 1,
            blur_mode: BlurMode::Blur,
            scale,
            selected: None,
            draft: None,
            text: None,
            crop_draft: None,
            version: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            drag: Drag::None,
            fonts,
        }
    }

    pub fn fonts(&self) -> Option<&Fonts> {
        self.fonts.as_deref()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn is_busy(&self) -> bool {
        !matches!(self.drag, Drag::None) || self.text.is_some()
    }

    fn snap(&self) -> Snap {
        Snap { items: self.items.clone(), crop: self.crop }
    }

    fn push(&mut self, before: Snap) {
        if before != self.snap() {
            self.undo.push(before);
            self.redo.clear();
        }
    }

    fn bump(&mut self, c: Change) -> Change {
        if c != Change::None {
            self.version += 1;
        }
        c
    }

    fn width(&self) -> f32 {
        WIDTHS[self.width_idx.min(2)] * self.scale
    }

    fn clamp(&self, p: P) -> P {
        let b = self.bounds;
        P::new(p.x.clamp(b.left as f32, b.right as f32), p.y.clamp(b.top as f32, b.bottom as f32))
    }

    fn inside(&self, p: P) -> bool {
        let b = self.bounds;
        p.x >= b.left as f32 && p.x < b.right as f32 && p.y >= b.top as f32 && p.y < b.bottom as f32
    }

    /// Items to render now: committed ones plus the draft and the text being typed.
    pub fn preview(&self) -> Vec<Item> {
        let mut v = self.items.clone();
        if let Some(d) = &self.draft {
            v.push(d.clone());
        }
        if let Some(t) = &self.text {
            let item = Item { shape: Shape::Text { at: t.at, text: t.text.clone(), size: t.size }, color: t.color, width: 0.0 };
            match t.idx {
                Some(i) if i <= v.len() => v.insert(i, item),
                _ => v.push(item),
            }
        }
        v
    }

    /// Topmost item under the point.
    pub fn hit(&self, p: P) -> Option<usize> {
        let tol = 5.0 * self.scale;
        self.items.iter().enumerate().rev().find_map(|(i, it)| {
            let w = it.width / 2.0 + tol;
            let ok = match &it.shape {
                Shape::Arrow { a, b } => seg_dist(p, *a, *b) <= w.max(it.width * 2.0),
                Shape::Line { a, b } => seg_dist(p, *a, *b) <= w,
                Shape::Rect { a, b } => {
                    let (l, t, r, bt) = norm(*a, *b);
                    let inside = p.x >= l - w && p.x <= r + w && p.y >= t - w && p.y <= bt + w;
                    let deep = p.x > l + w && p.x < r - w && p.y > t + w && p.y < bt - w;
                    inside && !deep
                }
                Shape::Ellipse { a, b } => {
                    let (l, t, r, bt) = norm(*a, *b);
                    let (cx, cy, rx, ry) = ((l + r) / 2.0, (t + bt) / 2.0, ((r - l) / 2.0).max(1.0), ((bt - t) / 2.0).max(1.0));
                    let d = (((p.x - cx) / rx).powi(2) + ((p.y - cy) / ry).powi(2)).sqrt();
                    (d - 1.0).abs() * rx.min(ry) <= w
                }
                Shape::Blur { a, b, .. } => {
                    let (l, t, r, bt) = norm(*a, *b);
                    p.x >= l && p.x <= r && p.y >= t && p.y <= bt
                }
                Shape::Pen { pts } | Shape::Highlighter { pts } => {
                    let ww = if matches!(it.shape, Shape::Highlighter { .. }) { it.width * 1.6 + tol } else { w };
                    pts.windows(2).any(|s| seg_dist(p, s[0], s[1]) <= ww) || (pts.len() == 1 && pts[0].dist(p) <= ww)
                }
                Shape::Text { .. } => {
                    let (l, t, r, b) = bbox(it, self.fonts());
                    p.x >= l && p.x <= r && p.y >= t && p.y <= b
                }
                Shape::Step { c, r } => c.dist(p) <= r + tol,
            };
            ok.then_some(i)
        })
    }

    /// Handle points of the selected item.
    pub fn handles(&self) -> Vec<(Handle, P)> {
        let Some(it) = self.selected.and_then(|i| self.items.get(i)) else { return Vec::new() };
        match &it.shape {
            Shape::Arrow { a, b } | Shape::Line { a, b } => vec![(Handle::A, *a), (Handle::B, *b)],
            Shape::Rect { a, b } | Shape::Ellipse { a, b } | Shape::Blur { a, b, .. } => {
                let (l, t, r, bt) = norm(*a, *b);
                vec![
                    (Handle::Corner(0), P::new(l, t)),
                    (Handle::Corner(1), P::new(r, t)),
                    (Handle::Corner(2), P::new(r, bt)),
                    (Handle::Corner(3), P::new(l, bt)),
                ]
            }
            _ => Vec::new(),
        }
    }

    fn handle_at(&self, p: P) -> Option<Handle> {
        let tol = 7.0 * self.scale;
        self.handles().into_iter().find(|(_, hp)| hp.dist(p) <= tol).map(|(h, _)| h)
    }

    /// True if a press at `p` would act on an item (Select tool). The overlay
    /// uses this to decide between editing marks and moving the frame.
    pub fn wants(&self, p: P) -> bool {
        self.handle_at(p).is_some() || self.hit(p).is_some()
    }

    pub fn set_tool(&mut self, t: Tool) -> Change {
        let mut c = self.finish_text();
        self.crop_draft = None;
        self.draft = None;
        self.drag = Drag::None;
        if t != Tool::Select && self.selected.take().is_some() && c == Change::None {
            c = Change::Redraw;
        }
        self.tool = t;
        self.bump(if c == Change::None { Change::Redraw } else { c })
    }

    /// New colour; also recolours the selected item or the text being typed.
    pub fn set_color(&mut self, col: Color) -> Change {
        self.color = col;
        if let Some(t) = &mut self.text {
            t.color = col;
            return self.bump(Change::Redraw);
        }
        if let Some(i) = self.selected {
            let before = self.snap();
            self.items[i].color = col;
            self.push(before);
            return self.bump(Change::Committed);
        }
        Change::None
    }

    pub fn set_width(&mut self, idx: usize) -> Change {
        self.width_idx = idx.min(2);
        let w = self.width();
        if let Some(t) = &mut self.text {
            t.size = TEXT_SIZES[self.width_idx] * self.scale;
            return self.bump(Change::Redraw);
        }
        if let Some(i) = self.selected {
            let before = self.snap();
            let it = &mut self.items[i];
            match &mut it.shape {
                Shape::Text { size, .. } => *size = TEXT_SIZES[self.width_idx] * self.scale,
                Shape::Step { r, .. } => *r = STEP_RADII[self.width_idx] * self.scale,
                _ => it.width = w,
            }
            self.push(before);
            return self.bump(Change::Committed);
        }
        Change::None
    }

    pub fn set_blur_mode(&mut self, m: BlurMode) -> Change {
        self.blur_mode = m;
        if let Some(i) = self.selected {
            if matches!(self.items[i].shape, Shape::Blur { .. }) {
                let before = self.snap();
                if let Shape::Blur { mode, .. } = &mut self.items[i].shape {
                    *mode = m;
                }
                self.push(before);
                return self.bump(Change::Committed);
            }
        }
        Change::None
    }

    pub fn mouse_down(&mut self, p: P, _shift: bool) -> Change {
        let mut c = Change::None;
        if let Some(t) = &self.text {
            let it = Item { shape: Shape::Text { at: t.at, text: t.text.clone(), size: t.size }, color: t.color, width: 0.0 };
            let (l, tp, r, b) = bbox(&it, self.fonts());
            if p.x >= l && p.x <= r && p.y >= tp && p.y <= b {
                return Change::None;
            }
            c = self.finish_text();
        }
        match self.tool {
            Tool::Select => {
                if let Some(h) = self.handle_at(p) {
                    self.drag = Drag::Handle { h, before: self.snap(), moved: false };
                    return self.bump(Change::Redraw.max_with(c));
                }
                let hit = self.hit(p);
                let changed = hit != self.selected;
                self.selected = hit;
                if hit.is_some() {
                    self.drag = Drag::Move { last: p, before: self.snap(), moved: false };
                }
                self.bump(if changed { Change::Redraw.max_with(c) } else { c })
            }
            _ if !self.inside(p) => c,
            Tool::Arrow | Tool::Line | Tool::Rect | Tool::Ellipse | Tool::Blur => {
                let shape = match self.tool {
                    Tool::Arrow => Shape::Arrow { a: p, b: p },
                    Tool::Line => Shape::Line { a: p, b: p },
                    Tool::Rect => Shape::Rect { a: p, b: p },
                    Tool::Ellipse => Shape::Ellipse { a: p, b: p },
                    _ => Shape::Blur { a: p, b: p, mode: self.blur_mode },
                };
                self.draft = Some(Item { shape, color: self.color, width: self.width() });
                self.drag = Drag::Create;
                self.selected = None;
                self.bump(Change::Redraw.max_with(c))
            }
            Tool::Pen | Tool::Highlighter => {
                let shape = if self.tool == Tool::Pen { Shape::Pen { pts: vec![p] } } else { Shape::Highlighter { pts: vec![p] } };
                self.draft = Some(Item { shape, color: self.color, width: self.width() });
                self.drag = Drag::Create;
                self.selected = None;
                self.bump(Change::Redraw.max_with(c))
            }
            Tool::Text => {
                let size = TEXT_SIZES[self.width_idx] * self.scale;
                self.selected = None;
                self.text = Some(TextEdit { idx: None, at: P::new(p.x, p.y - size * 0.6), text: String::new(), size, color: self.color, before: self.snap() });
                self.bump(Change::Redraw.max_with(c))
            }
            Tool::Step => {
                let before = self.snap();
                let r = STEP_RADII[self.width_idx] * self.scale;
                self.items.push(Item { shape: Shape::Step { c: p, r }, color: self.color, width: 0.0 });
                self.selected = None;
                self.push(before);
                self.bump(Change::Committed)
            }
            Tool::Crop => {
                self.crop_draft = Some((p, p));
                self.drag = Drag::Crop;
                self.bump(Change::Redraw.max_with(c))
            }
        }
    }

    pub fn mouse_move(&mut self, p: P, shift: bool) -> Change {
        match &mut self.drag {
            Drag::None => Change::None,
            Drag::Create => {
                let q = self.clamp(p);
                let Some(d) = &mut self.draft else { return Change::None };
                match &mut d.shape {
                    Shape::Arrow { a, b } | Shape::Line { a, b } => *b = if shift { snap45(*a, q) } else { q },
                    Shape::Rect { a, b } | Shape::Ellipse { a, b } => *b = if shift { square(*a, q) } else { q },
                    Shape::Blur { b, .. } => *b = q,
                    Shape::Pen { pts } | Shape::Highlighter { pts } => {
                        if pts.last().is_none_or(|l| l.dist(q) >= 1.5) {
                            pts.push(q);
                        }
                    }
                    _ => {}
                }
                self.bump(Change::Redraw)
            }
            Drag::Move { last, moved, .. } => {
                let (dx, dy) = (p.x - last.x, p.y - last.y);
                *last = p;
                *moved = true;
                if let Some(i) = self.selected {
                    self.items[i].translate(dx, dy);
                }
                self.bump(Change::Redraw)
            }
            Drag::Handle { h, moved, .. } => {
                let h = *h;
                *moved = true;
                let q = self.clamp(p);
                if let Some(i) = self.selected {
                    match &mut self.items[i].shape {
                        Shape::Arrow { a, b } | Shape::Line { a, b } => match h {
                            Handle::A => *a = q,
                            _ => *b = q,
                        },
                        Shape::Rect { a, b } | Shape::Ellipse { a, b } | Shape::Blur { a, b, .. } => {
                            let (l, t, r, bt) = norm(*a, *b);
                            let (na, nb) = match h {
                                Handle::Corner(0) => (q, P::new(r, bt)),
                                Handle::Corner(1) => (P::new(l, q.y), P::new(q.x, bt)),
                                Handle::Corner(2) => (P::new(l, t), q),
                                _ => (P::new(q.x, t), P::new(r, q.y)),
                            };
                            *a = na;
                            *b = nb;
                        }
                        _ => {}
                    }
                }
                self.bump(Change::Redraw)
            }
            Drag::Crop => {
                let q = self.clamp(p);
                if let Some((_, b)) = &mut self.crop_draft {
                    *b = q;
                }
                self.bump(Change::Redraw)
            }
        }
    }

    pub fn mouse_up(&mut self, p: P, shift: bool) -> Change {
        self.mouse_move(p, shift);
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::Create => {
                let Some(d) = self.draft.take() else { return Change::None };
                let keep = match &d.shape {
                    Shape::Arrow { a, b } | Shape::Line { a, b } => a.dist(*b) >= 4.0,
                    Shape::Rect { a, b } | Shape::Ellipse { a, b } | Shape::Blur { a, b, .. } => {
                        let (l, t, r, bt) = norm(*a, *b);
                        r - l >= 3.0 && bt - t >= 3.0
                    }
                    _ => true,
                };
                if !keep {
                    return self.bump(Change::Redraw);
                }
                let before = self.snap();
                self.items.push(d);
                self.push(before);
                self.bump(Change::Committed)
            }
            Drag::Move { before, moved, .. } | Drag::Handle { before, moved, .. } => {
                if moved {
                    self.push(before);
                    self.bump(Change::Committed)
                } else {
                    Change::None
                }
            }
            Drag::Crop => {
                // A tiny drag is a click: drop the draft.
                if let Some((a, b)) = self.crop_draft {
                    let r = to_rect(a, b);
                    if r.width() < 4 || r.height() < 4 {
                        self.crop_draft = None;
                    }
                }
                self.bump(Change::Redraw)
            }
            Drag::None => Change::None,
        }
    }

    /// Double-click: edit existing text in place.
    pub fn double_click(&mut self, p: P) -> Change {
        let Some(i) = self.hit(p) else { return Change::None };
        let before = self.snap();
        if let Shape::Text { at, text, size } = self.items[i].shape.clone() {
            let color = self.items[i].color;
            self.items.remove(i);
            self.selected = None;
            self.drag = Drag::None;
            self.text = Some(TextEdit { idx: Some(i), at, text, size, color, before });
            return self.bump(Change::Redraw);
        }
        Change::None
    }

    /// Commits the text being typed (or drops it when empty).
    pub fn finish_text(&mut self) -> Change {
        let Some(t) = self.text.take() else { return Change::None };
        if !t.text.trim().is_empty() {
            let item = Item { shape: Shape::Text { at: t.at, text: t.text, size: t.size }, color: t.color, width: 0.0 };
            match t.idx {
                Some(i) if i <= self.items.len() => self.items.insert(i, item),
                _ => self.items.push(item),
            }
        }
        self.push(t.before);
        self.bump(Change::Committed)
    }

    pub fn char(&mut self, ch: char) -> Change {
        let Some(t) = &mut self.text else { return Change::None };
        if ch.is_control() {
            return Change::None;
        }
        t.text.push(ch);
        self.bump(Change::Redraw)
    }

    /// Applies the crop draft.
    pub fn apply_crop(&mut self) -> Change {
        let Some((a, b)) = self.crop_draft.take() else { return Change::None };
        let area = self.crop.unwrap_or(self.bounds);
        let r = to_rect(a, b).intersect(&area);
        if r.width() < 4 || r.height() < 4 {
            return self.bump(Change::Redraw);
        }
        let before = self.snap();
        self.crop = Some(r);
        self.push(before);
        self.bump(Change::Committed)
    }

    pub fn key(&mut self, k: Key) -> Change {
        match k {
            Key::Escape => {
                if self.text.is_some() {
                    return self.finish_text();
                }
                if self.crop_draft.take().is_some() {
                    return self.bump(Change::Redraw);
                }
                if self.selected.take().is_some() {
                    return self.bump(Change::Redraw);
                }
                Change::None
            }
            Key::Enter => {
                if let Some(t) = &mut self.text {
                    t.text.push('\n');
                    return self.bump(Change::Redraw);
                }
                if self.crop_draft.is_some() {
                    return self.apply_crop();
                }
                Change::None
            }
            Key::Backspace => {
                let Some(t) = &mut self.text else { return Change::None };
                t.text.pop();
                self.bump(Change::Redraw)
            }
            Key::Delete => {
                if self.text.is_some() {
                    return Change::None;
                }
                let Some(i) = self.selected.take() else { return Change::None };
                let before = self.snap();
                self.items.remove(i);
                self.push(before);
                self.bump(Change::Committed)
            }
            Key::Undo | Key::Redo => {
                let _ = self.finish_text();
                self.draft = None;
                self.crop_draft = None;
                self.drag = Drag::None;
                let (from, to) = if k == Key::Undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
                let Some(s) = from.pop() else { return Change::None };
                to.push(Snap { items: self.items.clone(), crop: self.crop });
                self.items = s.items;
                self.crop = s.crop;
                self.selected = None;
                self.bump(Change::Committed)
            }
        }
    }

    /// Moves every item (the overlay frame was dragged; marks stay put on
    /// screen, so this is only used when the base image itself shifts).
    pub fn translate_all(&mut self, dx: f32, dy: f32) {
        for it in &mut self.items {
            it.translate(dx, dy);
        }
        self.version += 1;
    }
}

trait MaxWith {
    fn max_with(self, o: Change) -> Change;
}

impl MaxWith for Change {
    fn max_with(self, o: Change) -> Change {
        match (self, o) {
            (Change::Committed, _) | (_, Change::Committed) => Change::Committed,
            (Change::Redraw, _) | (_, Change::Redraw) => Change::Redraw,
            _ => Change::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Canvas {
        Canvas::new(Rect::new(0, 0, 400, 300), 1.0, None)
    }

    fn drag(c: &mut Canvas, a: (f32, f32), b: (f32, f32)) -> Change {
        c.mouse_down(P::new(a.0, a.1), false);
        c.mouse_move(P::new((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0), false);
        c.mouse_up(P::new(b.0, b.1), false)
    }

    #[test]
    fn draw_arrow_undo_redo() {
        let mut c = canvas();
        assert_eq!(drag(&mut c, (10.0, 10.0), (100.0, 50.0)), Change::Committed);
        assert_eq!(c.items.len(), 1);
        assert!(matches!(c.items[0].shape, Shape::Arrow { b, .. } if b == P::new(100.0, 50.0)));
        assert_eq!(c.key(Key::Undo), Change::Committed);
        assert!(c.items.is_empty());
        c.key(Key::Redo);
        assert_eq!(c.items.len(), 1);
        assert!(!c.can_redo());
    }

    #[test]
    fn tiny_drag_makes_nothing() {
        let mut c = canvas();
        c.set_tool(Tool::Rect);
        drag(&mut c, (10.0, 10.0), (11.0, 12.0));
        assert!(c.items.is_empty());
        assert!(!c.can_undo());
    }

    #[test]
    fn drawing_is_clamped_to_bounds() {
        let mut c = Canvas::new(Rect::new(100, 100, 200, 200), 1.0, None);
        c.set_tool(Tool::Line);
        assert_eq!(c.mouse_down(P::new(50.0, 50.0), false), Change::None);
        assert!(c.items.is_empty() && c.draft.is_none(), "press outside does nothing");
        drag(&mut c, (150.0, 150.0), (500.0, 150.0));
        assert!(matches!(c.items[0].shape, Shape::Line { b, .. } if b == P::new(200.0, 150.0)));
    }

    #[test]
    fn shift_snaps() {
        let mut c = canvas();
        c.mouse_down(P::new(0.0, 0.0), true);
        c.mouse_up(P::new(100.0, 10.0), true);
        let Shape::Arrow { b, .. } = c.items[0].shape else { panic!() };
        assert!((b.y).abs() < 0.01 && (b.x - 100.5).abs() < 1.0);
        c.set_tool(Tool::Rect);
        c.mouse_down(P::new(10.0, 10.0), true);
        c.mouse_up(P::new(60.0, 30.0), true);
        let Shape::Rect { b, .. } = c.items[1].shape else { panic!() };
        assert_eq!(b, P::new(60.0, 60.0));
    }

    #[test]
    fn select_move_resize_delete() {
        let mut c = canvas();
        c.set_tool(Tool::Rect);
        drag(&mut c, (50.0, 50.0), (150.0, 100.0));
        c.set_tool(Tool::Select);
        // Click on the outline, drag by (10, 20).
        c.mouse_down(P::new(50.0, 75.0), false);
        assert_eq!(c.selected, Some(0));
        c.mouse_up(P::new(60.0, 95.0), false);
        assert!(matches!(c.items[0].shape, Shape::Rect { a, .. } if a == P::new(60.0, 70.0)));
        // Bottom-right handle.
        c.mouse_down(P::new(160.0, 120.0), false);
        c.mouse_up(P::new(200.0, 150.0), false);
        assert!(matches!(c.items[0].shape, Shape::Rect { b, .. } if b == P::new(200.0, 150.0)));
        c.key(Key::Undo);
        assert!(matches!(c.items[0].shape, Shape::Rect { b, .. } if b == P::new(160.0, 120.0)));
        c.set_tool(Tool::Select);
        c.mouse_down(P::new(60.0, 90.0), false);
        c.mouse_up(P::new(60.0, 90.0), false);
        assert_eq!(c.key(Key::Delete), Change::Committed);
        assert!(c.items.is_empty());
    }

    #[test]
    fn click_inside_rect_does_not_select_it() {
        let mut c = canvas();
        c.set_tool(Tool::Rect);
        drag(&mut c, (50.0, 50.0), (250.0, 250.0));
        assert_eq!(c.hit(P::new(150.0, 150.0)), None);
        assert_eq!(c.hit(P::new(50.0, 150.0)), Some(0));
    }

    #[test]
    fn topmost_wins() {
        let mut c = canvas();
        c.set_tool(Tool::Step);
        c.mouse_down(P::new(100.0, 100.0), false);
        c.mouse_down(P::new(105.0, 100.0), false);
        assert_eq!(c.items.len(), 2);
        assert_eq!(c.hit(P::new(103.0, 100.0)), Some(1));
    }

    #[test]
    fn steps_and_recolor() {
        let mut c = canvas();
        c.set_tool(Tool::Step);
        for x in [50.0, 100.0, 150.0] {
            c.mouse_down(P::new(x, 50.0), false);
        }
        assert_eq!(c.items.len(), 3);
        c.set_tool(Tool::Select);
        c.mouse_down(P::new(100.0, 50.0), false);
        c.mouse_up(P::new(100.0, 50.0), false);
        assert_eq!(c.set_color(PALETTE[4]), Change::Committed);
        assert_eq!(c.items[1].color, PALETTE[4]);
        assert_eq!(c.items[0].color, PALETTE[0]);
    }

    #[test]
    fn text_typing_commit_and_reedit() {
        let mut c = canvas();
        c.set_tool(Tool::Text);
        c.mouse_down(P::new(50.0, 50.0), false);
        for ch in "Hi".chars() {
            c.char(ch);
        }
        c.key(Key::Enter);
        c.char('!');
        assert!(c.items.is_empty(), "not committed while typing");
        assert_eq!(c.preview().len(), 1);
        assert_eq!(c.key(Key::Escape), Change::Committed);
        assert!(matches!(&c.items[0].shape, Shape::Text { text, .. } if text == "Hi\n!"));
        // Double-click to edit again, erase all: text disappears, undo restores.
        let at = match &c.items[0].shape {
            Shape::Text { at, .. } => *at,
            _ => unreachable!(),
        };
        c.double_click(P::new(at.x + 5.0, at.y + 5.0));
        assert!(c.text.is_some());
        for _ in 0..4 {
            c.key(Key::Backspace);
        }
        c.finish_text();
        assert!(c.items.is_empty());
        c.key(Key::Undo);
        assert_eq!(c.items.len(), 1);
    }

    #[test]
    fn empty_text_leaves_nothing() {
        let mut c = canvas();
        c.set_tool(Tool::Text);
        c.mouse_down(P::new(50.0, 50.0), false);
        c.key(Key::Escape);
        assert!(c.items.is_empty());
        assert!(!c.can_undo());
    }

    #[test]
    fn crop_apply_and_undo() {
        let mut c = canvas();
        c.set_tool(Tool::Crop);
        drag(&mut c, (20.0, 30.0), (220.0, 130.0));
        assert!(c.crop.is_none());
        assert_eq!(c.key(Key::Enter), Change::Committed);
        assert_eq!(c.crop, Some(Rect::new(20, 30, 220, 130)));
        c.key(Key::Undo);
        assert_eq!(c.crop, None);
        // Escape drops a draft.
        drag(&mut c, (20.0, 30.0), (220.0, 130.0));
        c.key(Key::Escape);
        assert!(c.crop_draft.is_none());
    }

    #[test]
    fn pen_collects_points() {
        let mut c = canvas();
        c.set_tool(Tool::Pen);
        c.mouse_down(P::new(10.0, 10.0), false);
        for i in 1..20 {
            c.mouse_move(P::new(10.0 + i as f32 * 3.0, 10.0), false);
        }
        c.mouse_up(P::new(70.0, 10.0), false);
        assert!(matches!(&c.items[0].shape, Shape::Pen { pts } if pts.len() >= 19));
    }
}

//! Selection state machine for the capture overlay. Pure logic, no OS calls,
//! so it is unit tested on every platform.

use crate::geom::{Point, Rect};

/// Mouse travel (px) below which a press + release counts as a click.
pub const CLICK_SLOP: i32 = 3;
/// Elements smaller than this on either side are ignored.
pub const MIN_ELEMENT: i32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Redraw,
    Confirm(Rect),
    Cancel,
}

/// Which edges a resize drag moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Edges {
    pub left: bool,
    pub top: bool,
    pub right: bool,
    pub bottom: bool,
}

impl Edges {
    pub fn any(&self) -> bool {
        self.left || self.top || self.right || self.bottom
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditOp {
    Move { start: Point, from: Rect },
    Resize { edges: Edges, start: Point, from: Rect },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Hover,
    Pressed { anchor: Point },
    Dragging { anchor: Point, pos: Point },
    Editing { rect: Rect, op: Option<EditOp> },
}

/// What the overlay should draw right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    /// The bright area (drag frame, edited frame or hovered region).
    pub frame: Option<Rect>,
    /// True while the frame is a hover suggestion rather than a real selection.
    pub suggestion: bool,
    /// True while the frame can be moved and resized.
    pub editing: bool,
}

fn near(a: Rect, b: Rect) -> bool {
    (a.left - b.left).abs() <= 2 && (a.top - b.top).abs() <= 2 && (a.right - b.right).abs() <= 2 && (a.bottom - b.bottom).abs() <= 2
}

pub struct Selection {
    bounds: Rect,
    /// Top-level windows, topmost first, clipped to bounds.
    windows: Vec<Rect>,
    monitors: Vec<Rect>,
    /// Latest element answer and the window it belongs to.
    elements: Vec<Rect>,
    elements_window: Option<Rect>,
    /// Chosen level counted from the largest (0 = largest). None = smallest.
    up: Option<usize>,
    state: State,
    pos: Point,
    handle_tol: i32,
}

impl Selection {
    pub fn new(bounds: Rect, windows: Vec<Rect>, monitors: Vec<Rect>, handle_tol: i32) -> Self {
        let clip = |v: Vec<Rect>| v.into_iter().map(|r| r.intersect(&bounds)).filter(|r| !r.is_empty()).collect();
        Self {
            bounds,
            windows: clip(windows),
            monitors: clip(monitors),
            elements: Vec::new(),
            elements_window: None,
            up: None,
            state: State::Hover,
            pos: Point::default(),
            handle_tol: handle_tol.max(2),
        }
    }

    pub fn pos(&self) -> Point {
        self.pos
    }

    /// Index of the topmost window under the point.
    pub fn window_index(&self, p: Point) -> Option<usize> {
        self.windows.iter().position(|r| r.contains(p))
    }

    pub fn is_editing(&self) -> bool {
        matches!(self.state, State::Editing { .. })
    }

    pub fn is_hovering(&self) -> bool {
        matches!(self.state, State::Hover)
    }

    /// Levels under the point, smallest first.
    pub fn chain(&self, p: Point) -> Vec<Rect> {
        let mut v = Vec::new();
        if let Some(w) = self.windows.iter().copied().find(|r| r.contains(p)) {
            if self.elements_window == Some(w) {
                for e in &self.elements {
                    let e = e.intersect(&w);
                    if e.contains(p) && e.width() >= MIN_ELEMENT && e.height() >= MIN_ELEMENT && e.area() < w.area() {
                        v.push(e);
                    }
                }
            }
            v.push(w);
        }
        if let Some(m) = self.monitors.iter().copied().find(|r| r.contains(p)) {
            v.push(m);
        }
        v.sort_by_key(|r| r.area());
        let mut out: Vec<Rect> = Vec::new();
        for r in v {
            match out.last_mut() {
                Some(last) if near(*last, r) => *last = r,
                _ => out.push(r),
            }
        }
        out
    }

    fn level_index(&self, len: usize) -> usize {
        match self.up {
            None => 0,
            Some(k) => len - 1 - k.min(len - 1),
        }
    }

    fn suggest(&self, p: Point) -> Option<Rect> {
        let c = self.chain(p);
        if c.is_empty() { None } else { Some(c[self.level_index(c.len())]) }
    }

    pub fn view(&self) -> View {
        match self.state {
            State::Hover | State::Pressed { .. } => View { frame: self.suggest(self.pos), suggestion: true, editing: false },
            State::Dragging { anchor, pos } => {
                let r = self.drag_rect(anchor, pos);
                View { frame: (!r.is_empty()).then_some(r), suggestion: false, editing: false }
            }
            State::Editing { rect, .. } => View { frame: Some(rect), suggestion: false, editing: true },
        }
    }

    /// New element answer for `window` (rects in any order).
    pub fn set_elements(&mut self, window: Rect, rects: Vec<Rect>) -> Outcome {
        let before = self.view();
        self.elements_window = Some(window.intersect(&self.bounds));
        self.elements = rects;
        if self.view() != before { Outcome::Redraw } else { Outcome::Nothing }
    }

    /// Mouse wheel while hovering: bigger = one level up.
    pub fn wheel(&mut self, bigger: bool) -> Outcome {
        if !self.is_hovering() {
            return Outcome::Nothing;
        }
        let c = self.chain(self.pos);
        if c.len() < 2 {
            return Outcome::Nothing;
        }
        let from_top = c.len() - 1 - self.level_index(c.len());
        let new_up = if bigger {
            if from_top == 0 {
                return Outcome::Nothing;
            }
            Some(from_top - 1)
        } else if from_top + 1 >= c.len() - 1 {
            None
        } else {
            Some(from_top + 1)
        };
        if new_up == self.up {
            return Outcome::Nothing;
        }
        self.up = new_up;
        Outcome::Redraw
    }

    /// Edges under the point while editing (for cursor shapes). `None` = outside the frame.
    pub fn hit(&self, p: Point) -> Option<Edges> {
        match self.state {
            State::Editing { rect, .. } => self.hit_rect(rect, p),
            _ => None,
        }
    }

    fn drag_rect(&self, anchor: Point, pos: Point) -> Rect {
        Rect::from_points(anchor, pos).intersect(&self.bounds)
    }

    fn hit_rect(&self, r: Rect, p: Point) -> Option<Edges> {
        let t = self.handle_tol;
        let outer = Rect::new(r.left - t, r.top - t, r.right + t, r.bottom + t);
        if !outer.contains(p) {
            return None;
        }
        Some(Edges {
            left: (p.x - r.left).abs() <= t,
            top: (p.y - r.top).abs() <= t,
            right: (p.x - r.right).abs() <= t,
            bottom: (p.y - r.bottom).abs() <= t,
        })
    }

    pub fn mouse_down(&mut self, p: Point) -> Outcome {
        self.pos = p;
        if let State::Editing { rect, .. } = self.state {
            match self.hit_rect(rect, p) {
                Some(edges) if edges.any() => {
                    self.state = State::Editing { rect, op: Some(EditOp::Resize { edges, start: p, from: rect }) };
                    return Outcome::Nothing;
                }
                Some(_) => {
                    self.state = State::Editing { rect, op: Some(EditOp::Move { start: p, from: rect }) };
                    return Outcome::Nothing;
                }
                None => {}
            }
        }
        self.state = State::Pressed { anchor: p };
        Outcome::Redraw
    }

    pub fn mouse_move(&mut self, p: Point) -> Outcome {
        let prev_view = self.view();
        self.pos = p;
        match self.state {
            State::Hover => {}
            State::Pressed { anchor } => {
                if (p.x - anchor.x).abs() > CLICK_SLOP || (p.y - anchor.y).abs() > CLICK_SLOP {
                    self.state = State::Dragging { anchor, pos: p };
                }
            }
            State::Dragging { anchor, .. } => self.state = State::Dragging { anchor, pos: p },
            State::Editing { op: Some(op), .. } => {
                let rect = self.apply(op, p);
                self.state = State::Editing { rect, op: Some(op) };
            }
            State::Editing { op: None, .. } => {}
        }
        if self.view() != prev_view { Outcome::Redraw } else { Outcome::Nothing }
    }

    fn apply(&self, op: EditOp, p: Point) -> Rect {
        match op {
            EditOp::Move { start, from } => from.offset(p.x - start.x, p.y - start.y).clamp_inside(&self.bounds),
            EditOp::Resize { edges, start, from } => {
                let (dx, dy) = (p.x - start.x, p.y - start.y);
                let mut r = from;
                if edges.left {
                    r.left += dx;
                }
                if edges.right {
                    r.right += dx;
                }
                if edges.top {
                    r.top += dy;
                }
                if edges.bottom {
                    r.bottom += dy;
                }
                let r = Rect::from_points(Point::new(r.left, r.top), Point::new(r.right, r.bottom));
                let r = r.intersect(&self.bounds);
                if r.is_empty() { from } else { r }
            }
        }
    }

    /// `keep_open`: Ctrl held on release - keep the frame for editing instead of confirming.
    pub fn mouse_up(&mut self, p: Point, keep_open: bool) -> Outcome {
        self.pos = p;
        match self.state {
            State::Hover => Outcome::Nothing,
            State::Pressed { anchor } => match self.suggest(anchor) {
                Some(r) if keep_open => {
                    self.state = State::Editing { rect: r, op: None };
                    Outcome::Redraw
                }
                Some(r) => Outcome::Confirm(r),
                None => {
                    self.state = State::Hover;
                    Outcome::Redraw
                }
            },
            State::Dragging { anchor, .. } => {
                let r = self.drag_rect(anchor, p);
                if r.is_empty() {
                    self.state = State::Hover;
                    Outcome::Redraw
                } else if keep_open {
                    self.state = State::Editing { rect: r, op: None };
                    Outcome::Redraw
                } else {
                    Outcome::Confirm(r)
                }
            }
            State::Editing { rect, .. } => {
                self.state = State::Editing { rect, op: None };
                Outcome::Nothing
            }
        }
    }

    pub fn double_click(&mut self, p: Point) -> Outcome {
        match self.state {
            State::Editing { rect, .. } if rect.contains(p) => Outcome::Confirm(rect),
            _ => Outcome::Nothing,
        }
    }

    pub fn key(&mut self, key: Key, shift: bool) -> Outcome {
        if key == Key::Escape {
            return Outcome::Cancel;
        }
        let State::Editing { rect, op: None } = self.state else {
            return Outcome::Nothing;
        };
        let step = if shift { 10 } else { 1 };
        let (dx, dy) = match key {
            Key::Enter => return Outcome::Confirm(rect),
            Key::Left => (-step, 0),
            Key::Right => (step, 0),
            Key::Up => (0, -step),
            Key::Down => (0, step),
            Key::Escape => unreachable!(),
        };
        self.state = State::Editing { rect: rect.offset(dx, dy).clamp_inside(&self.bounds), op: None };
        Outcome::Redraw
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: Rect = Rect::new(100, 100, 300, 250);
    const BIG: Rect = Rect::new(50, 50, 900, 700);
    const MON: Rect = Rect::new(0, 0, 1000, 800);

    fn sel() -> Selection {
        // Topmost first: a small window over a big one.
        Selection::new(MON, vec![SMALL, BIG], vec![Rect::new(-100, 0, 1000, 800)], 5)
    }

    #[test]
    fn hover_suggests_topmost_window() {
        let mut s = sel();
        s.mouse_move(Point::new(150, 150));
        assert_eq!(s.view(), View { frame: Some(SMALL), suggestion: true, editing: false });
        s.mouse_move(Point::new(600, 600));
        assert_eq!(s.view().frame, Some(BIG));
        // Monitors are clipped to bounds; bare desktop offers the monitor.
        s.mouse_move(Point::new(10, 10));
        assert_eq!(s.view().frame, Some(MON));
    }

    #[test]
    fn click_confirms_window() {
        let mut s = sel();
        s.mouse_move(Point::new(150, 150));
        s.mouse_down(Point::new(150, 150));
        s.mouse_move(Point::new(152, 151));
        assert_eq!(s.mouse_up(Point::new(152, 151), false), Outcome::Confirm(SMALL));
    }

    #[test]
    fn elements_refine_and_wheel_walks_levels() {
        let mut s = sel();
        let button = Rect::new(500, 400, 560, 430);
        let panel = Rect::new(450, 300, 800, 600);
        let p = Point::new(520, 410);
        s.mouse_move(p);
        assert_eq!(s.view().frame, Some(BIG));
        // Unordered, with a near-duplicate, a tiny one and one outside the point.
        let out = s.set_elements(
            BIG,
            vec![panel, button, Rect::new(501, 401, 559, 429), Rect::new(510, 405, 515, 409), Rect::new(60, 60, 100, 100)],
        );
        assert_eq!(out, Outcome::Redraw);
        assert_eq!(s.chain(p), vec![button, panel, BIG, MON]);
        assert_eq!(s.view().frame, Some(button));
        assert_eq!(s.wheel(true), Outcome::Redraw);
        assert_eq!(s.view().frame, Some(panel));
        s.wheel(true);
        assert_eq!(s.view().frame, Some(BIG));
        s.wheel(true);
        assert_eq!(s.view().frame, Some(MON));
        assert_eq!(s.wheel(true), Outcome::Nothing);
        s.wheel(false);
        s.wheel(false);
        assert_eq!(s.view().frame, Some(panel));
        s.wheel(false);
        assert_eq!(s.view().frame, Some(button));
        assert_eq!(s.wheel(false), Outcome::Nothing);
    }

    #[test]
    fn window_level_sticks_while_moving() {
        let mut s = sel();
        s.set_elements(BIG, vec![Rect::new(500, 400, 560, 430), Rect::new(450, 300, 800, 600)]);
        s.mouse_move(Point::new(520, 410));
        s.wheel(true);
        s.wheel(true); // window level (one below monitor)
        assert_eq!(s.view().frame, Some(BIG));
        s.mouse_move(Point::new(150, 150));
        assert_eq!(s.view().frame, Some(SMALL));
    }

    #[test]
    fn elements_for_another_window_are_ignored() {
        let mut s = sel();
        s.set_elements(SMALL, vec![Rect::new(120, 120, 200, 200)]);
        s.mouse_move(Point::new(600, 600));
        assert_eq!(s.chain(Point::new(600, 600)), vec![BIG, MON]);
        s.mouse_move(Point::new(150, 150));
        assert_eq!(s.view().frame, Some(Rect::new(120, 120, 200, 200)));
    }

    #[test]
    fn click_uses_current_level() {
        let mut s = sel();
        s.set_elements(BIG, vec![Rect::new(500, 400, 560, 430)]);
        let p = Point::new(520, 410);
        s.mouse_move(p);
        s.wheel(true);
        s.mouse_down(p);
        assert_eq!(s.mouse_up(p, false), Outcome::Confirm(BIG));
    }

    #[test]
    fn drag_confirms_frame() {
        let mut s = sel();
        s.mouse_down(Point::new(400, 300));
        s.mouse_move(Point::new(500, 350));
        assert_eq!(s.view().frame, Some(Rect::new(400, 300, 500, 350)));
        assert!(!s.view().suggestion);
        assert_eq!(s.wheel(true), Outcome::Nothing);
        assert_eq!(s.mouse_up(Point::new(600, 450), false), Outcome::Confirm(Rect::new(400, 300, 600, 450)));
    }

    #[test]
    fn drag_backwards_and_clipped() {
        let mut s = sel();
        s.mouse_down(Point::new(100, 100));
        s.mouse_move(Point::new(-50, -50));
        assert_eq!(s.mouse_up(Point::new(-50, -50), false), Outcome::Confirm(Rect::new(0, 0, 100, 100)));
    }

    #[test]
    fn ctrl_release_then_edit_and_enter() {
        let mut s = sel();
        s.mouse_down(Point::new(400, 300));
        s.mouse_move(Point::new(500, 400));
        assert_eq!(s.mouse_up(Point::new(500, 400), true), Outcome::Redraw);
        s.mouse_down(Point::new(450, 350));
        s.mouse_move(Point::new(460, 360));
        s.mouse_up(Point::new(460, 360), false);
        assert_eq!(s.view().frame, Some(Rect::new(410, 310, 510, 410)));
        s.mouse_down(Point::new(510, 410));
        s.mouse_move(Point::new(530, 440));
        s.mouse_up(Point::new(530, 440), false);
        assert_eq!(s.view().frame, Some(Rect::new(410, 310, 530, 440)));
        s.key(Key::Right, false);
        s.key(Key::Down, true);
        assert_eq!(s.key(Key::Enter, false), Outcome::Confirm(Rect::new(411, 320, 531, 450)));
    }

    #[test]
    fn resize_past_opposite_edge_flips() {
        let mut s = sel();
        s.mouse_down(Point::new(400, 300));
        s.mouse_move(Point::new(500, 400));
        s.mouse_up(Point::new(500, 400), true);
        s.mouse_down(Point::new(400, 350));
        s.mouse_move(Point::new(560, 350));
        assert_eq!(s.view().frame, Some(Rect::new(500, 300, 560, 400)));
    }

    #[test]
    fn click_outside_edited_frame_starts_new_drag() {
        let mut s = sel();
        s.mouse_down(Point::new(400, 300));
        s.mouse_move(Point::new(500, 400));
        s.mouse_up(Point::new(500, 400), true);
        s.mouse_down(Point::new(700, 600));
        s.mouse_move(Point::new(800, 650));
        assert_eq!(s.mouse_up(Point::new(800, 650), false), Outcome::Confirm(Rect::new(700, 600, 800, 650)));
    }

    #[test]
    fn escape_cancels_anywhere() {
        let mut s = sel();
        assert_eq!(s.key(Key::Escape, false), Outcome::Cancel);
        s.mouse_down(Point::new(1, 1));
        assert_eq!(s.key(Key::Escape, false), Outcome::Cancel);
    }

    #[test]
    fn tiny_drag_with_no_window_returns_to_hover() {
        let mut s = Selection::new(Rect::new(0, 0, 100, 100), vec![], vec![], 5);
        s.mouse_down(Point::new(10, 10));
        assert_eq!(s.mouse_up(Point::new(11, 11), false), Outcome::Redraw);
        assert_eq!(s.view().frame, None);
    }
}

//! Selection state machine for the capture overlay. Pure logic, no OS calls,
//! so it is unit tested on every platform.

use crate::geom::{Point, Rect};

/// Mouse travel (px) below which a press + release counts as a click.
pub const CLICK_SLOP: i32 = 3;

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
    /// The bright area (drag frame, edited frame or hovered window).
    pub frame: Option<Rect>,
    /// True while the frame is a hover suggestion rather than a real selection.
    pub suggestion: bool,
    /// True while the frame can be moved and resized.
    pub editing: bool,
}

pub struct Selection {
    bounds: Rect,
    candidates: Vec<Rect>,
    state: State,
    pos: Point,
    handle_tol: i32,
}

impl Selection {
    /// `candidates` are window / region rects in z-order, topmost first.
    /// They are clipped to `bounds`; empty ones are dropped.
    pub fn new(bounds: Rect, candidates: Vec<Rect>, handle_tol: i32) -> Self {
        let candidates = candidates
            .into_iter()
            .map(|r| r.intersect(&bounds))
            .filter(|r| !r.is_empty())
            .collect();
        Self { bounds, candidates, state: State::Hover, pos: Point::default(), handle_tol: handle_tol.max(2) }
    }

    pub fn pos(&self) -> Point {
        self.pos
    }

    pub fn pick(&self, p: Point) -> Option<Rect> {
        self.candidates.iter().copied().find(|r| r.contains(p))
    }

    pub fn view(&self) -> View {
        match self.state {
            State::Hover | State::Pressed { .. } => {
                View { frame: self.pick(self.pos), suggestion: true, editing: false }
            }
            State::Dragging { anchor, pos } => {
                let r = self.drag_rect(anchor, pos);
                View { frame: (!r.is_empty()).then_some(r), suggestion: false, editing: false }
            }
            State::Editing { rect, .. } => View { frame: Some(rect), suggestion: false, editing: true },
        }
    }

    /// Edges under the point while editing (for cursor shapes). `None` = outside the frame.
    pub fn hit(&self, p: Point) -> Option<Edges> {
        match self.state {
            State::Editing { rect, .. } => self.hit_rect(rect, p),
            _ => None,
        }
    }

    pub fn is_editing(&self) -> bool {
        matches!(self.state, State::Editing { .. })
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
            State::Pressed { anchor } => match self.pick(anchor) {
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

    fn sel() -> Selection {
        let bounds = Rect::new(0, 0, 1000, 800);
        // Topmost first: a small window over a big one.
        Selection::new(bounds, vec![Rect::new(100, 100, 300, 250), Rect::new(50, 50, 900, 700), Rect::new(-100, 0, 1000, 800)], 5)
    }

    #[test]
    fn hover_suggests_topmost_window() {
        let mut s = sel();
        s.mouse_move(Point::new(150, 150));
        assert_eq!(s.view(), View { frame: Some(Rect::new(100, 100, 300, 250)), suggestion: true, editing: false });
        s.mouse_move(Point::new(600, 600));
        assert_eq!(s.view().frame, Some(Rect::new(50, 50, 900, 700)));
        // Candidates are clipped to bounds.
        s.mouse_move(Point::new(10, 10));
        assert_eq!(s.view().frame, Some(Rect::new(0, 0, 1000, 800)));
    }

    #[test]
    fn click_confirms_window() {
        let mut s = sel();
        s.mouse_move(Point::new(150, 150));
        s.mouse_down(Point::new(150, 150));
        s.mouse_move(Point::new(152, 151));
        assert_eq!(s.mouse_up(Point::new(152, 151), false), Outcome::Confirm(Rect::new(100, 100, 300, 250)));
    }

    #[test]
    fn drag_confirms_frame() {
        let mut s = sel();
        s.mouse_down(Point::new(400, 300));
        s.mouse_move(Point::new(500, 350));
        assert_eq!(s.view().frame, Some(Rect::new(400, 300, 500, 350)));
        assert!(!s.view().suggestion);
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
        assert!(s.is_editing());
        // Move by dragging inside.
        s.mouse_down(Point::new(450, 350));
        s.mouse_move(Point::new(460, 360));
        s.mouse_up(Point::new(460, 360), false);
        assert_eq!(s.view().frame, Some(Rect::new(410, 310, 510, 410)));
        // Resize the bottom-right corner.
        s.mouse_down(Point::new(510, 410));
        s.mouse_move(Point::new(530, 440));
        s.mouse_up(Point::new(530, 440), false);
        assert_eq!(s.view().frame, Some(Rect::new(410, 310, 530, 440)));
        // Nudge with arrows.
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
        s.mouse_down(Point::new(400, 350)); // left edge
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
        let mut s = Selection::new(Rect::new(0, 0, 100, 100), vec![], 5);
        s.mouse_down(Point::new(10, 10));
        assert_eq!(s.mouse_up(Point::new(11, 11), false), Outcome::Redraw);
        assert_eq!(s.view().frame, None);
    }
}

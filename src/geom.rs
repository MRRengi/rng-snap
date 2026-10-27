//! Plain geometry in physical virtual-desktop pixels. Rects are half-open:
//! `left..right`, `top..bottom`.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self { left, top, right, bottom }
    }

    pub fn from_xywh(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self::new(x, y, x + w, y + h)
    }

    /// Normalized rect spanning two corner points.
    pub fn from_points(a: Point, b: Point) -> Self {
        Self::new(a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
    }

    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn is_empty(&self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.left && p.x < self.right && p.y >= self.top && p.y < self.bottom
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let r = Rect::new(
            self.left.max(o.left),
            self.top.max(o.top),
            self.right.min(o.right),
            self.bottom.min(o.bottom),
        );
        if r.is_empty() { Rect::default() } else { r }
    }

    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.left + dx, self.top + dy, self.right + dx, self.bottom + dy)
    }

    /// Moves the rect inside `bounds` without resizing (as far as it fits).
    pub fn clamp_inside(&self, bounds: &Rect) -> Rect {
        let mut dx = 0;
        let mut dy = 0;
        if self.left < bounds.left {
            dx = bounds.left - self.left;
        } else if self.right > bounds.right {
            dx = bounds.right - self.right;
        }
        if self.top < bounds.top {
            dy = bounds.top - self.top;
        } else if self.bottom > bounds.bottom {
            dy = bounds.bottom - self.bottom;
        }
        self.offset(dx, dy)
    }

    pub fn area(&self) -> i64 {
        if self.is_empty() { 0 } else { self.width() as i64 * self.height() as i64 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_points_normalizes() {
        let r = Rect::from_points(Point::new(50, 40), Point::new(10, 90));
        assert_eq!(r, Rect::new(10, 40, 50, 90));
        assert_eq!((r.width(), r.height()), (40, 50));
    }

    #[test]
    fn contains_is_half_open() {
        let r = Rect::new(0, 0, 10, 10);
        assert!(r.contains(Point::new(0, 0)));
        assert!(r.contains(Point::new(9, 9)));
        assert!(!r.contains(Point::new(10, 5)));
    }

    #[test]
    fn intersect_and_clamp() {
        let a = Rect::new(0, 0, 100, 100);
        assert_eq!(a.intersect(&Rect::new(50, 50, 150, 150)), Rect::new(50, 50, 100, 100));
        assert!(a.intersect(&Rect::new(200, 200, 300, 300)).is_empty());
        assert_eq!(Rect::new(-10, 90, 20, 120).clamp_inside(&a), Rect::new(0, 70, 30, 100));
    }
}

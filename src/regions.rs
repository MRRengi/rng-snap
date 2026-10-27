//! Pixel rectangle finder for apps that expose no UI elements (games, canvas,
//! custom-drawn UIs). Looks for closed boxes of straight edges around a point
//! in the frozen frame.

use crate::geom::{Point, Rect};
use crate::image::Frame;

/// Sum of |dB|+|dG|+|dR| above which two neighbouring pixels form an edge.
const EDGE: i32 = 30;
/// How many candidate lines to collect in each direction.
const PER_SIDE: usize = 24;
/// Fraction of a side that must be edge pixels.
const COVER: f32 = 0.9;
const MAX_RESULTS: usize = 6;

/// Prefix sums of edge pixels inside `area` (desktop coords).
pub struct EdgeMap {
    area: Rect,
    w: usize,
    h: usize,
    /// Row-major, (w + 1) per row: boundary between row y and y + 1, prefix over x.
    hpre: Vec<u16>,
    /// Column-major, (h + 1) per column: boundary between column x and x + 1, prefix over y.
    vpre: Vec<u16>,
}

fn differs(a: &[u8], b: &[u8]) -> bool {
    (a[0] as i32 - b[0] as i32).abs() + (a[1] as i32 - b[1] as i32).abs() + (a[2] as i32 - b[2] as i32).abs() > EDGE
}

impl EdgeMap {
    pub fn build(frame: &Frame, area: Rect) -> EdgeMap {
        let area = area.intersect(&frame.rect);
        let (w, h) = (area.width().max(0) as usize, area.height().max(0) as usize);
        let mut hpre = vec![0u16; (w + 1) * h];
        let mut vpre = vec![0u16; (h + 1) * w];
        let stride = frame.stride();
        let x0 = (area.left - frame.rect.left) as usize;
        let y0 = (area.top - frame.rect.top) as usize;
        let px = |x: usize, y: usize| {
            let i = (y0 + y) * stride + (x0 + x) * 4;
            &frame.pixels[i..i + 3]
        };
        for y in 0..h {
            let row = y * (w + 1);
            for x in 0..w {
                let here = px(x, y);
                let down = y + 1 < h && differs(here, px(x, y + 1));
                hpre[row + x + 1] = hpre[row + x] + down as u16;
                let right = x + 1 < w && differs(here, px(x + 1, y));
                let col = x * (h + 1);
                vpre[col + y + 1] = vpre[col + y] + right as u16;
            }
        }
        EdgeMap { area, w, h, hpre, vpre }
    }

    pub fn area(&self) -> Rect {
        self.area
    }

    /// Edge pixels on the boundary below row `y`, columns `x0..x1`.
    fn hcount(&self, y: i32, x0: i32, x1: i32) -> i32 {
        let (x0, x1) = (x0.clamp(0, self.w as i32) as usize, x1.clamp(0, self.w as i32) as usize);
        if x1 <= x0 || y < 0 || y as usize >= self.h {
            return 0;
        }
        let row = y as usize * (self.w + 1);
        (self.hpre[row + x1] - self.hpre[row + x0]) as i32
    }

    /// Edge pixels on the boundary right of column `x`, rows `y0..y1`.
    fn vcount(&self, x: i32, y0: i32, y1: i32) -> i32 {
        let (y0, y1) = (y0.clamp(0, self.h as i32) as usize, y1.clamp(0, self.h as i32) as usize);
        if y1 <= y0 || x < 0 || x as usize >= self.w {
            return 0;
        }
        let col = x as usize * (self.h + 1);
        (self.vpre[col + y1] - self.vpre[col + y0]) as i32
    }
}

/// True if a straight edge run of 8 px passes through position `c` on this line.
fn passes(count: impl Fn(i32, i32) -> i32, c: i32, len: i32) -> bool {
    [(c - 7, c + 1), (c - 4, c + 4), (c, c + 8)]
        .iter()
        .any(|&(a, b)| a >= 0 && b <= len && count(a, b) >= 7)
}

fn near(a: Rect, b: Rect) -> bool {
    (a.left - b.left).abs() <= 2 && (a.top - b.top).abs() <= 2 && (a.right - b.right).abs() <= 2 && (a.bottom - b.bottom).abs() <= 2
}

/// Boxes around `p` (desktop coords), smallest first, excluding the whole area.
pub fn find(map: &EdgeMap, p: Point, min_side: i32) -> Vec<Rect> {
    let (w, h) = (map.w as i32, map.h as i32);
    let lx = p.x - map.area.left;
    let ly = p.y - map.area.top;
    if lx < 0 || ly < 0 || lx >= w || ly >= h {
        return Vec::new();
    }

    // Boundaries are stored as the row/column just before the box side;
    // -1 / h-1 / w-1 stand for the area's own edges.
    let mut tops = Vec::new();
    for y in (0..ly).rev() {
        if passes(|a, b| map.hcount(y, a, b), lx, w) {
            tops.push(y);
            if tops.len() == PER_SIDE {
                break;
            }
        }
    }
    tops.push(-1);
    let mut bottoms = Vec::new();
    for y in ly..h - 1 {
        if passes(|a, b| map.hcount(y, a, b), lx, w) {
            bottoms.push(y);
            if bottoms.len() == PER_SIDE {
                break;
            }
        }
    }
    bottoms.push(h - 1);
    let mut lefts = Vec::new();
    for x in (0..lx).rev() {
        if passes(|a, b| map.vcount(x, a, b), ly, h) {
            lefts.push(x);
            if lefts.len() == PER_SIDE {
                break;
            }
        }
    }
    lefts.push(-1);
    let mut rights = Vec::new();
    for x in lx..w - 1 {
        if passes(|a, b| map.vcount(x, a, b), ly, h) {
            rights.push(x);
            if rights.len() == PER_SIDE {
                break;
            }
        }
    }
    rights.push(w - 1);

    let covered = |count: i32, len: i32| count as f32 >= COVER * len as f32;
    let mut found = Vec::new();
    for &t in &tops {
        for &b in &bottoms {
            let (top, bottom) = (t + 1, b + 1);
            if bottom - top < min_side {
                continue;
            }
            for &l in &lefts {
                for &r in &rights {
                    let (left, right) = (l + 1, r + 1);
                    if right - left < min_side {
                        continue;
                    }
                    let edges_all = t == -1 && b == h - 1 && l == -1 && r == w - 1;
                    if edges_all {
                        continue;
                    }
                    let ok = (t == -1 || covered(map.hcount(t, left, right), right - left))
                        && (b == h - 1 || covered(map.hcount(b, left, right), right - left))
                        && (l == -1 || covered(map.vcount(l, top, bottom), bottom - top))
                        && (r == w - 1 || covered(map.vcount(r, top, bottom), bottom - top));
                    if ok {
                        found.push(Rect::new(left, top, right, bottom).offset(map.area.left, map.area.top));
                    }
                }
            }
        }
    }
    found.sort_by_key(|r| r.area());
    let mut out: Vec<Rect> = Vec::new();
    for r in found {
        match out.last_mut() {
            Some(last) if near(*last, r) => *last = r,
            _ => out.push(r),
        }
    }
    let whole = map.area;
    out.retain(|r| !near(*r, whole));
    out.truncate(MAX_RESULTS);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Canvas {
        w: i32,
        h: i32,
        px: Vec<u8>,
    }

    impl Canvas {
        fn new(w: i32, h: i32, bg: [u8; 3]) -> Canvas {
            let mut px = Vec::with_capacity((w * h * 4) as usize);
            for _ in 0..w * h {
                px.extend_from_slice(&[bg[0], bg[1], bg[2], 0]);
            }
            Canvas { w, h, px }
        }
        fn fill(&mut self, r: Rect, c: [u8; 3]) {
            for y in r.top..r.bottom {
                for x in r.left..r.right {
                    let i = ((y * self.w + x) * 4) as usize;
                    self.px[i..i + 3].copy_from_slice(&c);
                }
            }
        }
        fn border(&mut self, r: Rect, c: [u8; 3]) {
            self.fill(Rect::new(r.left, r.top, r.right, r.top + 1), c);
            self.fill(Rect::new(r.left, r.bottom - 1, r.right, r.bottom), c);
            self.fill(Rect::new(r.left, r.top, r.left + 1, r.bottom), c);
            self.fill(Rect::new(r.right - 1, r.top, r.right, r.bottom), c);
        }
        /// Fake text: short noisy strokes.
        fn text(&mut self, x: i32, y: i32, chars: i32, c: [u8; 3]) {
            for i in 0..chars {
                let cx = x + i * 8;
                self.fill(Rect::new(cx, y, cx + 1, y + 9), c);
                self.fill(Rect::new(cx + 1, y + 4, cx + 5, y + 5), c);
                if i % 3 == 0 {
                    self.fill(Rect::new(cx + 4, y + 2, cx + 5, y + 9), c);
                }
            }
        }
        fn frame(self, at: Point) -> Frame {
            Frame { rect: Rect::from_xywh(at.x, at.y, self.w, self.h), pixels: self.px }
        }
    }

    const BG: [u8; 3] = [240, 240, 240];
    const DARK: [u8; 3] = [60, 60, 60];
    const BLUE: [u8; 3] = [220, 120, 40];
    const WHITE: [u8; 3] = [255, 255, 255];

    fn ui() -> Frame {
        let mut c = Canvas::new(400, 300, BG);
        c.fill(Rect::new(0, 0, 80, 300), [200, 200, 205]); // sidebar flush to the left edge
        c.fill(Rect::new(110, 30, 370, 250), WHITE);
        c.border(Rect::new(110, 30, 370, 250), DARK); // card
        c.text(130, 50, 20, DARK);
        c.fill(Rect::new(140, 120, 260, 160), BLUE); // button
        c.text(155, 135, 8, WHITE);
        c.frame(Point::new(1000, 500))
    }

    #[test]
    fn finds_button_then_card() {
        let f = ui();
        let map = EdgeMap::build(&f, f.rect);
        let got = find(&map, Point::new(1000 + 200, 500 + 125), 12);
        assert!(got.len() >= 2, "{got:?}");
        assert_eq!(got[0], Rect::new(1140, 620, 1260, 660), "{got:?}");
        assert!(got.contains(&Rect::new(1110, 530, 1370, 750)), "{got:?}");
    }

    #[test]
    fn finds_card_from_its_text() {
        let f = ui();
        let map = EdgeMap::build(&f, f.rect);
        let got = find(&map, Point::new(1000 + 300, 500 + 210), 12);
        assert_eq!(got.first(), Some(&Rect::new(1110, 530, 1370, 750)), "{got:?}");
    }

    #[test]
    fn edge_flush_sidebar() {
        let f = ui();
        let map = EdgeMap::build(&f, f.rect);
        let got = find(&map, Point::new(1000 + 40, 500 + 150), 12);
        assert_eq!(got.first(), Some(&Rect::new(1000, 500, 1080, 800)), "{got:?}");
    }

    #[test]
    fn flat_area_gives_nothing() {
        let f = Canvas::new(200, 200, BG).frame(Point::new(0, 0));
        let map = EdgeMap::build(&f, f.rect);
        assert!(find(&map, Point::new(100, 100), 12).is_empty());
    }

    #[test]
    fn gradient_is_not_an_edge() {
        let mut c = Canvas::new(200, 200, BG);
        for y in 0..200 {
            let v = (40 + y) as u8;
            c.fill(Rect::new(0, y, 200, y + 1), [v, v, v]);
        }
        let f = c.frame(Point::new(0, 0));
        let map = EdgeMap::build(&f, f.rect);
        assert!(find(&map, Point::new(100, 100), 12).is_empty());
    }

    #[test]
    fn sub_area_only() {
        let f = ui();
        // Restrict to the card: the card itself is the area, so only the button comes back.
        let map = EdgeMap::build(&f, Rect::new(1110, 530, 1370, 750));
        let got = find(&map, Point::new(1200, 625), 12);
        assert_eq!(got, vec![Rect::new(1140, 620, 1260, 660)]);
    }
}

//! Renders annotations onto an image with tiny-skia (shapes) and ab_glyph
//! (text). Pure Rust, identical output on screen, on the clipboard and in
//! saved files.

use super::font::{self, Fonts};
use super::{BlurMode, Color, Item, P, Shape, norm};
use crate::geom::Rect;
use crate::image::Frame;
use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};
use tiny_skia::{BlendMode, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

/// Opaque RGBA pixmap from a BGRA frame.
pub fn base_pixmap(f: &Frame) -> Pixmap {
    let (w, h) = (f.width().max(1) as u32, f.height().max(1) as u32);
    let mut p = Pixmap::new(w, h).expect("pixmap");
    let d = p.data_mut();
    for (o, i) in d.chunks_exact_mut(4).zip(f.pixels.chunks_exact(4)) {
        o.copy_from_slice(&[i[2], i[1], i[0], 255]);
    }
    p
}

/// BGRA frame (opaque) from a pixmap.
pub fn to_frame(p: &Pixmap) -> Frame {
    let mut pixels = Vec::with_capacity(p.data().len());
    for c in p.data().chunks_exact(4) {
        pixels.extend_from_slice(&[c[2], c[1], c[0], 255]);
    }
    Frame { rect: Rect::from_xywh(0, 0, p.width() as i32, p.height() as i32), pixels }
}

fn paint(c: Color, a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c.0, c.1, c.2, a);
    p.anti_alias = true;
    p
}

fn stroke(w: f32) -> Stroke {
    Stroke { width: w.max(0.5), line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() }
}

/// Fill path of a Snagit-style tapered arrow from `a` to the tip `b`.
pub fn arrow_path(a: P, b: P, w: f32) -> Option<tiny_skia::Path> {
    let len = a.dist(b);
    if len < 1.0 {
        return None;
    }
    let (ux, uy) = ((b.x - a.x) / len, (b.y - a.y) / len);
    let (nx, ny) = (-uy, ux);
    let head = (w * 3.6 + 8.0).min(len * 0.6);
    let half = head * 0.58;
    let t0 = (w * 0.32).max(1.0);
    let t1 = (w * 0.68).max(1.4);
    let hb = P::new(b.x - ux * head, b.y - uy * head);
    let mut pb = PathBuilder::new();
    pb.move_to(a.x + nx * t0, a.y + ny * t0);
    pb.line_to(hb.x + nx * t1, hb.y + ny * t1);
    pb.line_to(hb.x + nx * half, hb.y + ny * half);
    pb.line_to(b.x, b.y);
    pb.line_to(hb.x - nx * half, hb.y - ny * half);
    pb.line_to(hb.x - nx * t1, hb.y - ny * t1);
    pb.line_to(a.x - nx * t0, a.y - ny * t0);
    pb.close();
    pb.finish()
}

fn polyline(pts: &[P]) -> Option<tiny_skia::Path> {
    let first = pts.first()?;
    let mut pb = PathBuilder::new();
    pb.move_to(first.x, first.y);
    if pts.len() == 1 {
        pb.line_to(first.x + 0.01, first.y);
    } else if pts.len() == 2 {
        pb.line_to(pts[1].x, pts[1].y);
    } else {
        // Quadratic smoothing through midpoints.
        for i in 1..pts.len() - 1 {
            let (p, q) = (pts[i], pts[i + 1]);
            pb.quad_to(p.x, p.y, (p.x + q.x) / 2.0, (p.y + q.y) / 2.0);
        }
        let l = pts[pts.len() - 1];
        pb.line_to(l.x, l.y);
    }
    pb.finish()
}

fn rect_path(a: P, b: P) -> Option<tiny_skia::Path> {
    let (l, t, r, bt) = norm(a, b);
    tiny_skia::Rect::from_ltrb(l, t, r.max(l + 0.5), bt.max(t + 0.5)).map(PathBuilder::from_rect)
}

fn oval_path(a: P, b: P) -> Option<tiny_skia::Path> {
    let (l, t, r, bt) = norm(a, b);
    tiny_skia::Rect::from_ltrb(l, t, r.max(l + 0.5), bt.max(t + 0.5)).and_then(PathBuilder::from_oval)
}

/// Item bounds in image coordinates (without shadow).
pub fn bbox(item: &Item, fonts: Option<&Fonts>) -> (f32, f32, f32, f32) {
    let pad = item.width;
    let grow = |(l, t, r, b): (f32, f32, f32, f32), p: f32| (l - p, t - p, r + p, b + p);
    match &item.shape {
        Shape::Arrow { a, b } => grow(norm(*a, *b), (item.width * 3.6 + 8.0) * 0.6),
        Shape::Line { a, b } | Shape::Rect { a, b } | Shape::Ellipse { a, b } => grow(norm(*a, *b), pad),
        Shape::Blur { a, b, .. } => norm(*a, *b),
        Shape::Pen { pts } | Shape::Highlighter { pts } => {
            let w = if matches!(item.shape, Shape::Highlighter { .. }) { item.width * 3.2 } else { item.width };
            let l = pts.iter().map(|p| p.x).fold(f32::MAX, f32::min);
            let t = pts.iter().map(|p| p.y).fold(f32::MAX, f32::min);
            let r = pts.iter().map(|p| p.x).fold(f32::MIN, f32::max);
            let b = pts.iter().map(|p| p.y).fold(f32::MIN, f32::max);
            grow((l, t, r, b), w)
        }
        Shape::Text { at, text, size } => match fonts {
            Some(f) => grow(font::text_box(&f.text, *size, text, at.x, at.y), outline_radius(*size)),
            None => (at.x, at.y, at.x + size * 4.0, at.y + size * 1.3),
        },
        Shape::Step { c, r } => (c.x - r, c.y - r, c.x + r, c.y + r),
    }
}

pub fn outline_radius(size: f32) -> f32 {
    (size / 11.0).max(1.6)
}

/// Draws shapes in `draw` into a scratch pixmap around `bounds`, blurs it
/// into a soft shadow and lays it under, offset down-right.
fn shadow(target: &mut Pixmap, bounds: (f32, f32, f32, f32), scale: f32, draw: impl Fn(&mut Pixmap, Transform)) {
    let blur = (3.0 * scale).round().max(2.0) as i32;
    let off = (2.0 * scale).round().max(1.0) as i32;
    let pad = blur * 3 + 2;
    let (l, t) = (bounds.0.floor() as i32 - pad, bounds.1.floor() as i32 - pad);
    let (w, h) = ((bounds.2.ceil() as i32 + pad - l).max(1), (bounds.3.ceil() as i32 + pad - t).max(1));
    if w as i64 * h as i64 > 40_000_000 {
        return;
    }
    let Some(mut tmp) = Pixmap::new(w as u32, h as u32) else { return };
    draw(&mut tmp, Transform::from_translate(-l as f32, -t as f32));
    let full = (0, 0, w, h);
    box_blur(&mut tmp, full, blur, 3);
    let pp = PixmapPaint { opacity: 0.42, ..Default::default() };
    target.draw_pixmap(l + off, t + off, tmp.as_ref(), &pp, Transform::identity(), None);
}

/// Separable box blur of premultiplied RGBA inside `area` (l, t, r, b).
pub fn box_blur(p: &mut Pixmap, area: (i32, i32, i32, i32), radius: i32, passes: u32) {
    let (pw, ph) = (p.width() as i32, p.height() as i32);
    let (l, t, r, b) = (area.0.max(0), area.1.max(0), area.2.min(pw), area.3.min(ph));
    if r - l < 2 || b - t < 2 || radius < 1 {
        return;
    }
    let (w, h) = ((r - l) as usize, (b - t) as usize);
    let stride = pw as usize * 4;
    let data = p.data_mut();
    let mut line = vec![0u32; w.max(h) * 4];
    let mut out = vec![0u8; w.max(h) * 4];
    let blur_line = |src: &mut dyn FnMut(usize, usize) -> u8, n: usize, line: &mut [u32], out: &mut [u8]| {
        for i in 0..n {
            for c in 0..4 {
                line[i * 4 + c] = src(i, c) as u32;
            }
        }
        let rad = radius as usize;
        let win = (rad * 2 + 1) as u32;
        for c in 0..4 {
            let at = |i: isize| -> u32 { line[(i.clamp(0, n as isize - 1) as usize) * 4 + c] };
            let mut sum: u32 = (-(rad as isize)..=rad as isize).map(at).sum();
            for i in 0..n {
                out[i * 4 + c] = (sum / win) as u8;
                sum += at(i as isize + rad as isize + 1);
                sum -= at(i as isize - rad as isize);
            }
        }
    };
    for _ in 0..passes {
        for y in 0..h {
            let row = (t as usize + y) * stride + l as usize * 4;
            {
                let d = &*data;
                let mut src = |i: usize, c: usize| d[row + i * 4 + c];
                blur_line(&mut src, w, &mut line, &mut out);
            }
            data[row..row + w * 4].copy_from_slice(&out[..w * 4]);
        }
        for x in 0..w {
            let col = l as usize * 4 + x * 4;
            {
                let d = &*data;
                let mut src = |i: usize, c: usize| d[(t as usize + i) * stride + col + c];
                blur_line(&mut src, h, &mut line, &mut out);
            }
            for y in 0..h {
                let i = (t as usize + y) * stride + col;
                data[i..i + 4].copy_from_slice(&out[y * 4..y * 4 + 4]);
            }
        }
    }
}

/// Averages square blocks inside `area`.
pub fn pixelate(p: &mut Pixmap, area: (i32, i32, i32, i32), block: i32) {
    let (pw, ph) = (p.width() as i32, p.height() as i32);
    let (l, t, r, b) = (area.0.max(0), area.1.max(0), area.2.min(pw), area.3.min(ph));
    let stride = pw as usize * 4;
    let data = p.data_mut();
    let mut by = t;
    while by < b {
        let mut bx = l;
        while bx < r {
            let (ex, ey) = ((bx + block).min(r), (by + block).min(b));
            let mut sum = [0u32; 4];
            let n = ((ex - bx) * (ey - by)) as u32;
            for y in by..ey {
                for x in bx..ex {
                    let i = y as usize * stride + x as usize * 4;
                    for c in 0..4 {
                        sum[c] += data[i + c] as u32;
                    }
                }
            }
            let avg = sum.map(|s| (s / n.max(1)) as u8);
            for y in by..ey {
                for x in bx..ex {
                    let i = y as usize * stride + x as usize * 4;
                    data[i..i + 4].copy_from_slice(&avg);
                }
            }
            bx += block;
        }
        by += block;
    }
}

/// Coverage mask of a text block; top-left of the first line at (pad, pad).
struct Mask {
    w: usize,
    h: usize,
    a: Vec<f32>,
}

fn text_mask(font: &FontArc, size: f32, text: &str, pad: usize) -> Mask {
    let scaled = font.as_scaled(PxScale::from(size));
    let lh = font::line_height(font, size);
    let lines: Vec<&str> = text.split('\n').collect();
    let width = lines.iter().map(|l| font::line_width(font, size, l)).fold(0.0f32, f32::max);
    let w = width.ceil() as usize + pad * 2 + 2;
    let h = (lh * lines.len().max(1) as f32).ceil() as usize + pad * 2 + 2;
    let mut a = vec![0f32; w * h];
    for (li, line) in lines.iter().enumerate() {
        let baseline = pad as f32 + li as f32 * lh + scaled.ascent();
        let mut x = pad as f32;
        let mut prev = None;
        for c in line.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            let g = id.with_scale_and_position(PxScale::from(size), point(x, baseline));
            if let Some(og) = font.outline_glyph(g) {
                let bb = og.px_bounds();
                og.draw(|gx, gy, cov| {
                    let px = bb.min.x as i32 + gx as i32;
                    let py = bb.min.y as i32 + gy as i32;
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        let i = py as usize * w + px as usize;
                        a[i] = a[i].max(cov.min(1.0));
                    }
                });
            }
            x += scaled.h_advance(id);
            prev = Some(id);
        }
    }
    Mask { w, h, a }
}

fn dilate(m: &Mask, r: f32) -> Mask {
    let ri = r.ceil() as i32;
    let mut out = vec![0f32; m.a.len()];
    let offs: Vec<(i32, i32, f32)> = (-ri..=ri)
        .flat_map(|dy| (-ri..=ri).map(move |dx| (dx, dy)))
        .filter_map(|(dx, dy)| {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            (d <= r + 0.5).then(|| (dx, dy, (r + 0.5 - d).clamp(0.0, 1.0)))
        })
        .collect();
    for y in 0..m.h as i32 {
        for x in 0..m.w as i32 {
            let mut v = 0f32;
            for &(dx, dy, wgt) in &offs {
                let (sx, sy) = (x + dx, y + dy);
                if sx >= 0 && sy >= 0 && (sx as usize) < m.w && (sy as usize) < m.h {
                    v = v.max(m.a[sy as usize * m.w + sx as usize] * wgt);
                }
            }
            out[y as usize * m.w + x as usize] = v;
        }
    }
    Mask { w: m.w, h: m.h, a: out }
}

fn blend_mask(p: &mut Pixmap, m: &Mask, x0: i32, y0: i32, c: Color) {
    let (pw, ph) = (p.width() as i32, p.height() as i32);
    let stride = pw as usize * 4;
    let data = p.data_mut();
    for y in 0..m.h as i32 {
        let py = y0 + y;
        if py < 0 || py >= ph {
            continue;
        }
        for x in 0..m.w as i32 {
            let px = x0 + x;
            let a = m.a[y as usize * m.w + x as usize];
            if px < 0 || px >= pw || a <= 0.0 {
                continue;
            }
            let i = py as usize * stride + px as usize * 4;
            let src = [c.0, c.1, c.2];
            for k in 0..3 {
                data[i + k] = (src[k] as f32 * a + data[i + k] as f32 * (1.0 - a)).round() as u8;
            }
            data[i + 3] = (255.0 * a + data[i + 3] as f32 * (1.0 - a)).round() as u8;
        }
    }
}

fn draw_text(p: &mut Pixmap, fonts: &Fonts, at: P, text: &str, size: f32, color: Color) {
    let r = outline_radius(size);
    let pad = r.ceil() as usize + 2;
    let m = text_mask(&fonts.text, size, text, pad);
    let outline = dilate(&m, r);
    let oc = if color.luma() > 0.6 { Color(0x1C, 0x20, 0x24) } else { Color(255, 255, 255) };
    let (x0, y0) = (at.x.round() as i32 - pad as i32, at.y.round() as i32 - pad as i32);
    blend_mask(p, &outline, x0, y0, oc);
    blend_mask(p, &m, x0, y0, color);
}

fn draw_step(p: &mut Pixmap, fonts: Option<&Fonts>, c: P, r: f32, color: Color, n: usize, scale: f32) {
    if let Some(circle) = PathBuilder::from_circle(c.x, c.y, r) {
        shadow(p, (c.x - r, c.y - r, c.x + r, c.y + r), scale, |t, tr| {
            t.fill_path(&circle, &paint(Color(0, 0, 0), 255), FillRule::Winding, tr, None);
        });
        p.fill_path(&circle, &paint(color, 255), FillRule::Winding, Transform::identity(), None);
        let ring = if color.luma() > 0.8 { Color(0x1C, 0x20, 0x24) } else { Color(255, 255, 255) };
        if let Some(rp) = PathBuilder::from_circle(c.x, c.y, r - r * 0.07) {
            p.stroke_path(&rp, &paint(ring, 255), &stroke(r * 0.12), Transform::identity(), None);
        }
    }
    let Some(f) = fonts else { return };
    let size = if n >= 10 { r * 0.95 } else { r * 1.15 };
    let label = n.to_string();
    let m = text_mask(&f.bold, size, &label, 1);
    // Centre the ink, not the advance box.
    let (mut l, mut t, mut rr, mut b) = (m.w, m.h, 0usize, 0usize);
    for y in 0..m.h {
        for x in 0..m.w {
            if m.a[y * m.w + x] > 0.3 {
                l = l.min(x);
                t = t.min(y);
                rr = rr.max(x);
                b = b.max(y);
            }
        }
    }
    if rr < l {
        return;
    }
    let cx = (l + rr) as f32 / 2.0;
    let cy = (t + b) as f32 / 2.0;
    let fg = if color.luma() > 0.8 { Color(0x1C, 0x20, 0x24) } else { Color(255, 255, 255) };
    blend_mask(p, &m, (c.x - cx).round() as i32, (c.y - cy).round() as i32, fg);
}

/// Draws one item at its coordinates (already relative to `p`).
pub fn draw_item(p: &mut Pixmap, item: &Item, fonts: Option<&Fonts>, step_no: usize, scale: f32) {
    let c = item.color;
    let w = item.width;
    let id = Transform::identity();
    match &item.shape {
        Shape::Arrow { a, b } => {
            if let Some(path) = arrow_path(*a, *b, w) {
                shadow(p, bbox(item, None), scale, |t, tr| t.fill_path(&path, &paint(Color(0, 0, 0), 255), FillRule::Winding, tr, None));
                p.fill_path(&path, &paint(c, 255), FillRule::Winding, id, None);
            }
        }
        Shape::Line { a, b } => {
            if let Some(path) = polyline(&[*a, *b]) {
                shadow(p, bbox(item, None), scale, |t, tr| t.stroke_path(&path, &paint(Color(0, 0, 0), 255), &stroke(w), tr, None));
                p.stroke_path(&path, &paint(c, 255), &stroke(w), id, None);
            }
        }
        Shape::Rect { a, b } | Shape::Ellipse { a, b } => {
            let path = if matches!(item.shape, Shape::Rect { .. }) { rect_path(*a, *b) } else { oval_path(*a, *b) };
            if let Some(path) = path {
                let mut st = stroke(w);
                st.line_join = LineJoin::Miter;
                shadow(p, bbox(item, None), scale, |t, tr| t.stroke_path(&path, &paint(Color(0, 0, 0), 255), &st, tr, None));
                p.stroke_path(&path, &paint(c, 255), &st, id, None);
            }
        }
        Shape::Pen { pts } => {
            if let Some(path) = polyline(pts) {
                p.stroke_path(&path, &paint(c, 255), &stroke(w), id, None);
            }
        }
        Shape::Highlighter { pts } => {
            if let Some(path) = polyline(pts) {
                let mut pt = paint(c, 255);
                pt.blend_mode = BlendMode::Multiply;
                let st = Stroke { width: w * 3.2, line_cap: LineCap::Butt, line_join: LineJoin::Round, ..Default::default() };
                p.stroke_path(&path, &pt, &st, id, None);
            }
        }
        Shape::Text { at, text, size } => {
            if let Some(f) = fonts {
                draw_text(p, f, *at, text, *size, c);
            }
        }
        Shape::Blur { a, b, mode } => {
            let (l, t, r, bt) = norm(*a, *b);
            let area = (l.round() as i32, t.round() as i32, r.round() as i32, bt.round() as i32);
            let short = (area.2 - area.0).min(area.3 - area.1).max(1);
            match mode {
                BlurMode::Blur => box_blur(p, area, (short / 10).clamp(4, 24), 3),
                BlurMode::Pixelate => pixelate(p, area, (short / 12).clamp(6, 24)),
            }
        }
        Shape::Step { c: center, r } => draw_step(p, fonts, *center, *r, c, step_no, scale),
    }
}

/// Composites `items` over `base` and returns the `clip` part of it.
/// Items are in base coordinates.
pub fn render(base: &Pixmap, items: &[Item], clip: Rect, fonts: Option<&Fonts>, scale: f32) -> Pixmap {
    let (w, h) = (clip.width().max(1) as u32, clip.height().max(1) as u32);
    let mut out = Pixmap::new(w, h).expect("pixmap");
    let bw = base.width() as i32;
    let bh = base.height() as i32;
    let src = base.data();
    let dst_stride = w as usize * 4;
    {
        let dst = out.data_mut();
        for y in 0..h as i32 {
            let sy = clip.top + y;
            if sy < 0 || sy >= bh {
                continue;
            }
            let x0 = clip.left.max(0);
            let x1 = clip.right.min(bw);
            if x1 <= x0 {
                continue;
            }
            let s = (sy as usize * bw as usize + x0 as usize) * 4;
            let d = y as usize * dst_stride + (x0 - clip.left) as usize * 4;
            let n = (x1 - x0) as usize * 4;
            dst[d..d + n].copy_from_slice(&src[s..s + n]);
        }
    }
    let mut step = 0;
    for it in items {
        let mut it = it.clone();
        it.translate(-clip.left as f32, -clip.top as f32);
        if matches!(it.shape, Shape::Step { .. }) {
            step += 1;
        }
        draw_item(&mut out, &it, fonts, step, scale);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annot::{PALETTE, Shape};

    fn gray(w: i32, h: i32) -> Pixmap {
        let mut p = Pixmap::new(w as u32, h as u32).unwrap();
        for c in p.data_mut().chunks_exact_mut(4) {
            c.copy_from_slice(&[200, 200, 200, 255]);
        }
        p
    }

    fn px(p: &Pixmap, x: i32, y: i32) -> [u8; 4] {
        let i = (y as usize * p.width() as usize + x as usize) * 4;
        p.data()[i..i + 4].try_into().unwrap()
    }

    fn red_item(shape: Shape) -> Item {
        Item { shape, color: PALETTE[0], width: 5.0 }
    }

    fn is_red(c: [u8; 4]) -> bool {
        c[0] > 180 && c[1] < 120 && c[2] < 120
    }

    #[test]
    fn arrow_lands_on_its_line_only() {
        let base = gray(200, 100);
        let it = red_item(Shape::Arrow { a: P::new(20.0, 50.0), b: P::new(180.0, 50.0) });
        let out = render(&base, &[it], Rect::new(0, 0, 200, 100), None, 1.0);
        assert!(is_red(px(&out, 100, 50)));
        assert!(is_red(px(&out, 170, 50)), "head");
        assert!(!is_red(px(&out, 100, 80)));
        // Soft shadow darkens below-right of the shaft.
        assert!(px(&out, 100, 56)[0] < 200);
        // Head is wider than the shaft.
        assert!(is_red(px(&out, 160, 58)));
        assert!(!is_red(px(&out, 60, 58)));
    }

    #[test]
    fn rect_is_an_outline() {
        let base = gray(100, 100);
        let it = red_item(Shape::Rect { a: P::new(20.0, 20.0), b: P::new(80.0, 80.0) });
        let out = render(&base, &[it], Rect::new(0, 0, 100, 100), None, 1.0);
        assert!(is_red(px(&out, 20, 50)));
        assert!(is_red(px(&out, 50, 80)));
        assert_eq!(px(&out, 50, 50), [200, 200, 200, 255]);
    }

    #[test]
    fn highlighter_multiplies() {
        let mut base = gray(100, 50);
        // A black "text" pixel under the marker stays black.
        let i = (25 * 100 + 50) * 4;
        base.data_mut()[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
        let it = Item { shape: Shape::Highlighter { pts: vec![P::new(10.0, 25.0), P::new(90.0, 25.0)] }, color: PALETTE[2], width: 5.0 };
        let out = render(&base, &[it], Rect::new(0, 0, 100, 50), None, 1.0);
        assert_eq!(px(&out, 50, 25)[..3], [0, 0, 0]);
        let y = px(&out, 30, 25);
        assert!(y[0] > 180 && y[2] < 60, "{y:?}");
    }

    #[test]
    fn blur_and_pixelate_reduce_detail() {
        let mut base = gray(60, 60);
        for (i, c) in base.data_mut().chunks_exact_mut(4).enumerate() {
            let v = if (i % 60 + i / 60) % 2 == 0 { 0 } else { 255 };
            c.copy_from_slice(&[v, v, v, 255]);
        }
        let var = |p: &Pixmap| {
            let v: Vec<f32> = (20..40).flat_map(|y| (20..40).map(move |x| (x, y))).map(|(x, y)| px(p, x, y)[0] as f32).collect();
            let m = v.iter().sum::<f32>() / v.len() as f32;
            v.iter().map(|a| (a - m).powi(2)).sum::<f32>() / v.len() as f32
        };
        let before = var(&base);
        let b = render(&base, &[red_item(Shape::Blur { a: P::new(10.0, 10.0), b: P::new(50.0, 50.0), mode: BlurMode::Blur })], Rect::new(0, 0, 60, 60), None, 1.0);
        assert!(var(&b) < before / 20.0);
        let p = render(&base, &[red_item(Shape::Blur { a: P::new(0.0, 0.0), b: P::new(60.0, 60.0), mode: BlurMode::Pixelate })], Rect::new(0, 0, 60, 60), None, 1.0);
        assert_eq!(px(&p, 0, 0), px(&p, 5, 5));
        // Outside the blur box nothing changes.
        assert_eq!(px(&b, 2, 2), px(&base, 2, 2));
    }

    #[test]
    fn clip_offsets_items() {
        let base = gray(300, 300);
        let it = red_item(Shape::Rect { a: P::new(120.0, 120.0), b: P::new(180.0, 180.0) });
        let out = render(&base, &[it], Rect::new(100, 100, 200, 200), None, 1.0);
        assert_eq!((out.width(), out.height()), (100, 100));
        assert!(is_red(px(&out, 20, 50)));
    }

    #[test]
    fn text_and_steps_draw_with_a_font() {
        let Some(f) = Fonts::load() else { return };
        let base = gray(200, 100);
        let items = vec![
            red_item(Shape::Text { at: P::new(10.0, 10.0), text: "Hi".into(), size: 30.0 }),
            red_item(Shape::Step { c: P::new(150.0, 50.0), r: 16.0 }),
        ];
        let out = render(&base, &items, Rect::new(0, 0, 200, 100), Some(&f), 1.0);
        let reds = (0..100).flat_map(|y| (0..100).map(move |x| (x, y))).filter(|&(x, y)| is_red(px(&out, x, y))).count();
        assert!(reds > 30, "{reds} red text pixels");
        // Step: red disc with white digit in the middle area.
        assert!(is_red(px(&out, 150, 38)));
        let whites = (40..60).flat_map(|y| (142..158).map(move |x| (x, y))).filter(|&(x, y)| px(&out, x, y)[0] > 230 && px(&out, x, y)[1] > 230).count();
        assert!(whites > 10, "{whites} white digit pixels");
    }

    #[test]
    fn frame_round_trip() {
        let f = Frame { rect: Rect::new(0, 0, 2, 1), pixels: vec![1, 2, 3, 0, 4, 5, 6, 0] };
        let p = base_pixmap(&f);
        assert_eq!(&p.data()[..4], &[3, 2, 1, 255]);
        assert_eq!(to_frame(&p).pixels, vec![1, 2, 3, 255, 4, 5, 6, 255]);
    }
}

#[cfg(test)]
mod demo {
    use super::*;
    use crate::annot::{PALETTE, Shape};

    /// `cargo test demo_sheet -- --ignored` writes a look-at-it sheet.
    #[test]
    #[ignore]
    fn demo_sheet() {
        let f = Fonts::load();
        let (w, h) = (900, 520);
        let mut base = Pixmap::new(w, h).unwrap();
        for (i, c) in base.data_mut().chunks_exact_mut(4).enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let v = 235.0 - y * 0.05;
            c.copy_from_slice(&[v as u8, (v + 2.0) as u8, (v + 6.0) as u8, 255]);
            if (40..60).contains(&(y as i32 % 70)) && x > 520.0 && x < 860.0 && ((x as i32 / 6) % 3 != 0) {
                c.copy_from_slice(&[40, 44, 52, 255]);
            }
        }
        let it = |shape, c: usize, w: f32| Item { shape, color: PALETTE[c], width: w };
        let items = vec![
            it(Shape::Arrow { a: P::new(60.0, 420.0), b: P::new(300.0, 120.0) }, 0, 5.0),
            it(Shape::Arrow { a: P::new(80.0, 470.0), b: P::new(420.0, 380.0) }, 4, 3.0),
            it(Shape::Arrow { a: P::new(330.0, 470.0), b: P::new(470.0, 250.0) }, 3, 8.0),
            it(Shape::Rect { a: P::new(330.0, 60.0), b: P::new(480.0, 160.0) }, 0, 5.0),
            it(Shape::Ellipse { a: P::new(40.0, 40.0), b: P::new(200.0, 120.0) }, 5, 5.0),
            it(Shape::Highlighter { pts: vec![P::new(525.0, 50.0), P::new(855.0, 50.0)] }, 2, 5.0),
            it(Shape::Blur { a: P::new(520.0, 100.0), b: P::new(860.0, 140.0), mode: BlurMode::Blur }, 0, 5.0),
            it(Shape::Blur { a: P::new(520.0, 170.0), b: P::new(860.0, 210.0), mode: BlurMode::Pixelate }, 0, 5.0),
            it(Shape::Text { at: P::new(540.0, 300.0), text: "Click here to save".into(), size: 26.0 }, 0, 0.0),
            it(Shape::Text { at: P::new(540.0, 350.0), text: "Yellow text\nsecond line".into(), size: 26.0 }, 2, 0.0),
            it(Shape::Step { c: P::new(560.0, 460.0), r: 16.0 }, 0, 0.0),
            it(Shape::Step { c: P::new(620.0, 460.0), r: 16.0 }, 4, 0.0),
            it(Shape::Step { c: P::new(690.0, 460.0), r: 22.0 }, 3, 0.0),
            it(Shape::Pen { pts: (0..40).map(|i| P::new(240.0 + i as f32 * 5.0, 230.0 + (i as f32 * 0.5).sin() * 18.0)).collect() }, 1, 5.0),
            it(Shape::Line { a: P::new(40.0, 300.0), b: P::new(200.0, 300.0) }, 6, 5.0),
        ];
        let out = render(&base, &items, Rect::new(0, 0, w as i32, h as i32), f.as_ref(), 1.0);
        std::fs::write("/tmp/annot_demo.png", to_frame(&out).encode_png()).unwrap();
    }
}

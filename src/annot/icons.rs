//! Tool icons drawn with tiny-skia on a 24-unit grid: white on transparent,
//! so the toolbar never depends on which icon font a Windows build ships.

use super::render::arrow_path;
use super::{P, Tool};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};

fn white(a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(255, 255, 255, a);
    p.anti_alias = true;
    p
}

fn st(w: f32) -> Stroke {
    Stroke { width: w, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Default::default() }
}

/// RGBA (premultiplied) icon of `n x n` pixels.
pub fn tool_icon(t: Tool, n: u32) -> Pixmap {
    let mut p = Pixmap::new(n, n).expect("icon");
    let s = n as f32 / 24.0;
    let tr = Transform::from_scale(s, s);
    let line = |p: &mut Pixmap, pts: &[(f32, f32)], w: f32, a: u8| {
        let mut pb = PathBuilder::new();
        pb.move_to(pts[0].0, pts[0].1);
        for q in &pts[1..] {
            pb.line_to(q.0, q.1);
        }
        if let Some(path) = pb.finish() {
            p.stroke_path(&path, &white(a), &st(w), tr, None);
        }
    };
    match t {
        Tool::Select => {
            let mut pb = PathBuilder::new();
            pb.move_to(6.0, 3.5);
            pb.line_to(18.0, 13.5);
            pb.line_to(12.5, 14.2);
            pb.line_to(15.5, 20.0);
            pb.line_to(13.0, 21.0);
            pb.line_to(10.2, 15.3);
            pb.line_to(6.0, 19.0);
            pb.close();
            if let Some(path) = pb.finish() {
                p.stroke_path(&path, &white(255), &st(1.6), tr, None);
            }
        }
        Tool::Arrow => {
            if let Some(path) = arrow_path(P::new(4.5, 19.5), P::new(20.0, 4.0), 2.6) {
                p.fill_path(&path, &white(255), FillRule::Winding, tr, None);
            }
        }
        Tool::Line => line(&mut p, &[(5.0, 19.0), (19.0, 5.0)], 2.2, 255),
        Tool::Rect => line(&mut p, &[(4.5, 6.5), (19.5, 6.5), (19.5, 17.5), (4.5, 17.5), (4.5, 6.5)], 1.9, 255),
        Tool::Ellipse => {
            if let Some(r) = tiny_skia::Rect::from_ltrb(4.0, 6.0, 20.0, 18.0) {
                if let Some(path) = PathBuilder::from_oval(r) {
                    p.stroke_path(&path, &white(255), &st(1.9), tr, None);
                }
            }
        }
        Tool::Pen => {
            let mut pb = PathBuilder::new();
            pb.move_to(4.0, 17.0);
            pb.cubic_to(7.0, 6.0, 10.0, 6.0, 12.0, 12.0);
            pb.cubic_to(14.0, 18.0, 17.0, 18.0, 20.0, 7.0);
            if let Some(path) = pb.finish() {
                p.stroke_path(&path, &white(255), &st(2.0), tr, None);
            }
        }
        Tool::Highlighter => {
            let mut pb = PathBuilder::new();
            pb.move_to(4.0, 16.0);
            pb.line_to(20.0, 8.0);
            if let Some(path) = pb.finish() {
                let s2 = Stroke { width: 6.5, line_cap: LineCap::Butt, ..Default::default() };
                p.stroke_path(&path, &white(150), &s2, tr, None);
            }
            line(&mut p, &[(4.0, 20.5), (20.0, 20.5)], 1.4, 255);
        }
        Tool::Text => {
            line(&mut p, &[(5.5, 6.0), (18.5, 6.0)], 2.2, 255);
            line(&mut p, &[(12.0, 6.0), (12.0, 19.5)], 2.2, 255);
        }
        Tool::Blur => {
            for (i, (x, y)) in [(5.0, 5.0), (10.0, 5.0), (15.0, 5.0), (5.0, 10.0), (10.0, 10.0), (15.0, 10.0), (5.0, 15.0), (10.0, 15.0), (15.0, 15.0)]
                .iter()
                .enumerate()
            {
                let a = [255, 140, 70, 140, 70, 140, 70, 140, 255][i];
                if let Some(r) = tiny_skia::Rect::from_xywh(*x, *y, 4.2, 4.2) {
                    p.fill_rect(r, &white(a), tr, None);
                }
            }
        }
        Tool::Step => {
            if let Some(path) = PathBuilder::from_circle(12.0, 12.0, 8.0) {
                p.stroke_path(&path, &white(255), &st(1.9), tr, None);
            }
            line(&mut p, &[(10.4, 9.6), (12.4, 8.0), (12.4, 16.5)], 1.9, 255);
        }
        Tool::Crop => {
            line(&mut p, &[(7.0, 3.5), (7.0, 17.0), (20.5, 17.0)], 2.0, 255);
            line(&mut p, &[(3.5, 7.0), (17.0, 7.0), (17.0, 20.5)], 2.0, 255);
        }
        Tool::CutOut => {
            // A dashed outline lifting off, with a sparkle: "cut by shape".
            let mut pb = PathBuilder::new();
            pb.move_to(5.0, 17.5);
            pb.cubic_to(3.0, 12.0, 6.0, 6.5, 11.0, 6.5);
            pb.cubic_to(15.5, 6.5, 17.5, 10.0, 16.5, 13.0);
            pb.cubic_to(15.5, 16.5, 12.0, 15.5, 11.0, 18.5);
            pb.cubic_to(10.5, 20.0, 7.0, 20.5, 5.0, 17.5);
            pb.close();
            if let Some(path) = pb.finish() {
                let mut s = st(1.8);
                s.dash = tiny_skia::StrokeDash::new(vec![2.6, 2.2], 0.0);
                p.stroke_path(&path, &white(255), &s, tr, None);
            }
            let mut sp = PathBuilder::new();
            sp.move_to(18.5, 2.5);
            sp.quad_to(19.0, 5.0, 21.5, 5.5);
            sp.quad_to(19.0, 6.0, 18.5, 8.5);
            sp.quad_to(18.0, 6.0, 15.5, 5.5);
            sp.quad_to(18.0, 5.0, 18.5, 2.5);
            sp.close();
            if let Some(path) = sp.finish() {
                p.fill_path(&path, &white(255), FillRule::Winding, tr, None);
            }
        }
        Tool::Zoom => {
            if let Some(path) = PathBuilder::from_circle(10.0, 10.0, 6.0) {
                p.stroke_path(&path, &white(255), &st(2.0), tr, None);
            }
            line(&mut p, &[(14.5, 14.5), (20.0, 20.0)], 2.6, 255);
            line(&mut p, &[(7.5, 10.0), (12.5, 10.0)], 1.7, 255);
            line(&mut p, &[(10.0, 7.5), (10.0, 12.5)], 1.7, 255);
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annot::ALL_TOOLS;

    #[test]
    fn every_icon_has_ink() {
        for t in ALL_TOOLS.into_iter().chain([Tool::Zoom, Tool::CutOut]) {
            let p = tool_icon(t, 24);
            let ink = p.data().chunks_exact(4).filter(|c| c[3] > 100).count();
            assert!(ink > 15, "{t:?} has {ink} pixels");
        }
    }
}

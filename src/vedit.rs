//! Video editor model, pure Rust: clips cut from one source file and played
//! in any order, annotation items with on-screen spans, camera zooms, and the
//! per-frame composition shared by the preview and the export.
//!
//! Times are milliseconds. "Source" time is a position in the recorded file;
//! "output" time is a position in the edited video (clips back to back).

use crate::annot::font::Fonts;
use crate::annot::render::{bbox, draw_item};
use crate::annot::{Item, P, Shape, norm};
use crate::geom::Rect;
use crate::image::Frame;
use std::path::{Path, PathBuf};
use tiny_skia::Pixmap;

/// Shortest clip a split or trim may leave.
pub const MIN_CLIP_MS: u32 = 100;
/// Fade in and out of annotations.
pub const FADE_MS: u32 = 160;
/// Camera ease into and out of a zoom.
pub const ZOOM_EASE_MS: u32 = 450;
/// Smallest zoom area, as a fraction of the frame width.
pub const MIN_ZOOM_FRAC: f32 = 0.12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clip {
    /// Source start (ms).
    pub a: u32,
    /// Source end (ms), exclusive.
    pub b: u32,
}

impl Clip {
    pub fn len(&self) -> u32 {
        self.b.saturating_sub(self.a)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Timeline {
    pub src_ms: u32,
    pub clips: Vec<Clip>,
}

impl Timeline {
    pub fn new(src_ms: u32) -> Timeline {
        Timeline { src_ms, clips: vec![Clip { a: 0, b: src_ms.max(1) }] }
    }

    /// Length of the edited video.
    pub fn total(&self) -> u32 {
        self.clips.iter().map(Clip::len).sum()
    }

    /// Output time where clip `i` starts.
    pub fn start_of(&self, i: usize) -> u32 {
        self.clips[..i.min(self.clips.len())].iter().map(Clip::len).sum()
    }

    /// Clip under output time `t` (t in [0, total)).
    pub fn index_at(&self, t: u32) -> Option<usize> {
        let mut s = 0;
        for (i, c) in self.clips.iter().enumerate() {
            if t < s + c.len() {
                return Some(i);
            }
            s += c.len();
        }
        None
    }

    /// Clip and source time shown at output time `t`; the very end shows the
    /// last frame of the last clip.
    pub fn locate(&self, t: u32) -> (usize, u32) {
        match self.index_at(t) {
            Some(i) => (i, self.clips[i].a + (t - self.start_of(i))),
            None => {
                let i = self.clips.len() - 1;
                (i, self.clips[i].b.saturating_sub(1).max(self.clips[i].a))
            }
        }
    }

    /// Splits the clip under `t` in two. False if a piece would be too short.
    pub fn split(&mut self, t: u32) -> bool {
        let Some(i) = self.index_at(t) else { return false };
        let at = self.clips[i].a + (t - self.start_of(i));
        let c = self.clips[i];
        if at < c.a + MIN_CLIP_MS || at + MIN_CLIP_MS > c.b {
            return false;
        }
        self.clips[i] = Clip { a: c.a, b: at };
        self.clips.insert(i + 1, Clip { a: at, b: c.b });
        true
    }

    /// Removes clip `i`; the last clip can not go.
    pub fn delete(&mut self, i: usize) -> bool {
        if self.clips.len() < 2 || i >= self.clips.len() {
            return false;
        }
        self.clips.remove(i);
        true
    }

    /// Moves clip `from` so it ends up at index `to` of the new order.
    pub fn move_clip(&mut self, from: usize, to: usize) -> bool {
        let n = self.clips.len();
        if from >= n || to >= n || from == to {
            return false;
        }
        let c = self.clips.remove(from);
        self.clips.insert(to, c);
        true
    }

    /// Sets clip `i` to source [a, b), kept inside the source and at least
    /// MIN_CLIP_MS long.
    pub fn trim(&mut self, i: usize, a: u32, b: u32) -> bool {
        let Some(c) = self.clips.get(i).copied() else { return false };
        let a = a.min(self.src_ms.saturating_sub(MIN_CLIP_MS));
        let b = b.clamp(a + MIN_CLIP_MS, self.src_ms.max(a + MIN_CLIP_MS));
        let n = Clip { a, b };
        if n == c {
            return false;
        }
        self.clips[i] = n;
        true
    }
}

/// Where the camera looks, in source pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cam {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Cam {
    pub fn full(fw: f32, fh: f32) -> Cam {
        Cam { x: 0.0, y: 0.0, w: fw, h: fh }
    }

    fn lerp(self, o: Cam, k: f32) -> Cam {
        Cam { x: self.x + (o.x - self.x) * k, y: self.y + (o.y - self.y) * k, w: self.w + (o.w - self.w) * k, h: self.h + (o.h - self.h) * k }
    }

    pub fn is_full(&self, fw: f32, fh: f32) -> bool {
        self.x.abs() < 0.01 && self.y.abs() < 0.01 && (self.w - fw).abs() < 0.01 && (self.h - fh).abs() < 0.01
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The area a zoom item really shows: the drawn rect grown to the frame's
/// aspect around its centre, never smaller than MIN_ZOOM_FRAC of the frame,
/// and kept inside the frame.
pub fn zoom_rect(a: P, b: P, fw: f32, fh: f32) -> Cam {
    let (l, t, r, bt) = norm(a, b);
    let (cx, cy) = ((l + r) / 2.0, (t + bt) / 2.0);
    let aspect = fw / fh.max(1.0);
    let mut w = (r - l).max((bt - t) * aspect).max(fw * MIN_ZOOM_FRAC).min(fw);
    let mut h = w / aspect;
    if h > fh {
        h = fh;
        w = h * aspect;
    }
    let x = (cx - w / 2.0).clamp(0.0, (fw - w).max(0.0));
    let y = (cy - h / 2.0).clamp(0.0, (fh - h).max(0.0));
    Cam { x, y, w, h }
}

/// How far into its zoom the camera is at `t` (0 = full frame, 1 = zoomed).
fn zoom_weight(span: (u32, u32), t: u32) -> f32 {
    let (a, b) = span;
    if t < a || t >= b {
        return 0.0;
    }
    let ease = ZOOM_EASE_MS.min((b - a) / 2).max(1) as f32;
    let k = ((t - a) as f32 / ease).min((b - t) as f32 / ease).min(1.0);
    smooth(k)
}

/// Camera at output time `t`: full frame, eased into and out of every zoom
/// item whose span covers `t` (later items win where zooms meet).
pub fn camera(items: &[Item], t: u32, fw: f32, fh: f32) -> Cam {
    let mut zooms: Vec<(&Item, (u32, u32))> = items.iter().filter_map(|it| matches!(it.shape, Shape::Zoom { .. }).then_some(it).zip(it.span)).collect();
    zooms.sort_by_key(|(_, s)| s.0);
    let mut cam = Cam::full(fw, fh);
    for (it, span) in zooms {
        let k = zoom_weight(span, t);
        if k <= 0.0 {
            continue;
        }
        if let Shape::Zoom { a, b } = it.shape {
            cam = cam.lerp(zoom_rect(a, b, fw, fh), k);
        }
    }
    cam
}

/// Opacity of an item at `t` (fades in and out at the ends of its span).
pub fn alpha(it: &Item, t: u32) -> f32 {
    match it.span {
        None => 1.0,
        Some((a, b)) => {
            if t < a || t >= b {
                return 0.0;
            }
            let f = FADE_MS.min((b - a) / 3).max(1) as f32;
            ((t - a) as f32 / f).min((b - t) as f32 / f).min(1.0)
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
/// Annotations on screen at `t` with their opacity and step number. Steps are
/// numbered by their order in the whole project, so numbers never jump.
pub fn frame_items(items: &[Item], t: u32) -> Vec<(Item, f32, usize)> {
    let mut step = 0;
    let mut out = Vec::new();
    for it in items {
        if matches!(it.shape, Shape::Step { .. }) {
            step += 1;
        }
        if matches!(it.shape, Shape::Zoom { .. }) {
            continue;
        }
        let a = alpha(it, t);
        if a > 0.0 {
            out.push((it.clone(), a, step));
        }
    }
    out
}

/// Maps an item from source pixels into an output picture that shows `cam`
/// at `s` output pixels per source pixel.
pub fn transform_item(it: &Item, cam: Cam, s: f32) -> Item {
    let m = |p: P| P::new((p.x - cam.x) * s, (p.y - cam.y) * s);
    let mut o = it.clone();
    o.width *= s;
    match &mut o.shape {
        Shape::Arrow { a, b } | Shape::Line { a, b } | Shape::Rect { a, b } | Shape::Ellipse { a, b } | Shape::Blur { a, b, .. } | Shape::Zoom { a, b } => {
            *a = m(*a);
            *b = m(*b);
        }
        Shape::Pen { pts } | Shape::Highlighter { pts } => pts.iter_mut().for_each(|p| *p = m(*p)),
        Shape::Text { at, size, .. } => {
            *at = m(*at);
            *size *= s;
        }
        Shape::Step { c, r } => {
            *c = m(*c);
            *r *= s;
        }
    }
    o
}

/// Copies part of a BGRA frame into an opaque RGBA pixmap.
fn pixmap_of(src: &Frame, r: Rect) -> Option<Pixmap> {
    let (w, h) = (r.width().max(1) as u32, r.height().max(1) as u32);
    let mut p = Pixmap::new(w, h)?;
    let stride = src.stride();
    let d = p.data_mut();
    for y in 0..h as usize {
        let s0 = (r.top as usize + y) * stride + r.left as usize * 4;
        let row = &src.pixels[s0..s0 + w as usize * 4];
        let o = &mut d[y * w as usize * 4..(y + 1) * w as usize * 4];
        for (oc, ic) in o.chunks_exact_mut(4).zip(row.chunks_exact(4)) {
            oc.copy_from_slice(&[ic[2], ic[1], ic[0], 255]);
        }
    }
    Some(p)
}

/// Halves a frame with a 2x2 box filter (used before big downscales).
fn halve(src: &Frame) -> Frame {
    let (w, h) = (src.width() / 2, src.height() / 2);
    let st = src.stride();
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let (r0, r1) = (&src.pixels[2 * y * st..], &src.pixels[(2 * y + 1) * st..]);
        let o = &mut out[y * w * 4..(y + 1) * w * 4];
        for x in 0..w {
            for c in 0..4 {
                let i = 8 * x + c;
                o[4 * x + c] = ((r0[i] as u32 + r0[i + 4] as u32 + r1[i] as u32 + r1[i + 4] as u32 + 2) >> 2) as u8;
            }
        }
    }
    Frame { rect: Rect::from_xywh(0, 0, w as i32, h as i32), pixels: out }
}

/// Bilinear resample of the `cam` area of `src` into `ow x oh` (BGRA),
/// split across threads by rows.
pub fn resample(src: &Frame, cam: Cam, ow: u32, oh: u32) -> Frame {
    let (ow, oh) = (ow.max(1) as usize, oh.max(1) as usize);
    // Big shrink: box-halve first so thin text doesn't alias.
    if (ow as f32) < cam.w * 0.55 && src.width() >= 4 && src.height() >= 4 {
        let half = halve(src);
        let c = Cam { x: cam.x / 2.0, y: cam.y / 2.0, w: cam.w / 2.0, h: cam.h / 2.0 };
        return resample(&half, c, ow as u32, oh as u32);
    }
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let axis = |n: usize, start: f32, len: f32, max: i32| -> Vec<(u32, u32, u32)> {
        (0..n)
            .map(|i| {
                let u = start + (i as f32 + 0.5) * len / n as f32 - 0.5;
                let u = u.clamp(0.0, (max - 1) as f32);
                let i0 = u.floor() as i32;
                let i1 = (i0 + 1).min(max - 1);
                (i0 as u32, i1 as u32, ((u - i0 as f32) * 256.0) as u32)
            })
            .collect()
    };
    let xs = axis(ow, cam.x, cam.w, sw);
    let ys = axis(oh, cam.y, cam.h, sh);
    let st = src.stride();
    let mut out = vec![0u8; ow * oh * 4];
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
    let rows_per = oh.div_ceil(threads);
    std::thread::scope(|sc| {
        for (k, chunk) in out.chunks_mut(rows_per * ow * 4).enumerate() {
            let (xs, ys) = (&xs, &ys);
            let px = &src.pixels;
            sc.spawn(move || {
                for (ry, orow) in chunk.chunks_exact_mut(ow * 4).enumerate() {
                    let (y0, y1, fy) = ys[k * rows_per + ry];
                    let (r0, r1) = (&px[y0 as usize * st..], &px[y1 as usize * st..]);
                    for (x, o) in orow.chunks_exact_mut(4).enumerate() {
                        let (x0, x1, fx) = xs[x];
                        let (a, b) = (x0 as usize * 4, x1 as usize * 4);
                        for c in 0..3 {
                            let top = r0[a + c] as u32 * (256 - fx) + r0[b + c] as u32 * fx;
                            let bot = r1[a + c] as u32 * (256 - fx) + r1[b + c] as u32 * fx;
                            o[c] = ((top * (256 - fy) + bot * fy + 32768) >> 16) as u8;
                        }
                        o[3] = 255;
                    }
                }
            });
        }
    });
    Frame { rect: Rect::from_xywh(0, 0, ow as i32, oh as i32), pixels: out }
}

/// Draws one (already transformed) item into the frame, touching only the
/// pixels around it; `a` < 1 mixes it with what was there.
fn draw_into(out: &mut Frame, it: &Item, a: f32, step: usize, s: f32, fonts: Option<&Fonts>) {
    let (l, tp, r, b) = bbox(it, fonts);
    let pad = 14.0 * s + 10.0;
    let area = Rect::new((l - pad).floor() as i32, (tp - pad).floor() as i32, (r + pad).ceil() as i32, (b + pad).ceil() as i32)
        .intersect(&Rect::from_xywh(0, 0, out.width() as i32, out.height() as i32));
    if area.width() <= 0 || area.height() <= 0 {
        return;
    }
    let Some(mut pm) = pixmap_of(out, area) else { return };
    let mut local = it.clone();
    local.translate(-area.left as f32, -area.top as f32);
    draw_item(&mut pm, &local, fonts, step, s);
    let k = (a.clamp(0.0, 1.0) * 256.0) as u32;
    let st = out.stride();
    let w = area.width() as usize;
    let d = pm.data();
    for y in 0..area.height() as usize {
        let o = &mut out.pixels[(area.top as usize + y) * st + area.left as usize * 4..][..w * 4];
        let n = &d[y * w * 4..(y + 1) * w * 4];
        for (oc, nc) in o.chunks_exact_mut(4).zip(n.chunks_exact(4)) {
            for (c, nci) in [(0, 2), (1, 1), (2, 0)] {
                oc[c] = ((oc[c] as u32 * (256 - k) + nc[nci] as u32 * k) >> 8) as u8;
            }
        }
    }
}

/// Items whose look doesn't depend on the picture under them can be drawn
/// once onto a transparent layer and reused while the camera holds still.
fn layer_cacheable(it: &Item) -> bool {
    !matches!(it.shape, Shape::Blur { .. } | Shape::Highlighter { .. } | Shape::Zoom { .. })
}

struct Layer {
    /// Top-left in scaled-source space (source px * s).
    x: i32,
    y: i32,
    pm: Pixmap,
}

/// Composes frames, keeping drawn annotation layers between frames.
#[derive(Default)]
pub struct Composer {
    layers: std::collections::HashMap<(usize, u64), Layer>,
}

fn layer_key(i: usize, it: &Item, s: f32) -> (usize, u64) {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{it:?}").hash(&mut h);
    ((s * 1000.0).round() as i64).hash(&mut h);
    (i, h.finish())
}

impl Composer {
    /// One frame of the edited video at output time `t`: the source frame
    /// seen through the camera, scaled to `ow x oh`, with annotations on top.
    pub fn compose(&mut self, src: &Frame, items: &[Item], t: u32, ow: u32, oh: u32, fonts: Option<&Fonts>) -> Frame {
        let (fw, fh) = (src.width() as f32, src.height() as f32);
        let cam = camera(items, t, fw, fh);
        let mut out = if cam.is_full(fw, fh) && ow as usize == src.width() && oh as usize == src.height() {
            let mut f = src.clone();
            f.rect = Rect::from_xywh(0, 0, ow as i32, oh as i32);
            f
        } else {
            resample(src, cam, ow, oh)
        };
        let s = (ow as f32 / cam.w + oh as f32 / cam.h) / 2.0;
        let mut used = std::collections::HashSet::new();
        let mut step = 0;
        for (i, it) in items.iter().enumerate() {
            if matches!(it.shape, Shape::Step { .. }) {
                step += 1;
            }
            if matches!(it.shape, Shape::Zoom { .. }) {
                continue;
            }
            let a = alpha(it, t);
            if a <= 0.0 {
                continue;
            }
            if !layer_cacheable(it) {
                draw_into(&mut out, &transform_item(it, cam, s), a, step, s, fonts);
                continue;
            }
            let key = layer_key(i, it, s);
            used.insert(key);
            let layer = self.layers.entry(key).or_insert_with(|| {
                // Draw in scaled-source space, no camera offset.
                let scaled = transform_item(it, Cam { x: 0.0, y: 0.0, w: fw, h: fh }, s);
                let (l, tp, r, b) = bbox(&scaled, fonts);
                let pad = 14.0 * s + 10.0;
                let (x, y) = ((l - pad).floor() as i32, (tp - pad).floor() as i32);
                let (w, h) = (((r + pad).ceil() as i32 - x).clamp(1, 8192), ((b + pad).ceil() as i32 - y).clamp(1, 8192));
                let mut pm = Pixmap::new(w as u32, h as u32).expect("layer");
                let mut local = scaled;
                local.translate(-x as f32, -y as f32);
                draw_item(&mut pm, &local, fonts, step, s);
                Layer { x, y, pm }
            });
            let (dx, dy) = (layer.x - (cam.x * s).round() as i32, layer.y - (cam.y * s).round() as i32);
            blend_layer(&mut out, &layer.pm, dx, dy, a);
        }
        self.layers.retain(|k, _| used.contains(k));
        out
    }
}

/// Premultiplied RGBA layer over a BGRA frame at (dx, dy), times `a`.
fn blend_layer(out: &mut Frame, pm: &Pixmap, dx: i32, dy: i32, a: f32) {
    let (fw, fh) = (out.width() as i32, out.height() as i32);
    let (lw, lh) = (pm.width() as i32, pm.height() as i32);
    let (x0, y0) = (dx.max(0), dy.max(0));
    let (x1, y1) = ((dx + lw).min(fw), (dy + lh).min(fh));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let k = (a.clamp(0.0, 1.0) * 256.0) as u32;
    let st = out.stride();
    let d = pm.data();
    for y in y0..y1 {
        let lrow = &d[((y - dy) * lw + (x0 - dx)) as usize * 4..((y - dy) * lw + (x1 - dx)) as usize * 4];
        let orow = &mut out.pixels[y as usize * st + x0 as usize * 4..y as usize * st + x1 as usize * 4];
        for (o, l) in orow.chunks_exact_mut(4).zip(lrow.chunks_exact(4)) {
            let la = l[3] as u32 * k >> 8;
            if la == 0 {
                continue;
            }
            for (c, li) in [(0, 2), (1, 1), (2, 0)] {
                let lc = l[li] as u32 * k >> 8;
                o[c] = (lc + (o[c] as u32 * (255 - la) + 127) / 255).min(255) as u8;
            }
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
/// One-off composition (no layer reuse).
pub fn compose(src: &Frame, items: &[Item], t: u32, ow: u32, oh: u32, fonts: Option<&Fonts>) -> Frame {
    Composer::default().compose(src, items, t, ow, oh, fonts)
}

/// Biggest |sample| per bucket of `bucket` frames, 0..=255, mixed to mono.
pub fn peaks(pcm: &[i16], channels: usize, bucket: usize) -> Vec<u8> {
    let ch = channels.max(1);
    pcm.chunks(bucket.max(1) * ch)
        .map(|c| {
            let m = c.iter().map(|s| (*s as i32).unsigned_abs()).max().unwrap_or(0);
            ((m * 255) / 32768).min(255) as u8
        })
        .collect()
}

/// Short fades at both ends of a PCM block so cuts don't click.
pub fn edge_fades(pcm: &mut [i16], channels: usize, fade_frames: usize) {
    let ch = channels.max(1);
    let frames = pcm.len() / ch;
    let n = fade_frames.min(frames / 2);
    for i in 0..n {
        let g = i as f32 / n as f32;
        for c in 0..ch {
            let a = i * ch + c;
            let b = (frames - 1 - i) * ch + c;
            pcm[a] = (pcm[a] as f32 * g) as i16;
            pcm[b] = (pcm[b] as f32 * g) as i16;
        }
    }
}

/// `<stem> edit.mp4` next to the source, or `<stem> edit (n).mp4` if taken.
pub fn edited_path(src: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let dir = src.parent().unwrap_or(Path::new("."));
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Video".into());
    let stem = stem.strip_suffix(" edit").unwrap_or(&stem).to_string();
    let first = dir.join(format!("{stem} edit.mp4"));
    if !exists(&first) {
        return first;
    }
    (2..).map(|n| dir.join(format!("{stem} edit ({n}).mp4"))).find(|p| !exists(p)).unwrap()
}

/// "1:05.3" style time for the editor.
pub fn clock(ms: u32) -> String {
    let s = ms / 1000;
    format!("{}:{:02}.{}", s / 60, s % 60, (ms % 1000) / 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annot::{Color, PALETTE};

    fn tl(ms: u32) -> Timeline {
        Timeline::new(ms)
    }

    #[test]
    fn split_delete_move_trim() {
        let mut t = tl(10_000);
        assert!(t.split(3000));
        assert!(t.split(7000));
        assert_eq!(t.clips, vec![Clip { a: 0, b: 3000 }, Clip { a: 3000, b: 7000 }, Clip { a: 7000, b: 10_000 }]);
        assert!(!t.split(3050), "too close to a cut");
        assert!(t.delete(1));
        assert_eq!(t.total(), 6000);
        assert_eq!(t.locate(3500), (1, 7500));
        assert!(t.move_clip(1, 0));
        assert_eq!(t.clips[0], Clip { a: 7000, b: 10_000 });
        assert_eq!(t.locate(0), (0, 7000));
        assert_eq!(t.locate(3000), (1, 0));
        assert!(t.trim(1, 500, 2500));
        assert_eq!(t.total(), 5000);
        assert!(t.trim(1, 2400, 2410));
        assert_eq!(t.clips[1].len(), MIN_CLIP_MS);
        assert!(t.delete(0));
        assert!(!t.delete(0), "last clip stays");
    }

    #[test]
    fn locate_end_shows_last_frame() {
        let t = tl(2000);
        assert_eq!(t.locate(2000), (0, 1999));
        assert_eq!(t.index_at(2000), None);
    }

    fn item(shape: Shape, span: (u32, u32)) -> Item {
        Item { shape, color: PALETTE[0], width: 5.0, span: Some(span) }
    }

    #[test]
    fn zoom_eases_in_holds_and_eases_out() {
        let z = item(Shape::Zoom { a: P::new(100.0, 100.0), b: P::new(300.0, 200.0) }, (1000, 4000));
        let (fw, fh) = (1600.0, 900.0);
        assert!(camera(&[z.clone()], 500, fw, fh).is_full(fw, fh));
        let mid = camera(&[z.clone()], 2500, fw, fh);
        let r = zoom_rect(P::new(100.0, 100.0), P::new(300.0, 200.0), fw, fh);
        assert_eq!(mid, r);
        assert!((r.w / r.h - fw / fh).abs() < 0.01, "aspect kept");
        let half = camera(&[z.clone()], 1000 + ZOOM_EASE_MS / 2, fw, fh);
        assert!(half.w < fw && half.w > r.w, "{half:?}");
        assert!(camera(&[z], 4000, fw, fh).is_full(fw, fh));
    }

    #[test]
    fn zoom_rect_is_clamped_and_not_tiny() {
        let r = zoom_rect(P::new(1590.0, 890.0), P::new(1595.0, 895.0), 1600.0, 900.0);
        assert!(r.w >= 1600.0 * MIN_ZOOM_FRAC - 0.01);
        assert!(r.x + r.w <= 1600.01 && r.y + r.h <= 900.01);
    }

    #[test]
    fn items_fade_and_steps_keep_numbers() {
        let s1 = item(Shape::Step { c: P::new(10.0, 10.0), r: 10.0 }, (0, 1000));
        let s2 = item(Shape::Step { c: P::new(40.0, 10.0), r: 10.0 }, (2000, 3000));
        let v = frame_items(&[s1.clone(), s2.clone()], 2500);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].2, 2, "second step stays number 2 when the first is gone");
        assert!(alpha(&s1, 0) < 0.01 && (alpha(&s1, 500) - 1.0).abs() < 1e-6 && alpha(&s1, 1000) == 0.0);
    }

    fn split_frame(w: i32, h: i32) -> Frame {
        // Left half red, right half blue (BGRA).
        let mut pixels = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(if x < w / 2 { &[0, 0, 255, 255] } else { &[255, 0, 0, 255] });
            }
        }
        Frame { rect: Rect::from_xywh(0, 0, w, h), pixels }
    }

    #[test]
    fn compose_passes_through_and_zooms() {
        let f = split_frame(320, 180);
        let same = compose(&f, &[], 0, 320, 180, None);
        assert_eq!(same.pixels, f.pixels);
        // 50x50 drawn, grows to 16:9 around its centre: still all blue.
        let z = item(Shape::Zoom { a: P::new(240.0, 60.0), b: P::new(290.0, 110.0) }, (0, 10_000));
        let out = compose(&f, &[z], 5000, 160, 90, None);
        let blue = out.pixels.chunks_exact(4).filter(|p| p[0] > 200 && p[2] < 50).count();
        assert!(blue > 160 * 90 * 98 / 100, "zoomed into the blue half: {blue}");
        let small = compose(&f, &[], 0, 80, 45, None);
        assert_eq!((small.width(), small.height()), (80, 45));
    }

    #[test]
    fn compose_draws_items_scaled_and_faded() {
        let f = split_frame(200, 100);
        let rect = Item { shape: Shape::Rect { a: P::new(20.0, 20.0), b: P::new(80.0, 80.0) }, color: Color(0, 255, 0), width: 6.0, span: Some((0, 10_000)) };
        let full = compose(&f, &[rect.clone()], 5000, 100, 50, None);
        let green = |fr: &Frame| fr.pixels.chunks_exact(4).filter(|p| p[1] > 200 && p[0] < 80 && p[2] < 80).count();
        assert!(green(&full) > 100, "{}", green(&full));
        let fading = compose(&f, &[rect], 40, 100, 50, None);
        assert!(green(&fading) < green(&full) / 2, "mid-fade is fainter");
    }

    #[test]
    fn audio_helpers() {
        let mut pcm = vec![10_000i16; 2 * 100];
        edge_fades(&mut pcm, 2, 10);
        assert_eq!(pcm[0], 0);
        assert_eq!(pcm[199], 0);
        assert_eq!(pcm[100], 10_000);
        let p = peaks(&[0, 0, 32767, -32768, 100, 100], 2, 1);
        assert_eq!(p, vec![0, 255, 0]);
    }

    #[test]
    fn edited_names_do_not_overwrite() {
        let src = Path::new("C:/v/Video 1.mp4");
        assert_eq!(edited_path(src, |_| false), Path::new("C:/v/Video 1 edit.mp4"));
        let taken = [PathBuf::from("C:/v/Video 1 edit.mp4")];
        assert_eq!(edited_path(src, |p| taken.iter().any(|t| t == p)), Path::new("C:/v/Video 1 edit (2).mp4"));
        assert_eq!(edited_path(Path::new("C:/v/Video 1 edit.mp4"), |_| false), Path::new("C:/v/Video 1 edit.mp4"));
        assert_eq!(clock(65_340), "1:05.3");
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use crate::annot::{Color, PALETTE};

    #[test]
    #[ignore]
    fn compose_speed() {
        let (w, h) = (1920, 1080);
        let f = Frame { rect: Rect::from_xywh(0, 0, w, h), pixels: vec![128; (w * h * 4) as usize] };
        let fonts = crate::annot::font::shared();
        let items = vec![
            Item { shape: Shape::Arrow { a: P::new(100.0, 100.0), b: P::new(600.0, 400.0) }, color: PALETTE[0], width: 6.0, span: Some((0, 10_000)) },
            Item { shape: Shape::Text { at: P::new(200.0, 600.0), text: "Hello there".into(), size: 40.0 }, color: Color(255, 0, 0), width: 0.0, span: Some((0, 10_000)) },
            Item { shape: Shape::Zoom { a: P::new(400.0, 200.0), b: P::new(1200.0, 700.0) }, color: PALETTE[0], width: 0.0, span: Some((0, 10_000)) },
        ];
        let only = |k: usize| vec![items[k].clone()];
        for (set, label) in [(only(0), "arrow"), (only(1), "text"), (only(2), "zoom")] {
            let t0 = std::time::Instant::now();
            for i in 0..10 {
                std::hint::black_box(compose(&f, &set, 2000 + i, 1920, 1080, fonts.as_deref()));
            }
            eprintln!("{label}: {:.1} ms/frame", t0.elapsed().as_secs_f64() * 1000.0 / 10.0);
        }
        for (ow, oh, name) in [(1920, 1080, "export"), (960, 540, "preview")] {
            let mut c = Composer::default();
            let t0 = std::time::Instant::now();
            for i in 0..20 {
                std::hint::black_box(c.compose(&f, &items, 2000 + i, ow, oh, fonts.as_deref()));
            }
            eprintln!("{name}: {:.1} ms/frame", t0.elapsed().as_secs_f64() * 1000.0 / 20.0);
        }
    }
}

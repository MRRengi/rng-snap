//! Cut out: lifts the object under the cursor off a picture, like Apple's
//! "Lift subject". MobileSAM runs locally through ONNX Runtime: the encoder
//! looks at the whole picture once, then the small decoder answers every
//! hover and click in a few milliseconds.
//!
//! Everything except `Engine` is plain pixel maths and is unit-tested.

#![allow(dead_code)]

use std::path::Path;
use std::sync::{Mutex, OnceLock};

/// The encoder's input is a 1024x1024 square.
pub const SIDE: usize = 1024;
/// The decoder's low-resolution masks are 256x256.
pub const LOW: usize = 256;
const MEAN: [f32; 3] = [123.675, 116.28, 103.53];
const STD: [f32; 3] = [58.395, 57.12, 57.375];

/// The picture as the encoder wants it.
pub struct Prepared {
    /// CHW, RGB, normalized, padded to SIDE x SIDE.
    pub data: Vec<f32>,
    /// Picture pixels to encoder pixels.
    pub scale: f32,
    pub w: usize,
    pub h: usize,
}

/// Resizes the longest side to 1024 (bilinear), normalizes and pads, from
/// top-down BGRA.
pub fn prepare(bgra: &[u8], w: usize, h: usize) -> Prepared {
    let scale = SIDE as f32 / w.max(h).max(1) as f32;
    let nw = ((w as f32 * scale).round() as usize).clamp(1, SIDE);
    let nh = ((h as f32 * scale).round() as usize).clamp(1, SIDE);
    let mut data = vec![0f32; 3 * SIDE * SIDE];
    let px = |x: usize, y: usize, c: usize| bgra[(y * w + x) * 4 + (2 - c)] as f32;
    for y in 0..nh {
        let sy = ((y as f32 + 0.5) * h as f32 / nh as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (y0, fy) = (sy.floor() as usize, sy.fract());
        let y1 = (y0 + 1).min(h - 1);
        for x in 0..nw {
            let sx = ((x as f32 + 0.5) * w as f32 / nw as f32 - 0.5).clamp(0.0, (w - 1) as f32);
            let (x0, fx) = (sx.floor() as usize, sx.fract());
            let x1 = (x0 + 1).min(w - 1);
            for c in 0..3 {
                let top = px(x0, y0, c) * (1.0 - fx) + px(x1, y0, c) * fx;
                let bot = px(x0, y1, c) * (1.0 - fx) + px(x1, y1, c) * fx;
                let v = top * (1.0 - fy) + bot * fy;
                data[c * SIDE * SIDE + y * SIDE + x] = (v - MEAN[c]) / STD[c];
            }
        }
    }
    Prepared { data, scale, w, h }
}

/// What the decoder said about one prompt.
pub struct Masks {
    /// 4 masks of LOW x LOW logits (0 = single answer, 1..3 = small to large).
    pub low: Vec<f32>,
    pub iou: [f32; 4],
}

impl Masks {
    pub fn mask(&self, i: usize) -> &[f32] {
        &self.low[i * LOW * LOW..(i + 1) * LOW * LOW]
    }
}

/// How settled a mask is: area above +1 over area above -1 (SAM's
/// "stability score"). Blotchy, unsure masks score low.
pub fn stability(low: &[f32]) -> f32 {
    let hi = low.iter().filter(|&&l| l > 1.0).count();
    let lo = low.iter().filter(|&&l| l > -1.0).count();
    if lo == 0 { 0.0 } else { hi as f32 / lo as f32 }
}

/// Picks the mask that best matches "the object": the largest of the three
/// granularities that is about as confident as the best one and settled
/// (so a click on a shirt lifts the person, but not a blotchy background).
pub fn choose(m: &Masks) -> usize {
    let iou = &m.iou;
    let best = (1..4).fold(1, |b, i| if iou[i] > iou[b] { i } else { b });
    (best..4).rev().find(|&i| iou[i] >= iou[best] - 0.015 && (i == best || stability(m.mask(i)) >= 0.92)).unwrap_or(best)
}

/// Keeps only the parts of the mask that touch one of the clicked points
/// (drops stray specks elsewhere) and fills small holes inside.
pub fn tidy(alpha: &mut [u8], w: usize, h: usize, seeds: &[(usize, usize)]) {
    let solid = |a: u8| a >= 128;
    // Connected parts of the mask; keep the ones under a clicked point and
    // any big ones (a click can land on a strap of a person), drop specks.
    let mut comp = vec![u32::MAX; w * h];
    let mut sizes: Vec<usize> = Vec::new();
    for start in 0..w * h {
        if comp[start] != u32::MAX || !solid(alpha[start]) {
            continue;
        }
        let id = sizes.len() as u32;
        let mut stack = vec![start];
        comp[start] = id;
        let mut n = 0;
        while let Some(i) = stack.pop() {
            n += 1;
            let (x, y) = (i % w, i / w);
            let mut go = |j: usize| {
                if comp[j] == u32::MAX && solid(alpha[j]) {
                    comp[j] = id;
                    stack.push(j);
                }
            };
            if x > 0 {
                go(i - 1);
            }
            if x + 1 < w {
                go(i + 1);
            }
            if y > 0 {
                go(i - w);
            }
            if y + 1 < h {
                go(i + w);
            }
        }
        sizes.push(n);
    }
    if sizes.is_empty() {
        return;
    }
    let total: usize = sizes.iter().sum();
    let mut wanted: Vec<bool> = sizes.iter().map(|&n| n * 20 >= total).collect();
    let mut seeded = false;
    for &(x, y) in seeds {
        if x < w && y < h && comp[y * w + x] != u32::MAX {
            wanted[comp[y * w + x] as usize] = true;
            seeded = true;
        }
    }
    if !seeded && seeds.iter().any(|&(x, y)| x < w && y < h) {
        // The click is just outside the mask edge: keep the biggest part only.
        let big = (0..sizes.len()).max_by_key(|&i| sizes[i]).unwrap_or(0);
        wanted = vec![false; sizes.len()];
        wanted[big] = true;
    }
    let keep: Vec<bool> = comp.iter().map(|&c| c != u32::MAX && wanted[c as usize]).collect();
    let mut keep = keep;
    // Background = not kept and reachable from the border; holes are the rest.
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = Vec::new();
    for x in 0..w {
        stack.push(x);
        stack.push((h - 1) * w + x);
    }
    for y in 0..h {
        stack.push(y * w);
        stack.push(y * w + w - 1);
    }
    while let Some(i) = stack.pop() {
        if outside[i] || keep[i] {
            continue;
        }
        outside[i] = true;
        let (x, y) = (i % w, i / w);
        if x > 0 {
            stack.push(i - 1);
        }
        if x + 1 < w {
            stack.push(i + 1);
        }
        if y > 0 {
            stack.push(i - w);
        }
        if y + 1 < h {
            stack.push(i + w);
        }
    }
    let area = keep.iter().filter(|k| **k).count();
    let max_hole = area / 50;
    // Measure each hole; fill the small ones.
    let mut seen = vec![false; w * h];
    for start in 0..w * h {
        if keep[start] || outside[start] || seen[start] {
            continue;
        }
        let mut hole = vec![start];
        let mut k = 0;
        seen[start] = true;
        while k < hole.len() {
            let i = hole[k];
            k += 1;
            let (x, y) = (i % w, i / w);
            for j in [i.wrapping_sub(1), i + 1, i.wrapping_sub(w), i + w] {
                let ok = j < w * h && ((j == i.wrapping_sub(1) && x > 0) || (j == i + 1 && x + 1 < w) || (j == i.wrapping_sub(w) && y > 0) || (j == i + w && y + 1 < h));
                if ok && !keep[j] && !outside[j] && !seen[j] {
                    seen[j] = true;
                    hole.push(j);
                }
            }
        }
        if hole.len() <= max_hole {
            for i in hole {
                keep[i] = true;
            }
        }
    }
    // Pixels next to a kept pixel keep their soft edge; everything else goes.
    for i in 0..w * h {
        if keep[i] {
            alpha[i] = alpha[i].max(if solid(alpha[i]) { alpha[i] } else { 255 });
            continue;
        }
        let (x, y) = (i % w, i / w);
        let near = (x > 0 && keep[i - 1]) || (x + 1 < w && keep[i + 1]) || (y > 0 && keep[i - w]) || (y + 1 < h && keep[i + w]);
        if !near {
            alpha[i] = 0;
        }
    }
}

/// Logits of one low-res mask, scaled to the picture (w x h), the way SAM's
/// own post-processing does (up to the padded square, crop, down to size).
pub fn upscale(low: &[f32], scale: f32, w: usize, h: usize) -> Vec<f32> {
    let nw = (w as f32 * scale).round().max(1.0);
    let nh = (h as f32 * scale).round().max(1.0);
    let k = LOW as f32 / SIDE as f32;
    let mut out = vec![0f32; w * h];
    let sample = |x: f32, y: f32| {
        let x = x.clamp(0.0, (LOW - 1) as f32);
        let y = y.clamp(0.0, (LOW - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(LOW - 1), (y0 + 1).min(LOW - 1));
        let (fx, fy) = (x.fract(), y.fract());
        let t = low[y0 * LOW + x0] * (1.0 - fx) + low[y0 * LOW + x1] * fx;
        let b = low[y1 * LOW + x0] * (1.0 - fx) + low[y1 * LOW + x1] * fx;
        t * (1.0 - fy) + b * fy
    };
    for y in 0..h {
        // Picture pixel -> encoder pixel -> low-res pixel (pixel centres).
        let ey = (y as f32 + 0.5) * nh / h as f32;
        let ly = ey * k - 0.5;
        for x in 0..w {
            let ex = (x as f32 + 0.5) * nw / w as f32;
            out[y * w + x] = sample(ex * k - 0.5, ly);
        }
    }
    out
}

/// Soft alpha from logits: 0 is the edge, a few logits either side fade.
pub fn alpha(logits: &[f32]) -> Vec<u8> {
    logits.iter().map(|&l| ((0.5 + l * 0.8).clamp(0.0, 1.0) * 255.0).round() as u8).collect()
}

/// Union of two alpha masks.
pub fn union(a: &mut [u8], b: &[u8]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x = (*x).max(*y);
    }
}

/// Bounding box (x0, y0, x1, y1), exclusive, of everything half-opaque or more.
pub fn bbox(alpha: &[u8], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if alpha[y * w + x] >= 128 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

/// A picture with straight (not premultiplied) alpha, RGBA, top-down.
#[derive(Clone)]
pub struct Rgba {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Rgba {
    pub fn encode_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.w as u32, self.h as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Fast);
            let mut wr = enc.write_header().expect("png header");
            wr.write_image_data(&self.px).expect("png data");
        }
        out
    }

    /// Premultiplied BGRA (for Direct2D and DIBV5 previews).
    pub fn premul_bgra(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.px.len());
        for p in self.px.chunks_exact(4) {
            let a = p[3] as u32;
            let m = |c: u8| ((c as u32 * a + 127) / 255) as u8;
            v.extend_from_slice(&[m(p[2]), m(p[1]), m(p[0]), p[3]]);
        }
        v
    }
}

/// The object on transparency, cropped to it with `margin` pixels around.
pub fn cut(bgra: &[u8], w: usize, h: usize, alpha: &[u8], margin: usize) -> Option<Rgba> {
    let (x0, y0, x1, y1) = bbox(alpha, w, h)?;
    let (x0, y0) = (x0.saturating_sub(margin), y0.saturating_sub(margin));
    let (x1, y1) = ((x1 + margin).min(w), (y1 + margin).min(h));
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut px = Vec::with_capacity(cw * ch * 4);
    for y in y0..y1 {
        for x in x0..x1 {
            let i = (y * w + x) * 4;
            px.extend_from_slice(&[bgra[i + 2], bgra[i + 1], bgra[i], alpha[y * w + x]]);
        }
    }
    Some(Rgba { w: cw, h: ch, px })
}

/// Distance (in pixels, chamfer 3-4) from every pixel to the nearest pixel
/// that is at least half opaque.
fn distance(alpha: &[u8], w: usize, h: usize) -> Vec<f32> {
    let inf = 1e9f32;
    let mut d: Vec<f32> = alpha.iter().map(|&a| if a >= 128 { 0.0 } else { inf }).collect();
    let (a, b) = (1.0f32, 1.4142f32);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let mut v = d[i];
            if x > 0 {
                v = v.min(d[i - 1] + a);
            }
            if y > 0 {
                v = v.min(d[i - w] + a);
                if x > 0 {
                    v = v.min(d[i - w - 1] + b);
                }
                if x + 1 < w {
                    v = v.min(d[i - w + 1] + b);
                }
            }
            d[i] = v;
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            let mut v = d[i];
            if x + 1 < w {
                v = v.min(d[i + 1] + a);
            }
            if y + 1 < h {
                v = v.min(d[i + w] + a);
                if x + 1 < w {
                    v = v.min(d[i + w + 1] + b);
                }
                if x > 0 {
                    v = v.min(d[i + w - 1] + b);
                }
            }
            d[i] = v;
        }
    }
    d
}

/// Three box-blur passes (close to a Gaussian) of a single channel.
fn blur(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut a = src.to_vec();
    let mut b = vec![0f32; a.len()];
    for _ in 0..3 {
        for y in 0..h {
            let mut acc = 0f32;
            for x in 0..(r + 1).min(w) {
                acc += a[y * w + x];
            }
            for x in 0..w {
                let lo = x.saturating_sub(r);
                let hi = (x + r).min(w - 1);
                b[y * w + x] = acc / (hi - lo + 1) as f32;
                if x + r + 1 < w {
                    acc += a[y * w + x + r + 1];
                }
                if x >= r {
                    acc -= a[y * w + x - r];
                }
            }
        }
        for x in 0..w {
            let mut acc = 0f32;
            for y in 0..(r + 1).min(h) {
                acc += b[y * w + x];
            }
            for y in 0..h {
                let lo = y.saturating_sub(r);
                let hi = (y + r).min(h - 1);
                a[y * w + x] = acc / (hi - lo + 1) as f32;
                if y + r + 1 < h {
                    acc += b[(y + r + 1) * w + x];
                }
                if y >= r {
                    acc -= b[(y - r) * w + x];
                }
            }
        }
    }
    a
}

/// iMessage-style sticker: a white outline around the object and a soft
/// shadow under it, on a canvas grown to fit both.
pub fn sticker(src: &Rgba) -> Rgba {
    let size = src.w.max(src.h) as f32;
    let border = (size * 0.025).clamp(4.0, 18.0);
    let shadow_r = (border * 1.2).round() as usize;
    let pad = (border.ceil() as usize) + shadow_r * 2 + 2;
    let (w, h) = (src.w + pad * 2, src.h + pad * 2);
    let mut alpha = vec![0u8; w * h];
    for y in 0..src.h {
        for x in 0..src.w {
            alpha[(y + pad) * w + x + pad] = src.px[(y * src.w + x) * 4 + 3];
        }
    }
    let dist = distance(&alpha, w, h);
    // Outline coverage, anti-aliased over one pixel at its outer edge.
    let outline: Vec<f32> = dist.iter().map(|&d| (border + 0.5 - d).clamp(0.0, 1.0)).collect();
    let dy = (border * 0.5).round() as usize;
    let mut shifted = vec![0f32; w * h];
    for y in dy..h {
        shifted[y * w..(y + 1) * w].copy_from_slice(&outline[(y - dy) * w..(y - dy + 1) * w]);
    }
    let shadow = blur(&shifted, w, h, shadow_r.max(1));
    let mut px = vec![0u8; w * h * 4];
    for i in 0..w * h {
        // Shadow (black, 30%) under a white outline under the object.
        let sa = shadow[i] * 0.30;
        let (mut r, mut g, mut b, mut a) = (0f32, 0f32, 0f32, sa);
        let oa = outline[i];
        r = r * (1.0 - oa) + 255.0 * oa;
        g = g * (1.0 - oa) + 255.0 * oa;
        b = b * (1.0 - oa) + 255.0 * oa;
        a = oa + a * (1.0 - oa);
        let (x, y) = (i % w, i / w);
        if x >= pad && y >= pad && x < pad + src.w && y < pad + src.h {
            let s = &src.px[((y - pad) * src.w + x - pad) * 4..][..4];
            let ta = s[3] as f32 / 255.0;
            // Straight-alpha "over": blend colours weighted by coverage.
            let out_a = ta + a * (1.0 - ta);
            if out_a > 0.0 {
                let mix = |top: u8, under: f32| (top as f32 * ta + under * a * (1.0 - ta)) / out_a;
                r = mix(s[0], r);
                g = mix(s[1], g);
                b = mix(s[2], b);
            }
            a = out_a;
        } else if a > 0.0 && oa == 0.0 {
            // Pure shadow pixels: colour black, alpha as is.
            r = 0.0;
            g = 0.0;
            b = 0.0;
        }
        px[i * 4] = r.round().clamp(0.0, 255.0) as u8;
        px[i * 4 + 1] = g.round().clamp(0.0, 255.0) as u8;
        px[i * 4 + 2] = b.round().clamp(0.0, 255.0) as u8;
        px[i * 4 + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    Rgba { w, h, px }
}

/// 0..255 strength of the mask's edge band (for the hover glow): pixels
/// within `r` of the boundary, strongest on it.
pub fn edge(alpha: &[u8], w: usize, h: usize, r: f32) -> Vec<u8> {
    let inside = distance(alpha, w, h);
    let inv: Vec<u8> = alpha.iter().map(|&a| 255 - a).collect();
    let outside = distance(&inv, w, h);
    inside
        .iter()
        .zip(&outside)
        .map(|(&di, &dout)| {
            let d = di.max(dout);
            ((1.0 - d / r).clamp(0.0, 1.0) * 255.0) as u8
        })
        .collect()
}

// ---------------------------------------------------------------- models

pub const ENCODER: &str = "snap_cutout_encoder.onnx";
pub const DECODER: &str = "snap_cutout_decoder.onnx";

/// The two sessions. ONNX Runtime allows one environment per process, so the
/// engine is created once and shared.
pub struct Engine {
    enc: ort::session::Session,
    dec: ort::session::Session,
}

static ENGINE: OnceLock<Result<Mutex<Engine>, String>> = OnceLock::new();

/// Loads (once) the engine from `dir`, with the ONNX Runtime library `dylib`.
pub fn engine(dir: &Path, dylib: &Path) -> Result<&'static Mutex<Engine>, String> {
    ENGINE.get_or_init(|| Engine::load(dir, dylib).map(Mutex::new)).as_ref().map_err(|e| e.clone())
}

fn e<E: std::fmt::Display>(what: &str) -> impl Fn(E) -> String + '_ {
    move |err| format!("{what}: {err}")
}

impl Engine {
    fn load(dir: &Path, dylib: &Path) -> Result<Engine, String> {
        use ort::session::Session;
        use ort::session::builder::GraphOptimizationLevel;
        let env = ort::init_from(dylib).map_err(e("onnx runtime"))?.with_name("snap").build().map_err(e("onnx environment"))?;
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
        let mk = |file: &str| -> Result<Session, String> {
            Session::builder(&env)
                .map_err(e("session"))?
                .with_optimization_level(GraphOptimizationLevel::All)
                .map_err(e("optimization"))?
                .with_intra_threads(threads)
                .map_err(e("threads"))?
                .commit_from_file(dir.join(file))
                .map_err(e(file))
        };
        let enc = mk(ENCODER)?;
        let dec = mk(DECODER)?;
        // The environment must outlive the sessions; it lives for the process.
        std::mem::forget(env);
        Ok(Engine { enc, dec })
    }

    /// Image embedding (1 x 256 x 64 x 64).
    pub fn embed(&mut self, p: &Prepared) -> Result<Vec<f32>, String> {
        let t = ort::value::Tensor::from_array(([1usize, 3, SIDE, SIDE], p.data.clone())).map_err(e("input"))?;
        let out = self.enc.run(ort::inputs!["image" => t]).map_err(e("encoder"))?;
        let (_, v) = out["embeddings"].try_extract_tensor::<f32>().map_err(e("embeddings"))?;
        Ok(v.to_vec())
    }

    /// Masks for clicked points (picture coordinates; `true` = keep, `false` = leave out).
    pub fn predict(&mut self, emb: &[f32], scale: f32, points: &[(f32, f32, bool)]) -> Result<Masks, String> {
        let n = points.len() + 1;
        let mut coords = Vec::with_capacity(n * 2);
        let mut labels = Vec::with_capacity(n);
        for &(x, y, keep) in points {
            coords.extend_from_slice(&[x * scale, y * scale]);
            labels.push(if keep { 1.0f32 } else { 0.0 });
        }
        // No box prompt: SAM wants a padding point labelled -1.
        coords.extend_from_slice(&[0.0, 0.0]);
        labels.push(-1.0);
        let inputs = ort::inputs![
            "image_embeddings" => ort::value::Tensor::from_array(([1usize, 256, 64, 64], emb.to_vec())).map_err(e("embeddings"))?,
            "point_coords" => ort::value::Tensor::from_array(([1usize, n, 2], coords)).map_err(e("points"))?,
            "point_labels" => ort::value::Tensor::from_array(([1usize, n], labels)).map_err(e("labels"))?,
            "mask_input" => ort::value::Tensor::from_array(([1usize, 1, LOW, LOW], vec![0f32; LOW * LOW])).map_err(e("mask"))?,
            "has_mask_input" => ort::value::Tensor::from_array(([1usize], vec![0f32])).map_err(e("has mask"))?,
            "orig_im_size" => ort::value::Tensor::from_array(([2usize], vec![LOW as f32, LOW as f32])).map_err(e("size"))?,
        ];
        let out = self.dec.run(inputs).map_err(e("decoder"))?;
        let (_, low) = out["low_res_masks"].try_extract_tensor::<f32>().map_err(e("masks"))?;
        let (_, iou) = out["iou_predictions"].try_extract_tensor::<f32>().map_err(e("scores"))?;
        Ok(Masks { low: low.to_vec(), iou: [iou[0], iou[1], iou[2], iou[3]] })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, bgr: [u8; 3]) -> Vec<u8> {
        (0..w * h).flat_map(|_| [bgr[0], bgr[1], bgr[2], 255]).collect()
    }

    #[test]
    fn prepare_scales_normalizes_and_pads() {
        let (w, h) = (200, 100);
        let p = prepare(&solid(w, h, [0, 0, 255]), w, h);
        assert!((p.scale - 5.12).abs() < 1e-4);
        // Red channel inside the picture, zero padding below it.
        let r = p.data[100 * SIDE + 100];
        assert!((r - (255.0 - MEAN[0]) / STD[0]).abs() < 1e-3, "{r}");
        let b = p.data[2 * SIDE * SIDE + 100 * SIDE + 100];
        assert!((b - (0.0 - MEAN[2]) / STD[2]).abs() < 1e-3);
        assert_eq!(p.data[700 * SIDE + 100], 0.0);
    }

    fn masks(iou: [f32; 4], stable: [bool; 4]) -> Masks {
        let mut low = Vec::new();
        for s in stable {
            // Settled: all +5 or -5; blotchy: lots of values around 0.
            low.extend((0..LOW * LOW).map(|i| if i % 2 == 0 { 5.0 } else if s { -5.0 } else { 0.5 }));
        }
        Masks { low, iou }
    }

    #[test]
    fn choose_prefers_the_larger_settled_object_when_close() {
        let all = [true; 4];
        assert_eq!(choose(&masks([0.9, 0.95, 0.80, 0.70], all)), 1);
        assert_eq!(choose(&masks([0.9, 0.95, 0.94, 0.70], all)), 2);
        assert_eq!(choose(&masks([0.9, 0.80, 0.85, 0.99], all)), 3);
        // The larger one is as confident but blotchy: keep the smaller.
        assert_eq!(choose(&masks([0.9, 0.95, 0.945, 0.70], [true, true, false, true])), 1);
        assert!(stability(masks([0.0; 4], all).mask(1)) > 0.99);
    }

    #[test]
    fn tidy_drops_specks_and_fills_small_holes() {
        let (w, h) = (40, 40);
        let mut a = vec![0u8; w * h];
        for y in 5..30 {
            for x in 5..30 {
                a[y * w + x] = 255;
            }
        }
        a[15 * w + 15] = 0; // a pinhole
        a[36 * w + 36] = 255; // a speck
        tidy(&mut a, w, h, &[(10, 10)]);
        assert_eq!(a[15 * w + 15], 255);
        assert_eq!(a[36 * w + 36], 0);
        assert_eq!(a[20 * w + 20], 255);
        // A click just outside the mask keeps its biggest part.
        let mut b = vec![0u8; w * h];
        b[3] = 255;
        tidy(&mut b, w, h, &[(30, 30)]);
        assert_eq!(b[3], 255);
        // A big part away from the click survives; a small one does not.
        let mut c = vec![0u8; w * h];
        for y in 0..10 {
            for x in 0..10 {
                c[y * w + x] = 255;
                c[(y + 20) * w + x + 20] = 255;
            }
        }
        c[0 * w + 39] = 255;
        tidy(&mut c, w, h, &[(5, 5)]);
        assert_eq!(c[25 * w + 25], 255);
        assert_eq!(c[39], 0);
    }

    #[test]
    fn upscale_maps_low_res_onto_the_picture() {
        // A low-res mask positive on its left half (in padded 1024 space).
        let mut low = vec![-5f32; LOW * LOW];
        for y in 0..LOW {
            for x in 0..LOW / 2 {
                low[y * LOW + x] = 5.0;
            }
        }
        // 1024x512 picture: scale 1, left half of the picture is positive.
        let up = upscale(&low, 1.0, 1024, 512);
        assert!(up[100 * 1024 + 200] > 0.0);
        assert!(up[100 * 1024 + 800] < 0.0);
        let a = alpha(&up);
        assert_eq!(a[10 * 1024 + 10], 255);
        assert_eq!(a[10 * 1024 + 1000], 0);
    }

    #[test]
    fn cut_crops_to_the_object_with_straight_alpha() {
        let (w, h) = (20, 10);
        let img = solid(w, h, [10, 20, 30]);
        let mut a = vec![0u8; w * h];
        for y in 2..5 {
            for x in 3..9 {
                a[y * w + x] = 255;
            }
        }
        assert_eq!(bbox(&a, w, h), Some((3, 2, 9, 5)));
        let c = cut(&img, w, h, &a, 1).unwrap();
        assert_eq!((c.w, c.h), (8, 5));
        // Corner is outside the object: transparent; inside: RGB from BGR.
        assert_eq!(c.px[3], 0);
        let i = (2 * c.w + 3) * 4;
        assert_eq!(&c.px[i..i + 4], &[30, 20, 10, 255]);
        assert!(cut(&img, w, h, &vec![0u8; w * h], 1).is_none());
    }

    #[test]
    fn sticker_adds_a_white_outline_and_shadow() {
        let mut s = Rgba { w: 40, h: 40, px: vec![0u8; 40 * 40 * 4] };
        for y in 10..30 {
            for x in 10..30 {
                s.px[(y * 40 + x) * 4..][..4].copy_from_slice(&[200, 0, 0, 255]);
            }
        }
        let st = sticker(&s);
        assert!(st.w > s.w && st.h > s.h);
        let pad = (st.w - s.w) / 2;
        let at = |x: usize, y: usize| &st.px[(y * st.w + x) * 4..][..4];
        // The object keeps its colour.
        assert_eq!(at(pad + 20, pad + 20), &[200, 0, 0, 255]);
        // Just outside it: opaque white outline.
        let o = at(pad + 20, pad + 8);
        assert!(o[0] > 240 && o[1] > 240 && o[3] > 240, "{o:?}");
        // Far corner: nothing.
        assert_eq!(at(0, 0)[3], 0);
        // Below the outline there is some shadow.
        let below = (0..st.h).rev().find(|&y| at(st.w / 2, y)[3] > 0).unwrap();
        assert!(below > pad + 30 + 4);
    }

    #[test]
    fn edge_is_strongest_on_the_boundary() {
        let (w, h) = (30, 30);
        let mut a = vec![0u8; w * h];
        for y in 10..20 {
            for x in 10..20 {
                a[y * w + x] = 255;
            }
        }
        let e = edge(&a, w, h, 3.0);
        assert!(e[15 * w + 10] > 150);
        assert_eq!(e[15 * w + 15], 0);
        assert_eq!(e[2 * w + 2], 0);
    }

    /// End to end with the real models: `SNAP_MODELS=<dir> ORT_DYLIB=<lib> cargo test -- --ignored cutout_real`.
    #[test]
    #[ignore]
    fn cutout_real_models_find_the_circle() {
        let dir = std::path::PathBuf::from(std::env::var("SNAP_MODELS").expect("SNAP_MODELS"));
        let lib = std::path::PathBuf::from(std::env::var("ORT_DYLIB").expect("ORT_DYLIB"));
        let (w, h) = (1280usize, 720usize);
        let mut img = vec![0u8; w * h * 4];
        let mut truth = vec![false; w * h];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                img[i..i + 4].copy_from_slice(&[180, (60 + y * 100 / h) as u8, (40 + x * 160 / w) as u8, 255]);
                let inside = ((x as f32 - 550.0) / 150.0).powi(2) + ((y as f32 - 350.0) / 150.0).powi(2) <= 1.0;
                if inside {
                    img[i..i + 4].copy_from_slice(&[40, 40, 230, 255]);
                    truth[y * w + x] = true;
                }
                if (850..1100).contains(&x) && (250..450).contains(&y) {
                    img[i..i + 4].copy_from_slice(&[200, 30, 30, 255]);
                }
            }
        }
        let eng = engine(&dir, &lib).unwrap();
        let mut eng = eng.lock().unwrap();
        let p = prepare(&img, w, h);
        let t = std::time::Instant::now();
        let emb = eng.embed(&p).unwrap();
        eprintln!("embed {} ms", t.elapsed().as_millis());
        let t = std::time::Instant::now();
        let m = eng.predict(&emb, p.scale, &[(550.0, 350.0, true)]).unwrap();
        eprintln!("predict {} ms, iou {:?}", t.elapsed().as_millis(), m.iou);
        let k = choose(&m);
        let mut a = alpha(&upscale(m.mask(k), p.scale, w, h));
        tidy(&mut a, w, h, &[(550, 350)]);
        let (mut inter, mut uni) = (0, 0);
        for i in 0..w * h {
            let mine = a[i] >= 128;
            inter += (mine && truth[i]) as usize;
            uni += (mine || truth[i]) as usize;
        }
        let iou = inter as f32 / uni as f32;
        eprintln!("mask {k}: iou with the circle {iou:.3}");
        assert!(iou > 0.9);
        let c = cut(&img, w, h, &a, 2).unwrap();
        assert!((c.w as i32 - 304).abs() < 12 && (c.h as i32 - 304).abs() < 12, "{}x{}", c.w, c.h);
        let st = sticker(&c);
        let _ = std::fs::write(std::env::temp_dir().join("snap-cutout-test.png"), st.encode_png());
    }

    /// Cuts a real photo for a look: `SNAP_PHOTO=<png>:<x>:<y>` (plus SNAP_MODELS, ORT_DYLIB).
    #[test]
    #[ignore]
    fn cutout_real_photo() {
        let spec = std::env::var("SNAP_PHOTO").expect("SNAP_PHOTO");
        let parts: Vec<&str> = spec.rsplitn(3, ':').collect();
        let (y, x, path) = (parts[0].parse::<f32>().unwrap(), parts[1].parse::<f32>().unwrap(), parts[2]);
        let f = crate::image::decode_png(&std::fs::read(path).unwrap()).unwrap();
        let (w, h) = (f.width(), f.height());
        let dir = std::path::PathBuf::from(std::env::var("SNAP_MODELS").unwrap());
        let lib = std::path::PathBuf::from(std::env::var("ORT_DYLIB").unwrap());
        let mut eng = engine(&dir, &lib).unwrap().lock().unwrap();
        let p = prepare(&f.pixels, w, h);
        let emb = eng.embed(&p).unwrap();
        let m = eng.predict(&emb, p.scale, &[(x, y, true)]).unwrap();
        let k = choose(&m);
        eprintln!("iou {:?} stability {:?} -> mask {k}", m.iou, (1..4).map(|i| stability(m.mask(i))).collect::<Vec<_>>());
        for i in 1..4 {
            let mut a = alpha(&upscale(m.mask(i), p.scale, w, h));
            tidy(&mut a, w, h, &[(x as usize, y as usize)]);
            if let Some(c) = cut(&f.pixels, w, h, &a, 4) {
                let name = std::path::Path::new(path).file_stem().unwrap().to_string_lossy().into_owned();
                let _ = std::fs::write(std::env::temp_dir().join(format!("{name}-mask{i}{}.png", if i == k { "-chosen" } else { "" })), sticker(&c).encode_png());
            }
        }
    }
}

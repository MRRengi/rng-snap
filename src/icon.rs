//! The app icon, drawn in code: a blue squircle with a soft top-to-bottom
//! gradient and a white viewfinder (four rounded corner brackets and a dot).
//! Returns BGRA, premultiplied.

fn coverage(n: usize, inside: impl Fn(f32, f32) -> bool) -> Vec<f32> {
    const SS: usize = 4;
    let mut out = vec![0.0; n * n];
    for y in 0..n {
        for x in 0..n {
            let mut hits = 0;
            for sy in 0..SS {
                for sx in 0..SS {
                    let fx = (x as f32 + (sx as f32 + 0.5) / SS as f32) / n as f32;
                    let fy = (y as f32 + (sy as f32 + 0.5) / SS as f32) / n as f32;
                    if inside(fx, fy) {
                        hits += 1;
                    }
                }
            }
            out[y * n + x] = hits as f32 / (SS * SS) as f32;
        }
    }
    out
}

/// Apple-style squircle (superellipse, n = 5).
fn squircle(x: f32, y: f32) -> bool {
    let (dx, dy) = (((x - 0.5) / 0.47).abs(), ((y - 0.5) / 0.47).abs());
    dx.powf(5.0) + dy.powf(5.0) <= 1.0
}

/// Distance from (x, y) to the segment a-b.
fn seg_dist(x: f32, y: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let t = (((x - a.0) * vx + (y - a.1) * vy) / (vx * vx + vy * vy)).clamp(0.0, 1.0);
    let (px, py) = (a.0 + vx * t, a.1 + vy * t);
    ((x - px).powi(2) + (y - py).powi(2)).sqrt()
}

fn viewfinder(x: f32, y: f32) -> bool {
    let (a, len, th) = (0.27, 0.17, 0.085);
    // Fold into the top-left quadrant: all four brackets are mirror images.
    let fx = if x > 0.5 { 1.0 - x } else { x };
    let fy = if y > 0.5 { 1.0 - y } else { y };
    let r = th / 2.0;
    let c = a + r;
    let bracket = seg_dist(fx, fy, (c, c), (c + len - th, c)) <= r || seg_dist(fx, fy, (c, c), (c, c + len - th)) <= r;
    let dot = (x - 0.5).powi(2) + (y - 0.5).powi(2) <= 0.068f32.powi(2);
    bracket || dot
}

pub fn pixels(n: usize) -> Vec<u8> {
    let bg = coverage(n, squircle);
    let fg = coverage(n, viewfinder);
    // Top and bottom of the gradient (systemBlue family).
    let top = (0x52 as f32, 0xB4 as f32, 0xFF as f32);
    let bottom = (0x0A as f32, 0x68 as f32, 0xF5 as f32);
    let mut out = Vec::with_capacity(n * n * 4);
    for i in 0..n * n {
        let y = (i / n) as f32 / (n.max(2) - 1) as f32;
        let mix = |a: f32, b: f32| a + (b - a) * y;
        // A faint sheen on the upper half.
        let sheen = (1.0 - y * 2.0).max(0.0) * 0.10;
        let (ar, ag, ab) = (mix(top.0, bottom.0), mix(top.1, bottom.1), mix(top.2, bottom.2));
        let (ar, ag, ab) = (ar + (255.0 - ar) * sheen, ag + (255.0 - ag) * sheen, ab + (255.0 - ab) * sheen);
        let a = bg[i];
        let w = fg[i].min(a);
        // Premultiplied: white over the gradient, both scaled by the squircle's coverage.
        let r = ar * (a - w) + 255.0 * w;
        let g = ag * (a - w) + 255.0 * w;
        let b = ab * (a - w) + 255.0 * w;
        out.extend_from_slice(&[b.round() as u8, g.round() as u8, r.round() as u8, (a * 255.0).round() as u8]);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_shape() {
        let n = 32;
        let p = super::pixels(n);
        assert_eq!(p.len(), n * n * 4);
        let at = |x: usize, y: usize| &p[(y * n + x) * 4..(y * n + x) * 4 + 4];
        assert_eq!(at(0, 0)[3], 0, "corner is transparent");
        assert_eq!(at(16, 16), &[255, 255, 255, 255], "center dot is white");
        assert_eq!(at(16, 3)[3], 255, "edge is opaque");
        assert!(at(16, 3)[0] > 0xF0, "edge is blue");
        // Lighter at the top than at the bottom.
        assert!(at(16, 3)[1] > at(16, 28)[1]);
    }
}

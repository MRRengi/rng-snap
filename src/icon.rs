//! The app icon, drawn in code: an accent rounded square with a white
//! viewfinder (four corner brackets and a dot). Returns BGRA, premultiplied.

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

fn rounded_square(x: f32, y: f32, inset: f32, r: f32) -> bool {
    let (l, t, rr, b) = (inset, inset, 1.0 - inset, 1.0 - inset);
    if x < l || x > rr || y < t || y > b {
        return false;
    }
    let cx = x.clamp(l + r, rr - r);
    let cy = y.clamp(t + r, b - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

fn viewfinder(x: f32, y: f32) -> bool {
    let (a, len, th) = (0.24, 0.19, 0.085);
    // Fold into the top-left quadrant: all four brackets are mirror images.
    let fx = if x > 0.5 { 1.0 - x } else { x };
    let fy = if y > 0.5 { 1.0 - y } else { y };
    let horiz = fy >= a && fy <= a + th && fx >= a && fx <= a + len;
    let vert = fx >= a && fx <= a + th && fy >= a && fy <= a + len;
    let dot = (x - 0.5).powi(2) + (y - 0.5).powi(2) <= 0.075f32.powi(2);
    horiz || vert || dot
}

pub fn pixels(n: usize) -> Vec<u8> {
    let bg = coverage(n, |x, y| rounded_square(x, y, 0.03, 0.22));
    let fg = coverage(n, viewfinder);
    let (ar, ag, ab) = (0x4Cu8 as f32, 0x8Du8 as f32, 0xFFu8 as f32);
    let mut out = Vec::with_capacity(n * n * 4);
    for i in 0..n * n {
        let a = bg[i];
        let w = fg[i].min(a);
        // Premultiplied: white over accent, both scaled by the square's coverage.
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
        assert_eq!(at(16, 4)[3], 255, "edge is opaque accent");
        assert_eq!(at(16, 4)[0], 0xFF);
    }
}

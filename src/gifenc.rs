//! GIF encoding of decoded video frames: frame-rate cap, downscale, a
//! per-frame palette (NeuQuant via the gif crate) and accumulated delays.

use crate::image::Frame;
use std::io::Write;

pub const MAX_FPS: u32 = 30;
pub const MAX_WIDTH: u32 = 960;

/// Output size for a source size (keeps aspect, never upscales, even-free).
pub fn out_size(w: u32, h: u32) -> (u32, u32) {
    if w <= MAX_WIDTH {
        (w, h)
    } else {
        (MAX_WIDTH, ((h as u64 * MAX_WIDTH as u64) / w as u64).max(1) as u32)
    }
}

/// Area-average downscale of a BGRA frame to `ow x oh`, returned as RGBA.
pub fn scale_to_rgba(f: &Frame, ow: u32, oh: u32) -> Vec<u8> {
    let (w, h) = (f.width() as u32, f.height() as u32);
    let mut out = Vec::with_capacity((ow * oh * 4) as usize);
    if (ow, oh) == (w, h) {
        for px in f.pixels.chunks_exact(4) {
            out.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        return out;
    }
    for oy in 0..oh {
        let y0 = (oy as u64 * h as u64 / oh as u64) as u32;
        let y1 = (((oy + 1) as u64 * h as u64 / oh as u64) as u32).max(y0 + 1).min(h);
        for ox in 0..ow {
            let x0 = (ox as u64 * w as u64 / ow as u64) as u32;
            let x1 = (((ox + 1) as u64 * w as u64 / ow as u64) as u32).max(x0 + 1).min(w);
            let (mut b, mut g, mut r, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in y0..y1 {
                let row = (y * w) as usize * 4;
                for x in x0..x1 {
                    let i = row + x as usize * 4;
                    b += f.pixels[i] as u32;
                    g += f.pixels[i + 1] as u32;
                    r += f.pixels[i + 2] as u32;
                    n += 1;
                }
            }
            out.extend_from_slice(&[(r / n) as u8, (g / n) as u8, (b / n) as u8, 255]);
        }
    }
    out
}

/// Streams frames into a GIF. Feed every decoded frame with its timestamp;
/// frames closer than 1/30 s to the previous kept one are skipped.
pub struct GifWriter<W: Write> {
    enc: gif::Encoder<W>,
    w: u32,
    h: u32,
    next_t: i64,
    pending: Option<(i64, Vec<u8>)>,
    /// Centiseconds already written, to keep delays from drifting.
    written_cs: i64,
    first_t: Option<i64>,
    pub frames: u32,
}

impl<W: Write> GifWriter<W> {
    pub fn new(out: W, src_w: u32, src_h: u32) -> Result<GifWriter<W>, String> {
        let (w, h) = out_size(src_w, src_h);
        let mut enc = gif::Encoder::new(out, w as u16, h as u16, &[]).map_err(|e| e.to_string())?;
        enc.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
        Ok(GifWriter { enc, w, h, next_t: i64::MIN, pending: None, written_cs: 0, first_t: None, frames: 0 })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    pub fn push(&mut self, t: i64, f: &Frame) -> Result<(), String> {
        if t < self.next_t {
            return Ok(());
        }
        self.next_t = t + 10_000_000 / MAX_FPS as i64 - 5_000;
        let rgba = scale_to_rgba(f, self.w, self.h);
        if let Some((pt, prev)) = self.pending.take() {
            self.emit(pt, prev, t)?;
        }
        self.pending = Some((t, rgba));
        Ok(())
    }

    fn emit(&mut self, t: i64, mut rgba: Vec<u8>, until: i64) -> Result<(), String> {
        let first = *self.first_t.get_or_insert(t);
        let end_cs = (until - first) / 100_000;
        let delay = (end_cs - self.written_cs).clamp(2, 65_535);
        self.written_cs += delay;
        let mut frame = gif::Frame::from_rgba_speed(self.w as u16, self.h as u16, &mut rgba, 10);
        frame.delay = delay as u16;
        self.enc.write_frame(&frame).map_err(|e| e.to_string())?;
        self.frames += 1;
        Ok(())
    }

    /// Writes the last frame (shown for `tail` 100 ns units).
    pub fn finish(mut self, tail: i64) -> Result<u32, String> {
        if let Some((pt, prev)) = self.pending.take() {
            self.emit(pt, prev, pt + tail.max(100_000))?;
        }
        Ok(self.frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rect;

    fn solid(w: i32, h: i32, bgr: [u8; 3]) -> Frame {
        let mut pixels = Vec::new();
        for _ in 0..w * h {
            pixels.extend_from_slice(&[bgr[0], bgr[1], bgr[2], 255]);
        }
        Frame { rect: Rect::from_xywh(0, 0, w, h), pixels }
    }

    #[test]
    fn sizes() {
        assert_eq!(out_size(800, 600), (800, 600));
        assert_eq!(out_size(1920, 1080), (960, 540));
    }

    #[test]
    fn scale_averages() {
        let mut f = solid(4, 2, [0, 0, 0]);
        // Left half white.
        for y in 0..2 {
            for x in 0..2 {
                let i = (y * 4 + x) * 4;
                f.pixels[i..i + 3].copy_from_slice(&[255, 255, 255]);
            }
        }
        let out = scale_to_rgba(&f, 2, 1);
        assert_eq!(out, vec![255, 255, 255, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn caps_rate_and_round_trips() {
        let mut buf = Vec::new();
        {
            let mut g = GifWriter::new(&mut buf, 40, 30).unwrap();
            // 120 fps input for 1 s, alternating colors.
            for i in 0..120i64 {
                let c = if (i / 4) % 2 == 0 { [0, 0, 255] } else { [255, 0, 0] };
                g.push(i * 10_000_000 / 120, &solid(40, 30, c)).unwrap();
            }
            let n = g.finish(10_000_000 / 120).unwrap();
            assert!((29..=31).contains(&n), "{n} frames");
        }
        let mut dec = gif::DecodeOptions::new();
        dec.set_color_output(gif::ColorOutput::RGBA);
        let mut d = dec.read_info(std::io::Cursor::new(buf)).unwrap();
        let (mut frames, mut total) = (0, 0u32);
        while let Some(f) = d.read_next_frame().unwrap() {
            frames += 1;
            total += f.delay as u32;
        }
        assert!((29..=31).contains(&frames));
        assert!((95..=105).contains(&total), "total {total} cs");
    }
}

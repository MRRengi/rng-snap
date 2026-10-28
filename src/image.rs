//! Frozen frame buffer and the pixel work done on it. Pure Rust.

use crate::geom::Rect;

/// Top-down 32-bit BGRA pixels (alpha ignored, treated as opaque).
#[derive(Clone)]
pub struct Frame {
    /// Virtual-desktop rect this frame covers.
    pub rect: Rect,
    pub pixels: Vec<u8>,
}

impl Frame {
    pub fn width(&self) -> usize {
        self.rect.width().max(0) as usize
    }

    pub fn height(&self) -> usize {
        self.rect.height().max(0) as usize
    }

    pub fn stride(&self) -> usize {
        self.width() * 4
    }

    /// BGR at a desktop coordinate, if inside.
    pub fn pixel(&self, x: i32, y: i32) -> Option<[u8; 3]> {
        let (lx, ly) = (x - self.rect.left, y - self.rect.top);
        if lx < 0 || ly < 0 || lx as usize >= self.width() || ly as usize >= self.height() {
            return None;
        }
        let i = ly as usize * self.stride() + lx as usize * 4;
        Some([self.pixels[i], self.pixels[i + 1], self.pixels[i + 2]])
    }

    /// Copies a desktop-coordinate area out as a new frame (clipped to this frame).
    pub fn crop(&self, area: Rect) -> Frame {
        let area = area.intersect(&self.rect);
        let (w, h) = (area.width().max(0) as usize, area.height().max(0) as usize);
        let mut pixels = Vec::with_capacity(w * h * 4);
        let x0 = (area.left - self.rect.left) as usize * 4;
        for row in 0..h {
            let y = (area.top - self.rect.top) as usize + row;
            let start = y * self.stride() + x0;
            pixels.extend_from_slice(&self.pixels[start..start + w * 4]);
        }
        Frame { rect: area, pixels }
    }

    /// Same pixels with alpha forced to 255 (for clipboard formats that honour alpha).
    pub fn opaque_bgra(&self) -> Vec<u8> {
        let mut out = self.pixels.clone();
        for px in out.chunks_exact_mut(4) {
            px[3] = 255;
        }
        out
    }

    /// Encodes as an 8-bit RGB PNG. Screenshots are opaque, so no alpha channel.
    pub fn encode_png(&self) -> Vec<u8> {
        let (w, h) = (self.width(), self.height());
        let mut rgb = Vec::with_capacity(w * h * 3);
        for px in self.pixels.chunks_exact(4) {
            rgb.extend_from_slice(&[px[2], px[1], px[0]]);
        }
        let mut out = Vec::with_capacity(rgb.len() / 3);
        {
            let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Fast);
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(&rgb).expect("png data");
        }
        out
    }
}

/// Decodes a PNG (any bit depth / colour type) into an opaque BGRA frame
/// at (0, 0). Transparent pixels are composited over white.
pub fn decode_png(bytes: &[u8]) -> Option<Frame> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return None,
    };
    let mut pixels = Vec::with_capacity(w * h * 4);
    for px in buf[..w * h * ch].chunks_exact(ch) {
        let (r, g, b, a) = match ch {
            1 => (px[0], px[0], px[0], 255),
            2 => (px[0], px[0], px[0], px[1]),
            3 => (px[0], px[1], px[2], 255),
            _ => (px[0], px[1], px[2], px[3]),
        };
        let mix = |c: u8| ((c as u32 * a as u32 + 255 * (255 - a as u32)) / 255) as u8;
        pixels.extend_from_slice(&[mix(b), mix(g), mix(r), 255]);
    }
    Some(Frame { rect: Rect::from_xywh(0, 0, w as i32, h as i32), pixels })
}

/// First frame of a GIF as an opaque BGRA frame (transparent = white).
pub fn decode_gif_first(bytes: &[u8]) -> Option<Frame> {
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut d = opts.read_info(std::io::Cursor::new(bytes)).ok()?;
    let (sw, sh) = (d.width() as usize, d.height() as usize);
    let f = d.read_next_frame().ok()??;
    let mut pixels = vec![255u8; sw * sh * 4];
    let (fl, ft, fw, fh) = (f.left as usize, f.top as usize, f.width as usize, f.height as usize);
    for y in 0..fh.min(sh.saturating_sub(ft)) {
        for x in 0..fw.min(sw.saturating_sub(fl)) {
            let si = (y * fw + x) * 4;
            let a = f.buffer[si + 3] as u32;
            let mix = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
            let di = ((ft + y) * sw + fl + x) * 4;
            pixels[di..di + 4].copy_from_slice(&[mix(f.buffer[si + 2]), mix(f.buffer[si + 1]), mix(f.buffer[si]), 255]);
        }
    }
    Some(Frame { rect: Rect::from_xywh(0, 0, sw as i32, sh as i32), pixels })
}

pub fn hex_color(bgr: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", bgr[2], bgr[1], bgr[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        // 4x3 frame at desktop (10, 20); pixel value encodes its position.
        let rect = Rect::new(10, 20, 14, 23);
        let mut pixels = Vec::new();
        for y in 0..3u8 {
            for x in 0..4u8 {
                pixels.extend_from_slice(&[x, y, 7, 0]);
            }
        }
        Frame { rect, pixels }
    }

    #[test]
    fn pixel_lookup() {
        let f = frame();
        assert_eq!(f.pixel(12, 21), Some([2, 1, 7]));
        assert_eq!(f.pixel(9, 21), None);
        assert_eq!(f.pixel(14, 21), None);
    }

    #[test]
    fn crop_clips_and_copies_rows() {
        let f = frame();
        let c = f.crop(Rect::new(11, 21, 100, 100));
        assert_eq!(c.rect, Rect::new(11, 21, 14, 23));
        assert_eq!((c.width(), c.height()), (3, 2));
        assert_eq!(c.pixel(11, 21), Some([1, 1, 7]));
        assert_eq!(c.pixel(13, 22), Some([3, 2, 7]));
    }

    #[test]
    fn png_round_trips() {
        let f = frame();
        let bytes = f.encode_png();
        let dec = png::Decoder::new(std::io::Cursor::new(bytes));
        let mut r = dec.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let info = r.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (4, 3));
        // Pixel (2,1) is BGR [2,1,7] -> RGB [7,1,2]
        let i = (1 * 4 + 2) * 3;
        assert_eq!(&buf[i..i + 3], &[7, 1, 2]);
    }

    #[test]
    fn png_decodes_back_to_the_same_pixels() {
        let f = frame();
        let d = decode_png(&f.encode_png()).unwrap();
        assert_eq!((d.width(), d.height()), (4, 3));
        assert_eq!(d.pixel(2, 1), Some([2, 1, 7]));
        assert!(decode_png(b"not a png").is_none());
    }

    #[test]
    fn gif_first_frame() {
        let mut out = Vec::new();
        {
            let mut enc = gif::Encoder::new(&mut out, 2, 1, &[255, 0, 0, 0, 0, 255]).unwrap();
            let fr = gif::Frame { width: 2, height: 1, buffer: std::borrow::Cow::Borrowed(&[0, 1]), ..Default::default() };
            enc.write_frame(&fr).unwrap();
        }
        let d = decode_gif_first(&out).unwrap();
        assert_eq!((d.width(), d.height()), (2, 1));
        assert_eq!(&d.pixels[0..3], &[0, 0, 255]);
        assert_eq!(&d.pixels[4..7], &[255, 0, 0]);
    }

    #[test]
    fn hex() {
        assert_eq!(hex_color([0x10, 0x20, 0xF0]), "#F02010");
    }
}

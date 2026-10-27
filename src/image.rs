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
    fn hex() {
        assert_eq!(hex_color([0x10, 0x20, 0xF0]), "#F02010");
    }
}

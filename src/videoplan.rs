//! Encoding plan for a recording: output size, codec, bitrate. Pure logic.

use crate::geom::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Hevc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Res {
    Original,
    P1080,
    P720,
}

impl Res {
    pub fn parse(s: &str) -> Res {
        match s {
            "1080p" => Res::P1080,
            "720p" => Res::P720,
            _ => Res::Original,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Res::Original => "original",
            Res::P1080 => "1080p",
            Res::P720 => "720p",
        }
    }

    fn bounds(&self) -> Option<(u32, u32)> {
        match self {
            Res::Original => None,
            Res::P1080 => Some((1920, 1080)),
            Res::P720 => Some((1280, 720)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub src_w: u32,
    pub src_h: u32,
    pub out_w: u32,
    pub out_h: u32,
    pub fps: u32,
    pub codec: Codec,
    pub bitrate: u32,
}

/// H.264 level 5.2 limits (macroblocks per frame, per second).
const H264_MAX_FRAME_MBS: u64 = 36_864;
const H264_MAX_MBS_PER_SEC: u64 = 2_073_600;
const MIN_BITRATE: u64 = 2_000_000;
const MAX_BITRATE: u64 = 80_000_000;

/// Width and height rounded down to even numbers (NV12 needs that).
pub fn even_rect(r: Rect) -> Rect {
    let w = (r.width() & !1).max(2);
    let h = (r.height() & !1).max(2);
    Rect::from_xywh(r.left, r.top, w, h)
}

fn even(v: f64) -> u32 {
    ((v.floor() as u32) & !1).max(2)
}

fn mbs(w: u32, h: u32) -> u64 {
    w.div_ceil(16) as u64 * h.div_ceil(16) as u64
}

/// Fits `w x h` inside `bw x bh` keeping aspect, never scaling up.
fn fit(w: u32, h: u32, bw: u32, bh: u32) -> (u32, u32) {
    let s = (bw as f64 / w as f64).min(bh as f64 / h as f64);
    if s >= 1.0 { (w, h) } else { (even(w as f64 * s), even(h as f64 * s)) }
}

pub fn plan(src_w: u32, src_h: u32, fps: u32, res: Res, hevc_available: bool) -> Plan {
    let fps = fps.clamp(1, 360);
    let (mut out_w, mut out_h) = match res.bounds() {
        Some((bw, bh)) => {
            // Portrait regions get the box turned too.
            let (bw, bh) = if src_h > src_w { (bh, bw) } else { (bw, bh) };
            fit(src_w, src_h, bw, bh)
        }
        None => (src_w & !1, src_h & !1),
    };
    let mut fps_out = fps;
    let fits_h264 = |w: u32, h: u32, f: u32| mbs(w, h) <= H264_MAX_FRAME_MBS && mbs(w, h) * f as u64 <= H264_MAX_MBS_PER_SEC;
    let codec = if fits_h264(out_w, out_h, fps) {
        Codec::H264
    } else if hevc_available {
        Codec::Hevc
    } else {
        // No HEVC encoder: keep H.264 and give up size first, then rate.
        if mbs(out_w, out_h) > H264_MAX_FRAME_MBS {
            let (w, h) = fit(out_w, out_h, 3840, 2160);
            out_w = w;
            out_h = h;
        }
        let max_fps = (H264_MAX_MBS_PER_SEC / mbs(out_w, out_h)) as u32;
        fps_out = fps.min(max_fps.max(1));
        Codec::H264
    };
    let px = out_w as u64 * out_h as u64 * fps_out as u64;
    let mut bitrate = (px / 10).clamp(MIN_BITRATE, MAX_BITRATE);
    if codec == Codec::Hevc {
        bitrate = bitrate * 7 / 10;
    }
    Plan { src_w, src_h, out_w, out_h, fps: fps_out, codec, bitrate: bitrate as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_rounding() {
        assert_eq!(even_rect(Rect::new(10, 10, 111, 61)), Rect::new(10, 10, 110, 60));
        assert_eq!(even_rect(Rect::new(0, 0, 1, 1)).width(), 2);
    }

    #[test]
    fn h264_where_it_fits() {
        let p = plan(1920, 1080, 120, Res::Original, true);
        assert_eq!((p.out_w, p.out_h, p.fps, p.codec), (1920, 1080, 120, Codec::H264));
        let p = plan(2560, 1440, 144, Res::Original, true);
        assert_eq!(p.codec, Codec::H264);
        let p = plan(3840, 2160, 60, Res::Original, true);
        assert_eq!(p.codec, Codec::H264);
    }

    #[test]
    fn hevc_above_the_limit() {
        let p = plan(3840, 2160, 120, Res::Original, true);
        assert_eq!((p.codec, p.fps), (Codec::Hevc, 120));
        let p = plan(2560, 1440, 240, Res::Original, true);
        assert_eq!(p.codec, Codec::Hevc);
    }

    #[test]
    fn no_hevc_drops_rate() {
        let p = plan(3840, 2160, 120, Res::Original, false);
        assert_eq!(p.codec, Codec::H264);
        assert_eq!(p.fps, 64);
        let p = plan(5120, 2880, 60, Res::Original, false);
        assert_eq!((p.out_w, p.out_h), (3840, 2160));
    }

    #[test]
    fn downscale_only() {
        let p = plan(1024, 768, 60, Res::P720, true);
        assert_eq!((p.out_w, p.out_h), (960, 720));
        let p = plan(800, 600, 60, Res::P1080, true);
        assert_eq!((p.out_w, p.out_h), (800, 600));
        let p = plan(3840, 2160, 60, Res::P1080, true);
        assert_eq!((p.out_w, p.out_h), (1920, 1080));
        // Portrait.
        let p = plan(1080, 1920, 60, Res::P720, true);
        assert_eq!((p.out_w, p.out_h), (720, 1280));
    }

    #[test]
    fn bitrate_scales_and_caps() {
        let a = plan(1920, 1080, 60, Res::Original, true).bitrate;
        let b = plan(1920, 1080, 120, Res::Original, true).bitrate;
        assert!(b > a);
        assert_eq!(plan(3840, 2160, 240, Res::Original, true).bitrate, 80_000_000 * 7 / 10);
        assert_eq!(plan(100, 100, 30, Res::Original, true).bitrate, 2_000_000);
    }
}

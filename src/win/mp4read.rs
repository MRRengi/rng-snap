//! Decodes video files frame by frame into BGRA with Media Foundation.
//! Used by GIF export and by the end-to-end test.

use crate::geom::Rect;
use crate::image::Frame;
use std::path::Path;
use windows::Win32::Media::MediaFoundation::*;
use windows::core::HSTRING;

pub struct Reader {
    r: IMFSourceReader,
    pub w: u32,
    pub h: u32,
    bottom_up: bool,
}

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;

pub fn startup() {
    unsafe {
        let _ = MFStartup(MF_VERSION, 0);
    }
}

impl Reader {
    pub fn open(path: &Path) -> Result<Reader, String> {
        startup();
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1).map_err(|e| e.to_string())?;
            let attrs = attrs.ok_or("attributes")?;
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(|e| e.to_string())?;
            let r = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attrs).map_err(|e| format!("open: {e}"))?;
            let t = MFCreateMediaType().map_err(|e| e.to_string())?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| e.to_string())?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(|e| e.to_string())?;
            r.SetCurrentMediaType(VIDEO, None, &t).map_err(|e| format!("rgb32: {e}"))?;
            let cur = r.GetCurrentMediaType(VIDEO).map_err(|e| e.to_string())?;
            let size = cur.GetUINT64(&MF_MT_FRAME_SIZE).map_err(|e| e.to_string())?;
            let (w, h) = ((size >> 32) as u32, (size & 0xFFFF_FFFF) as u32);
            let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(w as i32 * 4);
            Ok(Reader { r, w, h, bottom_up: stride < 0 })
        }
    }

    /// True if the file has an audio track.
    pub fn has_audio(&self) -> bool {
        unsafe { self.r.GetCurrentMediaType(AUDIO).is_ok() }
    }

    /// Next frame and its timestamp (100 ns), or None at the end.
    pub fn next(&mut self) -> Option<(i64, Frame)> {
        loop {
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            unsafe {
                self.r.ReadSample(VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).ok()?;
            }
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                return None;
            }
            let Some(sample) = sample else { continue };
            unsafe {
                let buf = sample.ConvertToContiguousBuffer().ok()?;
                let mut ptr = std::ptr::null_mut();
                let mut len = 0u32;
                buf.Lock(&mut ptr, None, Some(&mut len)).ok()?;
                let row = self.w as usize * 4;
                let need = row * self.h as usize;
                let mut pixels = Vec::with_capacity(need);
                if (len as usize) >= need {
                    let data = std::slice::from_raw_parts(ptr, need);
                    if self.bottom_up {
                        for r in data.chunks_exact(row).rev() {
                            pixels.extend_from_slice(r);
                        }
                    } else {
                        pixels.extend_from_slice(data);
                    }
                }
                let _ = buf.Unlock();
                if pixels.len() == need {
                    return Some((ts, Frame { rect: Rect::from_xywh(0, 0, self.w as i32, self.h as i32), pixels }));
                }
            }
        }
    }
}

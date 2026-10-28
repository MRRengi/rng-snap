//! Decodes video files frame by frame into BGRA with Media Foundation, and
//! their audio into 16-bit PCM. Used by GIF export, the video editor (preview
//! and export) and the end-to-end test.

use crate::geom::Rect;
use crate::image::Frame;
use std::path::Path;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Variant::VT_I8;
use windows::core::HSTRING;

pub struct Reader {
    r: IMFSourceReader,
    pub w: u32,
    pub h: u32,
    bottom_up: bool,
    /// Frame rate as a fraction (30/1 if the file doesn't say).
    pub fps: (u32, u32),
    /// Length in 100 ns units (0 if unknown).
    pub duration: i64,
}

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
const SOURCE: u32 = MF_SOURCE_READER_MEDIASOURCE.0 as u32;

pub fn startup() {
    unsafe {
        let _ = MFStartup(MF_VERSION, 0);
    }
}

fn duration_of(r: &IMFSourceReader) -> i64 {
    unsafe {
        match r.GetPresentationAttribute(SOURCE, &MF_PD_DURATION) {
            Ok(v) => v.Anonymous.Anonymous.Anonymous.uhVal as i64,
            Err(_) => 0,
        }
    }
}

fn seek_to(r: &IMFSourceReader, t: i64) -> bool {
    unsafe {
        let mut pv = PROPVARIANT::default();
        (*pv.Anonymous.Anonymous).vt = VT_I8;
        (*pv.Anonymous.Anonymous).Anonymous.hVal = t.max(0);
        r.SetCurrentPosition(&windows::core::GUID::zeroed(), &pv).is_ok()
    }
}

fn open_reader(path: &Path, video_processing: bool) -> Result<IMFSourceReader, String> {
    startup();
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1).map_err(|e| e.to_string())?;
        let attrs = attrs.ok_or("attributes")?;
        if video_processing {
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1).map_err(|e| e.to_string())?;
        }
        MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attrs).map_err(|e| format!("open: {e}"))
    }
}

impl Reader {
    pub fn open(path: &Path) -> Result<Reader, String> {
        unsafe {
            let r = open_reader(path, true)?;
            let _ = r.SetStreamSelection(AUDIO, false);
            let t = MFCreateMediaType().map_err(|e| e.to_string())?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| e.to_string())?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(|e| e.to_string())?;
            r.SetCurrentMediaType(VIDEO, None, &t).map_err(|e| format!("rgb32: {e}"))?;
            let cur = r.GetCurrentMediaType(VIDEO).map_err(|e| e.to_string())?;
            let size = cur.GetUINT64(&MF_MT_FRAME_SIZE).map_err(|e| e.to_string())?;
            let (w, h) = ((size >> 32) as u32, (size & 0xFFFF_FFFF) as u32);
            let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(w as i32 * 4);
            let fps = match cur.GetUINT64(&MF_MT_FRAME_RATE) {
                Ok(v) if (v >> 32) > 0 && (v & 0xFFFF_FFFF) > 0 => ((v >> 32) as u32, (v & 0xFFFF_FFFF) as u32),
                _ => (30, 1),
            };
            let duration = duration_of(&r);
            Ok(Reader { r, w, h, bottom_up: stride < 0, fps, duration })
        }
    }

    /// True if the file has an audio track.
    pub fn has_audio(&self) -> bool {
        unsafe { self.r.GetNativeMediaType(AUDIO, 0).is_ok() }
    }

    /// Frames per second, rounded.
    pub fn fps_round(&self) -> u32 {
        ((self.fps.0 as f64 / self.fps.1.max(1) as f64).round() as u32).clamp(1, 1000)
    }

    /// Jumps near `t` (100 ns): decoding resumes at the keyframe before it.
    pub fn seek(&mut self, t: i64) -> bool {
        seek_to(&self.r, t)
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

/// The audio track as interleaved 16-bit PCM at its own rate and channels.
pub struct AudioReader {
    r: IMFSourceReader,
    pub rate: u32,
    pub channels: u32,
}

impl AudioReader {
    pub fn open(path: &Path) -> Result<AudioReader, String> {
        unsafe {
            let r = open_reader(path, false)?;
            let _ = r.SetStreamSelection(VIDEO, false);
            r.SetStreamSelection(AUDIO, true).map_err(|e| format!("no audio: {e}"))?;
            let native = r.GetNativeMediaType(AUDIO, 0).map_err(|e| format!("no audio: {e}"))?;
            let rate = native.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(48_000);
            let channels = native.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2).clamp(1, 2);
            let t = MFCreateMediaType().map_err(|e| e.to_string())?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(|e| e.to_string())?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM).map_err(|e| e.to_string())?;
            t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(|e| e.to_string())?;
            t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).map_err(|e| e.to_string())?;
            t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels).map_err(|e| e.to_string())?;
            t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, channels * 2).map_err(|e| e.to_string())?;
            t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * channels * 2).map_err(|e| e.to_string())?;
            r.SetCurrentMediaType(AUDIO, None, &t).map_err(|e| format!("pcm: {e}"))?;
            Ok(AudioReader { r, rate, channels })
        }
    }

    pub fn seek(&mut self, t: i64) -> bool {
        seek_to(&self.r, t)
    }

    /// Next block of samples and its start time (100 ns), or None at the end.
    pub fn next(&mut self) -> Option<(i64, Vec<i16>)> {
        loop {
            let mut flags = 0u32;
            let mut ts = 0i64;
            let mut sample = None;
            unsafe {
                self.r.ReadSample(AUDIO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample)).ok()?;
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
                let n = len as usize / 2;
                let pcm = std::slice::from_raw_parts(ptr as *const i16, n).to_vec();
                let _ = buf.Unlock();
                return Some((ts, pcm));
            }
        }
    }
}

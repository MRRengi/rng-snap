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
    /// Keeps the GPU decoder's device alive.
    _gpu: Option<IMFDXGIDeviceManager>,
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

/// A Direct3D 11 device for hardware decoding, behind an MF device manager.
fn gpu_manager() -> Option<IMFDXGIDeviceManager> {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::*;
    unsafe {
        let mut dev = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, None).ok()?;
        let dev: ID3D11Device = dev?;
        if let Ok(mt) = windows::core::Interface::cast::<ID3D11Multithread>(&dev) {
            let _ = mt.SetMultithreadProtected(true);
        }
        let mut token = 0u32;
        let mut mgr = None;
        MFCreateDXGIDeviceManager(&mut token, &mut mgr).ok()?;
        let mgr = mgr?;
        mgr.ResetDevice(&dev, token).ok()?;
        Some(mgr)
    }
}

/// How a reader decodes.
#[derive(Clone, Copy, Default)]
pub struct Opts {
    /// Decode on the GPU when there is one (falls back to the CPU).
    pub gpu: bool,
    /// Scale frames to this size while converting (the video processor does
    /// it, on the GPU when `gpu`). None keeps the file's size.
    pub size: Option<(u32, u32)>,
}

impl Reader {
    pub fn open(path: &Path) -> Result<Reader, String> {
        Self::open_inner(path, Opts::default())
    }

    /// Fast reader for previews: GPU decode and scaling, falling back to the
    /// plain one if the system can't.
    pub fn open_with(path: &Path, opts: Opts) -> Result<Reader, String> {
        if opts.gpu || opts.size.is_some() {
            if let Ok(mut r) = Self::open_inner(path, opts) {
                // Some drivers only fail on the first frame: try one.
                if r.next().is_some() {
                    r.seek(0);
                    return Ok(r);
                }
            }
            if opts.gpu {
                if let Ok(mut r) = Self::open_inner(path, Opts { gpu: false, ..opts }) {
                    if r.next().is_some() {
                        r.seek(0);
                        return Ok(r);
                    }
                }
            }
        }
        Self::open_inner(path, Opts::default())
    }

    fn open_inner(path: &Path, opts: Opts) -> Result<Reader, String> {
        unsafe {
            let advanced = opts.gpu || opts.size.is_some();
            let gpu = if opts.gpu { gpu_manager() } else { None };
            let r = if advanced {
                startup();
                let mut attrs = None;
                MFCreateAttributes(&mut attrs, 3).map_err(|e| e.to_string())?;
                let attrs = attrs.ok_or("attributes")?;
                attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1).map_err(|e| e.to_string())?;
                if let Some(m) = &gpu {
                    attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1).map_err(|e| e.to_string())?;
                    attrs.SetUnknown(&MF_SOURCE_READER_D3D_MANAGER, m).map_err(|e| e.to_string())?;
                }
                MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attrs).map_err(|e| format!("open: {e}"))?
            } else {
                open_reader(path, true)?
            };
            let _ = r.SetStreamSelection(AUDIO, false);
            let t = MFCreateMediaType().map_err(|e| e.to_string())?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| e.to_string())?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).map_err(|e| e.to_string())?;
            if let Some((w, h)) = opts.size {
                let (w, h) = (w.max(2) & !1, h.max(2) & !1);
                t.SetUINT64(&MF_MT_FRAME_SIZE, ((w as u64) << 32) | h as u64).map_err(|e| e.to_string())?;
                // Square pixels, the whole picture.
                let _ = t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1);
            }
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
            Ok(Reader { r, _gpu: gpu, w, h, bottom_up: stride < 0, fps, duration })
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
                // GPU and padded buffers: copy row by row with their pitch.
                if let Ok(b2) = windows::core::Interface::cast::<IMF2DBuffer>(&buf) {
                    let mut scan0 = std::ptr::null_mut();
                    let mut pitch = 0i32;
                    if b2.Lock2D(&mut scan0, &mut pitch).is_ok() {
                        let row = self.w as usize * 4;
                        let mut pixels = Vec::with_capacity(row * self.h as usize);
                        for y in 0..self.h as isize {
                            let src = scan0.offset(y * pitch as isize);
                            pixels.extend_from_slice(std::slice::from_raw_parts(src, row));
                        }
                        let _ = b2.Unlock2D();
                        return Some((ts, Frame { rect: Rect::from_xywh(0, 0, self.w as i32, self.h as i32), pixels }));
                    }
                }
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

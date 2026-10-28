//! Writes the edited video: the clips in their order, every frame through the
//! composer (zooms and annotations), the audio cut to the same clips with short
//! fades at the joins, into a hardware H.264 / HEVC MP4.

use super::mp4read::{AudioReader, Reader};
use super::util::log;
use crate::annot::Item;
use crate::annot::font::Fonts;
use crate::vedit::{self, Composer, Timeline};
use crate::videoplan::{self, Codec, Res};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::core::HSTRING;

pub struct Job {
    pub src: PathBuf,
    pub out: PathBuf,
    pub timeline: Timeline,
    pub items: Vec<Item>,
    pub fonts: Option<Arc<Fonts>>,
}

#[derive(Default)]
pub struct Progress {
    /// 0..=1000.
    pub permille: AtomicU32,
    pub cancel: AtomicBool,
}

fn w<T>(r: windows::core::Result<T>, what: &str) -> Result<T, String> {
    r.map_err(|e| format!("{what}: {e}"))
}

fn media_type(major: &windows::core::GUID, sub: &windows::core::GUID) -> Result<IMFMediaType, String> {
    unsafe {
        let t = w(MFCreateMediaType(), "media type")?;
        w(t.SetGUID(&MF_MT_MAJOR_TYPE, major), "major")?;
        w(t.SetGUID(&MF_MT_SUBTYPE, sub), "subtype")?;
        Ok(t)
    }
}

fn video_type(sub: &windows::core::GUID, ow: u32, oh: u32, fps: (u32, u32)) -> Result<IMFMediaType, String> {
    let t = media_type(&MFMediaType_Video, sub)?;
    unsafe {
        w(t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32), "interlace")?;
        w(t.SetUINT64(&MF_MT_FRAME_SIZE, (ow as u64) << 32 | oh as u64), "size")?;
        w(t.SetUINT64(&MF_MT_FRAME_RATE, (fps.0 as u64) << 32 | fps.1 as u64), "rate")?;
        w(t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, 1u64 << 32 | 1), "par")?;
    }
    Ok(t)
}

fn audio_type(aac: bool, rate: u32, ch: u32) -> Result<IMFMediaType, String> {
    let t = media_type(&MFMediaType_Audio, if aac { &MFAudioFormat_AAC } else { &MFAudioFormat_PCM })?;
    unsafe {
        w(t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16), "bits")?;
        w(t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate), "rate")?;
        w(t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, ch), "channels")?;
        if aac {
            w(t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000), "aac bitrate")?;
        } else {
            w(t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, ch * 2), "align")?;
            w(t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * ch * 2), "bytes")?;
        }
    }
    Ok(t)
}

fn write_bytes(writer: &IMFSinkWriter, stream: u32, bytes: &[u8], t: i64, d: i64) -> Result<(), String> {
    unsafe {
        let buf = w(MFCreateMemoryBuffer(bytes.len() as u32), "buffer")?;
        let mut ptr = std::ptr::null_mut();
        w(buf.Lock(&mut ptr, None, None), "lock")?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = buf.Unlock();
        w(buf.SetCurrentLength(bytes.len() as u32), "length")?;
        let s = w(MFCreateSample(), "sample")?;
        w(s.AddBuffer(&buf), "add buffer")?;
        w(s.SetSampleTime(t), "time")?;
        w(s.SetSampleDuration(d), "duration")?;
        w(writer.WriteSample(stream, &s), "write")
    }
}

/// Runs on its own thread. Returns the written file.
pub fn run(job: Job, prog: Arc<Progress>) -> Result<PathBuf, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, 0);
    }
    let part = job.out.with_extension("mp4.part");
    let r = export(&job, &prog, &part);
    if let Err(e) = &r {
        log(&format!("video export failed: {e}"));
        let _ = std::fs::remove_file(&part);
    }
    unsafe {
        let _ = MFShutdown();
    }
    r
}

fn export(job: &Job, prog: &Progress, part: &std::path::Path) -> Result<PathBuf, String> {
    let mut reader = Reader::open(&job.src)?;
    let fps = reader.fps;
    let plan = videoplan::plan(reader.w, reader.h, reader.fps_round(), Res::Original, super::record::hevc_available());
    let (ow, oh) = (plan.out_w, plan.out_h);
    let mut audio = if reader.has_audio() { AudioReader::open(&job.src).ok() } else { None };
    if let Some(a) = &audio {
        if ![44_100, 48_000].contains(&a.rate) {
            log(&format!("export: audio at {} Hz is not supported by AAC, dropping it", a.rate));
            audio = None;
        }
    }

    let writer = unsafe {
        let mut attrs = None;
        w(MFCreateAttributes(&mut attrs, 2), "attributes")?;
        let attrs = attrs.ok_or("attributes")?;
        w(attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1), "hw")?;
        w(attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4), "container")?;
        w(MFCreateSinkWriterFromURL(&HSTRING::from(part.as_os_str()), None, &attrs), "sink writer")?
    };
    let sub = if plan.codec == Codec::Hevc { MFVideoFormat_HEVC } else { MFVideoFormat_H264 };
    let out_t = video_type(&sub, ow, oh, fps)?;
    unsafe {
        w(out_t.SetUINT32(&MF_MT_AVG_BITRATE, plan.bitrate), "bitrate")?;
        if plan.codec == Codec::H264 {
            let _ = out_t.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32);
        }
    }
    let vs = unsafe { w(writer.AddStream(&out_t), "video stream")? };
    let in_t = video_type(&MFVideoFormat_RGB32, ow, oh, fps)?;
    unsafe {
        let _ = in_t.SetUINT32(&MF_MT_DEFAULT_STRIDE, ow * 4);
        let _ = in_t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);
        w(writer.SetInputMediaType(vs, &in_t, None), "video input")?;
    }
    let mut astream = None;
    if let Some(a) = &audio {
        unsafe {
            match writer.AddStream(&audio_type(true, a.rate, a.channels)?) {
                Ok(i) => match writer.SetInputMediaType(i, &audio_type(false, a.rate, a.channels)?, None) {
                    Ok(()) => astream = Some(i),
                    Err(e) => log(&format!("export audio input: {e}")),
                },
                Err(e) => log(&format!("export audio stream: {e}")),
            }
        }
    }
    unsafe { w(writer.BeginWriting(), "begin writing")? };

    let tl = &job.timeline;
    // Audio first: it is small, and the muxer interleaves for us.
    if let (Some(a), Some(st)) = (audio.as_mut(), astream) {
        let (rate, ch) = (a.rate as i64, a.channels as usize);
        let mut pos: i64 = 0;
        for c in &tl.clips {
            let want = c.len() as i64 * rate / 1000;
            let (ca, cb) = (c.a as i64 * 10_000, c.b as i64 * 10_000);
            let mut buf: Vec<i16> = Vec::with_capacity(want as usize * ch);
            a.seek(ca);
            while let Some((ts, pcm)) = a.next() {
                let frames = (pcm.len() / ch) as i64;
                let end = ts + frames * 10_000_000 / rate;
                if end <= ca {
                    continue;
                }
                let skip = if ts < ca { ((ca - ts) * rate / 10_000_000).clamp(0, frames) } else { 0 };
                let take = if end > cb { ((cb - ts) * rate / 10_000_000).clamp(skip, frames) } else { frames };
                buf.extend_from_slice(&pcm[skip as usize * ch..take as usize * ch]);
                if end >= cb || buf.len() >= want as usize * ch {
                    break;
                }
            }
            buf.resize(want as usize * ch, 0);
            vedit::edge_fades(&mut buf, ch, (rate / 200) as usize);
            for chunk in buf.chunks(rate as usize * ch) {
                let n = (chunk.len() / ch) as i64;
                let bytes = unsafe { std::slice::from_raw_parts(chunk.as_ptr() as *const u8, chunk.len() * 2) };
                write_bytes(&writer, st, bytes, pos * 10_000_000 / rate, n * 10_000_000 / rate)?;
                pos += n;
            }
        }
    }

    // Video: output frame k shows output time k / fps.
    let (num, den) = (fps.0 as i64, fps.1.max(1) as i64);
    let frame_t = |k: i64| k * 10_000_000 * den / num;
    let total = tl.total() as i64 * 10_000;
    let n_frames = ((total * num + 10_000_000 * den - 1) / (10_000_000 * den)).max(1);
    let mut composer = Composer::default();
    let fonts = job.fonts.as_deref();
    let mut k: i64 = 0;
    for (i, c) in tl.clips.iter().enumerate() {
        let start = tl.start_of(i) as i64 * 10_000;
        let end = start + c.len() as i64 * 10_000;
        if frame_t(k) >= end {
            continue;
        }
        reader.seek(c.a as i64 * 10_000);
        let mut cur: Option<(i64, crate::image::Frame)> = None;
        let mut next = reader.next();
        let half = 5_000_000 * den / num;
        while k < n_frames && frame_t(k) < end {
            if prog.cancel.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            let src_t = c.a as i64 * 10_000 + (frame_t(k) - start);
            while let Some((ts, _)) = &next {
                if *ts > src_t + half {
                    break;
                }
                cur = next.take();
                next = reader.next();
            }
            if cur.is_none() {
                cur = next.take();
                next = reader.next();
            }
            let Some((_, f)) = &cur else { break };
            let out = composer.compose(f, &job.items, (frame_t(k) / 10_000) as u32, ow, oh, fonts);
            write_bytes(&writer, vs, &out.pixels, frame_t(k), frame_t(k + 1) - frame_t(k))?;
            k += 1;
            prog.permille.store(((k * 1000) / n_frames).min(999) as u32, Ordering::Release);
        }
    }
    if k == 0 {
        return Err("no frames decoded".into());
    }
    unsafe { w(writer.Finalize(), "finalize")? };
    drop(writer);
    std::fs::rename(part, &job.out).map_err(|e| format!("rename: {e}"))?;
    prog.permille.store(1000, Ordering::Release);
    Ok(job.out.clone())
}

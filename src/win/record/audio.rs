//! Audio capture: system sound (WASAPI loopback) and the default microphone,
//! both asked for 48 kHz float stereo (Windows converts), placed on one
//! timeline by their QPC timestamps and handed out as 16-bit PCM chunks.

use super::Control;
use crate::mixer::{Mixer, Track};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

pub const RATE: u32 = 48_000;
/// How far behind "now" the mixer emits, so late packets still land in place.
const LAG_100NS: i64 = 1_500_000;

/// PCM chunk: interleaved stereo i16 starting at `pos` frames on the timeline.
pub struct Chunk {
    pub pos: u64,
    pub pcm: Vec<i16>,
}

struct Source {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    is_mic: bool,
}

fn open(flow: EDataFlow, loopback: bool) -> Result<Source, String> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let dev = en.GetDefaultAudioEndpoint(flow, eConsole).map_err(|e| e.to_string())?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;
        let fmt = WAVEFORMATEX {
            wFormatTag: 3, // WAVE_FORMAT_IEEE_FLOAT
            nChannels: 2,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        let mut flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        if loopback {
            flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
        }
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &fmt, None).map_err(|e| e.to_string())?;
        let capture: IAudioCaptureClient = client.GetService().map_err(|e| e.to_string())?;
        client.Start().map_err(|e| e.to_string())?;
        Ok(Source { client, capture, is_mic: !loopback })
    }
}

fn qpc_100ns() -> i64 {
    let mut c = 0i64;
    let mut f = 0i64;
    unsafe {
        let _ = windows::Win32::System::Performance::QueryPerformanceCounter(&mut c);
        let _ = windows::Win32::System::Performance::QueryPerformanceFrequency(&mut f);
    }
    (c as i128 * 10_000_000 / f.max(1) as i128) as i64
}

pub fn now_100ns() -> i64 {
    qpc_100ns()
}

/// Opens the requested sources and runs the capture loop on this thread.
/// `ready` gets whether any source works; chunks go to `out` until stop.
pub fn run(system: bool, mic: bool, ctrl: Arc<Control>, ready: mpsc::Sender<bool>, out: mpsc::Sender<Chunk>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut sources = Vec::new();
    if system {
        match open(eRender, true) {
            Ok(s) => sources.push(s),
            Err(e) => super::log(&format!("system sound unavailable: {e}")),
        }
    }
    if mic {
        match open(eCapture, false) {
            Ok(s) => sources.push(s),
            Err(e) => super::log(&format!("microphone unavailable: {e}")),
        }
    }
    let _ = ready.send(!sources.is_empty());
    if sources.is_empty() {
        return;
    }
    // Wait for the video clock to start.
    while ctrl.start_100ns.load(Ordering::Acquire) == 0 {
        if ctrl.stop.load(Ordering::Acquire) {
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let start = ctrl.start_100ns.load(Ordering::Acquire);
    let mut mixer = Mixer::new();
    // One continuous track per source; resync past 20 ms of drift.
    let mut tracks: Vec<Track> = sources.iter().map(|_| Track::new(RATE as i64 / 50)).collect();
    let mut was_paused = false;
    let to_frames = |t: i64| -> i64 { (t as i128 * RATE as i128 / 10_000_000) as i64 };
    loop {
        let stopping = ctrl.stop.load(Ordering::Acquire);
        let paused = ctrl.paused.load(Ordering::Acquire) || ctrl.auto_paused.load(Ordering::Acquire);
        if paused != was_paused {
            tracks.iter_mut().for_each(Track::reset);
            was_paused = paused;
        }
        let paused_total = ctrl.paused_total_100ns.load(Ordering::Acquire);
        let muted = ctrl.mic_muted.load(Ordering::Acquire);
        for (s, track) in sources.iter().zip(tracks.iter_mut()) {
            loop {
                let n = unsafe { s.capture.GetNextPacketSize() }.unwrap_or(0);
                if n == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                let mut qpc = 0u64;
                if unsafe { s.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc)) }.is_err() {
                    break;
                }
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                let broke = flags & (AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32) != 0;
                if !paused {
                    // Silent packets still move the track along.
                    let pos = track.place(to_frames(qpc as i64 - start - paused_total), frames as usize, broke);
                    if !silent && !(s.is_mic && muted) && !data.is_null() {
                        let samples = unsafe { std::slice::from_raw_parts(data as *const f32, frames as usize * 2) };
                        mixer.add(pos, samples);
                    }
                }
                unsafe {
                    let _ = s.capture.ReleaseBuffer(frames);
                }
            }
        }
        let lag = if stopping { 0 } else { LAG_100NS };
        let elapsed = now_100ns() - start - paused_total - lag;
        if !paused && elapsed > 0 {
            let pos = mixer.emitted();
            let target = to_frames(elapsed) as u64;
            let pcm = mixer.take_until(target);
            if !pcm.is_empty() && out.send(Chunk { pos, pcm }).is_err() {
                break;
            }
        }
        if stopping {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    for s in &sources {
        unsafe {
            let _ = s.client.Stop();
        }
    }
}

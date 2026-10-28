//! Plays the edited video's sound while the preview plays: the clips in their
//! order, decoded from the source and fed to the default speakers (WASAPI,
//! shared mode). Without a sound device it simply stays silent.

use super::mp4read::AudioReader;
use crate::vedit::{self, Timeline};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

pub struct AudioPlay {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl AudioPlay {
    /// Starts playing output time `from_ms` onwards.
    pub fn start(src: PathBuf, timeline: Timeline, from_ms: u32) -> AudioPlay {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let handle = std::thread::spawn(move || {
            if let Err(e) = play(&src, &timeline, from_ms, &s) {
                super::util::log(&format!("preview sound off: {e}"));
            }
        });
        AudioPlay { stop, handle: Some(handle) }
    }
}

impl Drop for AudioPlay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Source ranges (100 ns) still to play, from output time `from_ms`.
fn ranges(tl: &Timeline, from_ms: u32) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for (i, c) in tl.clips.iter().enumerate() {
        let start = tl.start_of(i);
        let end = start + c.len();
        if end <= from_ms {
            continue;
        }
        let a = c.a + from_ms.saturating_sub(start);
        out.push((a as i64 * 10_000, c.b as i64 * 10_000));
    }
    out
}

fn play(src: &PathBuf, tl: &Timeline, from_ms: u32, stop: &AtomicBool) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut ar = AudioReader::open(src)?;
    let (rate, ch) = (ar.rate, ar.channels as usize);
    let (client, render) = unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).map_err(|e| e.to_string())?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;
        let fmt = WAVEFORMATEX {
            wFormatTag: 1, // PCM
            nChannels: ch as u16,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: rate * ch as u32 * 2,
            nBlockAlign: ch as u16 * 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &fmt, None).map_err(|e| e.to_string())?;
        let render: IAudioRenderClient = client.GetService().map_err(|e| e.to_string())?;
        (client, render)
    };
    let buffer = unsafe { client.GetBufferSize().map_err(|e| e.to_string())? } as usize;
    let fade = (rate / 200) as usize;
    let mut todo: VecDeque<(i64, i64)> = ranges(tl, from_ms).into();
    let mut q: VecDeque<i16> = VecDeque::new();
    // Decodes the next range into the queue; false when everything is queued.
    let mut feed = |q: &mut VecDeque<i16>| -> bool {
        let Some((a, b)) = todo.pop_front() else { return false };
        let mut clip: Vec<i16> = Vec::new();
        ar.seek(a);
        while let Some((ts, pcm)) = ar.next() {
            let frames = (pcm.len() / ch) as i64;
            let end = ts + frames * 10_000_000 / rate as i64;
            if end <= a {
                continue;
            }
            let skip = if ts < a { ((a - ts) * rate as i64 / 10_000_000).clamp(0, frames) } else { 0 };
            let take = if end > b { ((b - ts) * rate as i64 / 10_000_000).clamp(skip, frames) } else { frames };
            clip.extend_from_slice(&pcm[skip as usize * ch..take as usize * ch]);
            if end >= b {
                break;
            }
        }
        vedit::edge_fades(&mut clip, ch, fade);
        q.extend(clip);
        true
    };
    let mut more = feed(&mut q);
    let mut started = false;
    while !stop.load(Ordering::Acquire) {
        if q.len() < rate as usize * ch && more {
            more = feed(&mut q);
        }
        let pad = unsafe { client.GetCurrentPadding().unwrap_or(0) } as usize;
        let n = (buffer - pad).min(q.len() / ch);
        if n > 0 {
            unsafe {
                if let Ok(p) = render.GetBuffer(n as u32) {
                    let dst = std::slice::from_raw_parts_mut(p as *mut i16, n * ch);
                    for d in dst.iter_mut() {
                        *d = q.pop_front().unwrap_or(0);
                    }
                    let _ = render.ReleaseBuffer(n as u32, 0);
                }
            }
        }
        if !started {
            unsafe {
                client.Start().map_err(|e| e.to_string())?;
            }
            started = true;
        }
        if q.is_empty() && !more && pad == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unsafe {
        let _ = client.Stop();
    }
    Ok(())
}

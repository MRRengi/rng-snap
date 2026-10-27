//! Screen recording: Windows.Graphics.Capture -> (GPU scale) -> Media
//! Foundation hardware encoder, plus optional WASAPI audio.

mod audio;
mod video;

use crate::geom::Rect;
use crate::image::Frame;
use crate::videoplan::Plan;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, mpsc};

pub use video::{hevc_available, refresh_rate};

pub fn log(msg: &str) {
    super::util::log(msg);
}

pub struct Params {
    pub monitor: isize,
    pub monitor_rect: Rect,
    /// Desktop coords, even size, inside the monitor.
    pub region: Rect,
    pub plan: Plan,
    pub path: PathBuf,
    pub system_audio: bool,
    pub mic: bool,
}

/// Shared between the UI and the recording threads.
#[derive(Default)]
pub struct Control {
    pub stop: AtomicBool,
    pub discard: AtomicBool,
    pub paused: AtomicBool,
    pub mic_muted: AtomicBool,
    /// QPC time (100 ns) of video frame zero; 0 until recording starts.
    pub start_100ns: AtomicI64,
    pub paused_total_100ns: AtomicI64,
}

pub struct Summary {
    pub path: PathBuf,
    pub seconds: f64,
    pub frames: u64,
    pub plan: Plan,
    pub thumb: Option<Frame>,
    pub audio: bool,
}

pub struct Recorder {
    pub ctrl: Arc<Control>,
    thread: Option<std::thread::JoinHandle<Result<Summary, String>>>,
    paused_at: Option<i64>,
}

impl Recorder {
    /// Starts recording; returns once the encoder is ready (or failed).
    pub fn start(p: Params) -> Result<Recorder, String> {
        let ctrl = Arc::new(Control::default());
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let c = ctrl.clone();
        let thread = std::thread::spawn(move || video::run(p, c, ready_tx));
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Recorder { ctrl, thread: Some(thread), paused_at: None }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err(match thread.join() {
                Ok(Err(e)) => e,
                _ => "recorder thread ended".into(),
            }),
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn set_paused(&mut self, on: bool) {
        let now = audio::now_100ns();
        match (on, self.paused_at) {
            (true, None) => {
                self.paused_at = Some(now);
                self.ctrl.paused.store(true, Ordering::Release);
            }
            (false, Some(at)) => {
                self.ctrl.paused_total_100ns.fetch_add(now - at, Ordering::AcqRel);
                self.paused_at = None;
                self.ctrl.paused.store(false, Ordering::Release);
            }
            _ => {}
        }
    }

    pub fn set_mic_muted(&self, on: bool) {
        self.ctrl.mic_muted.store(on, Ordering::Release);
    }

    pub fn mic_muted(&self) -> bool {
        self.ctrl.mic_muted.load(Ordering::Acquire)
    }

    /// Seconds recorded so far (excluding pauses).
    pub fn elapsed(&self) -> f64 {
        let start = self.ctrl.start_100ns.load(Ordering::Acquire);
        if start == 0 {
            return 0.0;
        }
        let end = self.paused_at.unwrap_or_else(audio::now_100ns);
        (end - start - self.ctrl.paused_total_100ns.load(Ordering::Acquire)).max(0) as f64 / 1e7
    }

    /// Stops and finalizes the file (or deletes it when `discard`).
    pub fn finish(mut self, discard: bool) -> Result<Summary, String> {
        if self.paused_at.is_some() {
            self.set_paused(false);
        }
        self.ctrl.discard.store(discard, Ordering::Release);
        self.ctrl.stop.store(true, Ordering::Release);
        match self.thread.take().map(|t| t.join()) {
            Some(Ok(r)) => r,
            _ => Err("recorder thread panicked".into()),
        }
    }
}

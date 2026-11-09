//! Video editor workers: the player (decodes and composes the preview frame
//! for a position, or plays in real time) and the strip maker (thumbnails and
//! the audio waveform for the timeline).

use super::mp4read::{AudioReader, Opts, Reader};
use crate::annot::Item;
use crate::annot::font::Fonts;
use crate::image::Frame;
use crate::vedit::{self, Cam, Composer, Timeline, coarse_to_fine, fit};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// What the preview shows.
#[derive(Clone)]
pub struct View {
    pub timeline: Timeline,
    pub items: Vec<Item>,
    /// Preview size in pixels.
    pub ow: u32,
    pub oh: u32,
}

pub enum Cmd {
    /// Show output time `t` (paused).
    Show(u32),
    Play(u32),
    Pause,
    View(View),
}

#[derive(Default)]
pub struct Shared {
    /// Latest preview frame and its output time.
    pub frame: Mutex<Option<(u32, Arc<Frame>)>>,
    pub playing: AtomicBool,
    /// Output time of the latest frame.
    pub t: AtomicU32,
    /// Bumped with every new frame.
    pub seq: AtomicU32,
}

pub struct Player {
    tx: Sender<Cmd>,
    pub shared: Arc<Shared>,
}

impl Player {
    /// Starts the player thread; it posts `msg` to `notify` for every frame.
    pub fn start(src: PathBuf, view: View, fonts: Option<Arc<Fonts>>, notify: HWND, msg: u32) -> Result<Player, String> {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());
        let sh = shared.clone();
        let hw = notify.0 as isize;
        std::thread::spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            // COM objects stay on the thread that made them. GPU decode, and
            // big sources (4K) come in at most 1920x1080: plenty for a preview.
            let size = Reader::open(&src).ok().map(|r| fit(r.w, r.h, 1920, 1080)).filter(|&(w, h)| w > 0 && h > 0);
            let Ok(reader) = Reader::open_with(&src, Opts { gpu: true, size }) else { return };
            super::util::log(&format!("preview decoder: {}x{}", reader.w, reader.h));
            Worker { reader, view, fonts, sh, notify: hw, msg, composer: Composer::default(), raw: None, next: None, stats: Stats::default() }.run(rx)
        });
        let p = Player { tx, shared };
        p.send(Cmd::Show(0));
        Ok(p)
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    pub fn playing(&self) -> bool {
        self.shared.playing.load(Ordering::Acquire)
    }
}

struct Worker {
    reader: Reader,
    view: View,
    fonts: Option<Arc<Fonts>>,
    sh: Arc<Shared>,
    notify: isize,
    msg: u32,
    composer: Composer,
    /// Last decoded source frame: (clip, timestamp 100 ns, frame).
    raw: Option<(usize, i64, Arc<Frame>)>,
    next: Option<(i64, Frame)>,
    stats: Stats,
}

/// Playback timings, logged in test runs (RENGI_SNAP_HOME) every 2 s.
#[derive(Default)]
struct Stats {
    since: Option<Instant>,
    frames: u32,
    decode: Duration,
    compose: Duration,
}

impl Stats {
    fn add(&mut self, decode: Duration, compose: Duration, w: u32, h: u32) {
        if std::env::var_os("RENGI_SNAP_HOME").is_none() {
            return;
        }
        let since = *self.since.get_or_insert_with(Instant::now);
        self.frames += 1;
        self.decode += decode;
        self.compose += compose;
        let el = since.elapsed();
        if el >= Duration::from_secs(2) {
            let n = self.frames.max(1);
            super::util::log(&format!(
                "preview: {:.1} fps, decode {:.1} ms, compose {:.1} ms, {w}x{h}",
                self.frames as f32 / el.as_secs_f32(),
                self.decode.as_secs_f32() * 1000.0 / n as f32,
                self.compose.as_secs_f32() * 1000.0 / n as f32
            ));
            *self = Stats::default();
        }
    }
}

impl Worker {
    fn run(mut self, rx: Receiver<Cmd>) {
        let mut cur_t = 0u32;
        let mut play: Option<(Instant, u32)> = None;
        loop {
            let cmd = match play {
                None => match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                },
                Some(_) => {
                    let frame_ms = (1000 * self.reader.fps.1 as u64 / self.reader.fps.0.max(1) as u64).clamp(4, 100);
                    match rx.recv_timeout(Duration::from_millis(frame_ms / 2)) {
                        Ok(c) => Some(c),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            };
            // Only the newest Show matters; Views are applied in order.
            let mut cmds: Vec<Cmd> = cmd.into_iter().collect();
            while let Ok(c) = rx.try_recv() {
                cmds.push(c);
            }
            let mut want_show = None;
            let mut view_changed = false;
            for c in cmds {
                match c {
                    Cmd::Show(t) => {
                        play = None;
                        want_show = Some(t);
                    }
                    Cmd::Play(t) => {
                        play = Some((Instant::now(), t));
                        want_show = None;
                    }
                    Cmd::Pause => {
                        if play.take().is_some() {
                            want_show = Some(cur_t);
                        }
                    }
                    Cmd::View(v) => {
                        self.view = v;
                        view_changed = true;
                    }
                }
            }
            self.sh.playing.store(play.is_some(), Ordering::Release);
            let total = self.view.timeline.total();
            if let Some((start, t0)) = play {
                let t = t0 + start.elapsed().as_millis() as u32;
                if t >= total {
                    play = None;
                    self.sh.playing.store(false, Ordering::Release);
                    cur_t = total.saturating_sub(1);
                    self.show(cur_t);
                } else {
                    cur_t = t;
                    self.show(t);
                }
            } else if let Some(t) = want_show {
                cur_t = t.min(total.saturating_sub(1));
                self.show(cur_t);
            } else if view_changed {
                self.show(cur_t.min(total.saturating_sub(1)));
            }
        }
    }

    /// Decodes (if needed), composes and publishes the frame for output time `t`.
    fn show(&mut self, t: u32) {
        let t_dec = Instant::now();
        let (clip, src_ms) = self.view.timeline.locate(t);
        let src = src_ms as i64 * 10_000;
        let half = 5_000_000 * self.reader.fps.1 as i64 / self.reader.fps.0.max(1) as i64;
        let near = matches!(&self.raw, Some((c, ts, _)) if *c == clip && *ts <= src + half && src - *ts < 20_000_000);
        if !near {
            self.reader.seek(src);
            self.raw = None;
            self.next = self.reader.next();
        }
        while let Some((ts, _)) = &self.next {
            if *ts > src + half {
                break;
            }
            let (ts, f) = self.next.take().unwrap();
            self.raw = Some((clip, ts, Arc::new(f)));
            self.next = self.reader.next();
        }
        if self.raw.is_none() {
            if let Some((ts, f)) = self.next.take() {
                self.raw = Some((clip, ts, Arc::new(f)));
                self.next = self.reader.next();
            }
        }
        let Some((_, _, raw)) = &self.raw else { return };
        let decode = t_dec.elapsed();
        let t_comp = Instant::now();
        let out = self.composer.compose(raw, &self.view.items, t, self.view.ow.max(2), self.view.oh.max(2), self.fonts.as_deref());
        if self.sh.playing.load(Ordering::Acquire) {
            let (w, h) = (out.width() as u32, out.height() as u32);
            self.stats.add(decode, t_comp.elapsed(), w, h);
        }
        if let Ok(mut g) = self.sh.frame.lock() {
            *g = Some((t, Arc::new(out)));
        }
        self.sh.t.store(t, Ordering::Release);
        self.sh.seq.fetch_add(1, Ordering::AcqRel);
        unsafe {
            let _ = PostMessageW(Some(HWND(self.notify as *mut _)), self.msg, WPARAM(0), LPARAM(0));
        }
    }
}

/// Timeline strips: small frames across the source (sorted by time), and the
/// audio peaks every PEAK_MS.
#[derive(Default)]
pub struct Strips {
    pub thumbs: Mutex<Vec<(u32, Arc<Frame>)>>,
    pub peaks: Mutex<Vec<u8>>,
    pub done: AtomicBool,
}

pub const PEAK_MS: u32 = 20;

/// Fills `strips` in the background, posting `msg` to `notify` as it goes.
pub fn make_strips(src: PathBuf, duration_ms: u32, thumb_h: u32, strips: Arc<Strips>, notify: HWND, msg: u32) {
    let hw = notify.0 as isize;
    std::thread::spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let post = || unsafe {
            let _ = PostMessageW(Some(HWND(hw as *mut _)), msg, WPARAM(0), LPARAM(0));
        };
        // Audio first: it is quick and makes the timeline readable at once.
        if let Ok(mut a) = AudioReader::open(&src) {
            let bucket = (a.rate * PEAK_MS / 1000).max(1) as usize;
            let ch = a.channels as usize;
            let mut carry: Vec<i16> = Vec::new();
            while let Some((_, pcm)) = a.next() {
                carry.extend_from_slice(&pcm);
                let whole = carry.len() / (bucket * ch) * bucket * ch;
                if whole > 0 {
                    let p = vedit::peaks(&carry[..whole], ch, bucket);
                    if let Ok(mut g) = strips.peaks.lock() {
                        g.extend(p);
                    }
                    carry.drain(..whole);
                }
            }
            post();
        }
        // Frames: seek to each slot (keyframe, then forward to it) at a small
        // size the video processor makes, filling the whole length first.
        let Ok(probe) = Reader::open(&src) else {
            strips.done.store(true, Ordering::Release);
            return;
        };
        let th = thumb_h.max(8);
        let tw = ((probe.w as f32 / probe.h.max(1) as f32) * th as f32).round().max(8.0) as u32;
        drop(probe);
        let Ok(mut r) = Reader::open_with(&src, Opts { gpu: true, size: Some((tw, th)) }) else {
            strips.done.store(true, Ordering::Release);
            return;
        };
        let n = ((duration_ms / 250) as usize).clamp(2, 240);
        let started = Instant::now();
        let mut last_post = Instant::now();
        for slot in coarse_to_fine(n) {
            let target = (duration_ms as u64 * slot as u64 / n as u64) as i64 * 10_000;
            r.seek(target);
            let mut got = None;
            while let Some((ts, f)) = r.next() {
                let done = ts + 5_000_000 / 30 >= target;
                got = Some((ts, f));
                if done {
                    break;
                }
            }
            let Some((ts, f)) = got else { continue };
            let small = if f.width() as u32 == tw && f.height() as u32 == th { f } else { vedit::resample(&f, Cam::full(f.width() as f32, f.height() as f32), tw, th) };
            let ms = (ts / 10_000).max(0) as u32;
            if let Ok(mut g) = strips.thumbs.lock() {
                let at = g.partition_point(|(m, _)| *m < ms);
                if g.get(at).map(|(m, _)| *m) != Some(ms) {
                    g.insert(at, (ms, Arc::new(small)));
                }
            }
            if last_post.elapsed() > Duration::from_millis(120) {
                post();
                last_post = Instant::now();
            }
        }
        super::util::log(&format!("strips: {n} thumbnails in {} ms", started.elapsed().as_millis()));
        strips.done.store(true, Ordering::Release);
        post();
    });
}


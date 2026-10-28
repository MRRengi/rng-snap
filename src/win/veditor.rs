//! Video editor window: the preview (annotations are drawn and edited right
//! on it), a transport row, and the timeline: clips that can be split,
//! deleted, trimmed and dragged into a new order, plus a track with every
//! annotation and zoom as a bar that can be moved and stretched in time.

use super::class_name;
use super::d2d::{self, Gfx, rect_f, rgba};
use super::mp4read::Reader;
use super::overlay::ACCENT;
use super::toolstrip::{Btn, Slot, Strip, StripState};
use super::util::{lparam_point, log};
use super::vaudio::AudioPlay;
use super::vexport::{self, Job, Progress};
use super::vplayer::{Cmd, PEAK_MS, Player, Strips, View, make_strips};
use crate::annot::edit::{Canvas, Change, Key};
use crate::annot::font::{self, Fonts};
use crate::annot::render::bbox;
use crate::annot::{BlurMode, Color, Item, P, PALETTE, Shape, Tool};
use crate::geom::{Point, Rect};
use crate::vedit::{self, Cam, Clip, MIN_CLIP_MS, Timeline};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// A new preview frame is ready.
pub const WM_VFRAME: u32 = WM_APP + 40;
/// More thumbnails / waveform arrived.
pub const WM_VSTRIPS: u32 = WM_APP + 41;
const TICK_TIMER: usize = 1;
const STATUS_TIMER: usize = 2;
const BG: u32 = 0x161618;
const PANEL: u32 = 0x1E1E20;
const ZOOM_COLOR: u32 = 0xA78BFA;
/// Pixels around a clip or bar edge that grab the edge instead of the body.
const EDGE_PX: f32 = 7.0;

pub enum Event {
    CustomColor,
    /// A finished export (already on the clipboard).
    Exported(PathBuf),
}

pub const VSLOTS: [Slot; 32] = [
    Slot::B(Btn::Tool(Tool::Select)),
    Slot::B(Btn::Tool(Tool::Arrow)),
    Slot::B(Btn::Tool(Tool::Line)),
    Slot::B(Btn::Tool(Tool::Rect)),
    Slot::B(Btn::Tool(Tool::Ellipse)),
    Slot::B(Btn::Tool(Tool::Pen)),
    Slot::B(Btn::Tool(Tool::Highlighter)),
    Slot::B(Btn::Tool(Tool::Text)),
    Slot::B(Btn::Tool(Tool::Blur)),
    Slot::B(Btn::Tool(Tool::Step)),
    Slot::B(Btn::Tool(Tool::Zoom)),
    Slot::Sep,
    Slot::B(Btn::Color(0)),
    Slot::B(Btn::Color(1)),
    Slot::B(Btn::Color(2)),
    Slot::B(Btn::Color(3)),
    Slot::B(Btn::Color(4)),
    Slot::B(Btn::Color(5)),
    Slot::B(Btn::Color(6)),
    Slot::B(Btn::Color(7)),
    Slot::B(Btn::Custom),
    Slot::Sep,
    Slot::B(Btn::Width(0)),
    Slot::B(Btn::Width(1)),
    Slot::B(Btn::Width(2)),
    Slot::B(Btn::BlurMode),
    Slot::Sep,
    Slot::B(Btn::Undo),
    Slot::B(Btn::Redo),
    Slot::Spring,
    Slot::B(Btn::Gif),
    Slot::B(Btn::Export),
];

#[derive(Clone, PartialEq)]
struct Snap {
    timeline: Timeline,
    items: Vec<Item>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum SpanMode {
    Move,
    Start,
    End,
}

#[derive(Clone, Copy)]
enum Drag {
    None,
    Canvas,
    Scrub,
    Clip { idx: usize, x0: f32, moved: bool, target: usize },
    Trim { idx: usize, right: bool, x0: f32, orig: Clip, ms_px: f32 },
    Span { idx: usize, mode: SpanMode, x0: f32, orig: (u32, u32), ms_px: f32 },
}

struct Work {
    prog: Arc<Progress>,
    handle: JoinHandle<Result<PathBuf, String>>,
    /// Export only: make a GIF of the result when done.
    then_gif: bool,
    gif: bool,
}

/// Areas of the window, in client pixels.
#[derive(Clone, Copy, Default)]
struct Lay {
    bar: f32,
    /// Where the video picture is drawn.
    pic: D2D_RECT_F,
    transport: D2D_RECT_F,
    play: D2D_RECT_F,
    split: D2D_RECT_F,
    delete: D2D_RECT_F,
    ruler: D2D_RECT_F,
    clips: D2D_RECT_F,
    items: D2D_RECT_F,
    x0: f32,
    x1: f32,
}

pub struct VEditor {
    pub hwnd: HWND,
    rt: Option<ID2D1HwndRenderTarget>,
    rt_size: (u32, u32),
    scale: f32,
    strip: Strip,
    pub canvas: Canvas,
    timeline: Timeline,
    pub src: PathBuf,
    src_w: u32,
    src_h: u32,
    fps: (u32, u32),
    player: Player,
    strips: Arc<Strips>,
    frame_bmp: Option<(u32, ID2D1Bitmap)>,
    thumb_bmps: HashMap<u32, ID2D1Bitmap>,
    playhead: u32,
    sel_clip: Option<usize>,
    drag: Drag,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    last: Snap,
    edits: u64,
    work: Option<Work>,
    last_export: Option<(u64, PathBuf)>,
    status: String,
    fonts_ui: Option<(f32, IDWriteTextFormat, IDWriteTextFormat, IDWriteTextFormat)>,
    pub custom: Color,
    caret_on: bool,
    last_cursor: Point,
    view_size: (u32, u32),
    min_w: i32,
    /// Preview sound while playing.
    audio: Option<AudioPlay>,
}

fn dpi_scale(h: HWND) -> f32 {
    (unsafe { GetDpiForWindow(h) }.max(96)) as f32 / 96.0
}

fn inside(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

impl VEditor {
    pub fn open(instance: HINSTANCE, gfx: &Gfx, src: PathBuf, fonts: Option<Arc<Fonts>>, style: (Tool, Color, usize)) -> Result<VEditor, String> {
        let info = Reader::open(&src)?;
        let (src_w, src_h, fps) = (info.w, info.h, info.fps);
        let duration_ms = (info.duration / 10_000).max(0) as u32;
        drop(info);
        if duration_ms < 200 {
            return Err("this video is too short to edit".into());
        }
        let title = super::util::wide(&format!("{} - Rengi Snap video editor", src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name::VEDITOR,
                windows::core::PCWSTR(title.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                1000,
                700,
                None,
                None,
                Some(instance),
                None,
            )
            .map_err(|e| e.to_string())?
        };
        let scale = dpi_scale(hwnd);
        unsafe {
            let dark = 1u32;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
        }
        let timeline = Timeline::new(duration_ms);
        // Strokes and text sized for the video, not the screen.
        let mut canvas = Canvas::new(Rect::new(0, 0, src_w as i32, src_h as i32), (src_h as f32 / 720.0).max(1.0), fonts.clone());
        canvas.tool = if style.0 == Tool::Crop { Tool::Select } else { style.0 };
        canvas.color = style.1;
        canvas.width_idx = style.2;
        canvas.time = Some(0);
        canvas.span_len = 3000;
        let view = View { timeline: timeline.clone(), items: Vec::new(), ow: 640, oh: (640 * src_h / src_w.max(1)).max(2) };
        let player = Player::start(src.clone(), view, fonts, hwnd, WM_VFRAME)?;
        let strips = Arc::new(Strips::default());
        make_strips(src.clone(), duration_ms, (44.0 * scale) as u32, strips.clone(), hwnd, WM_VSTRIPS);
        let last = Snap { timeline: timeline.clone(), items: Vec::new() };
        let mut ed = VEditor {
            hwnd,
            rt: None,
            rt_size: (0, 0),
            scale,
            strip: Strip::new(),
            canvas,
            timeline,
            src,
            src_w,
            src_h,
            fps,
            player,
            strips,
            frame_bmp: None,
            thumb_bmps: HashMap::new(),
            playhead: 0,
            sel_clip: None,
            drag: Drag::None,
            undo: Vec::new(),
            redo: Vec::new(),
            last,
            edits: 0,
            work: None,
            last_export: None,
            status: String::new(),
            fonts_ui: None,
            custom: Color(0xFF, 0x4F, 0xB0),
            caret_on: true,
            last_cursor: Point::default(),
            view_size: (0, 0),
            min_w: 0,
            audio: None,
        };
        ed.strip.layout(gfx, &VSLOTS, Point::new(0, 0), ed.bar_h(), 0, scale);
        ed.min_w = ed.strip.rect.width() + (16.0 * scale) as i32;
        unsafe {
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let m = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(m, &mut mi);
            let work = super::util::to_rect(mi.rcWork);
            let w = (work.width() * 85 / 100).max(ed.min_w).min(work.width());
            let h = (work.height() * 85 / 100).min(work.height());
            let _ = SetWindowPos(hwnd, None, work.left + (work.width() - w) / 2, work.top + (work.height() - h) / 2, w, h, SWP_NOZORDER);
            let _ = ShowWindow(hwnd, SW_SHOW);
            super::overlay::force_foreground(hwnd);
            SetTimer(Some(hwnd), TICK_TIMER, 200, None);
        }
        ed.on_size(gfx);
        Ok(ed)
    }

    pub fn style(&self) -> (Tool, Color, usize) {
        (self.canvas.tool, self.canvas.color, self.canvas.width_idx)
    }

    fn bar_h(&self) -> i32 {
        (52.0 * self.scale).round() as i32
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn client(&self) -> (f32, f32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        ((r.right - r.left) as f32, (r.bottom - r.top) as f32)
    }

    pub fn set_status(&mut self, s: String) {
        self.status = s;
        unsafe {
            SetTimer(Some(self.hwnd), STATUS_TIMER, 4000, None);
        }
        self.invalidate();
    }

    fn lay(&self) -> Lay {
        let s = self.scale;
        let (cw, ch) = self.client();
        let bar = self.bar_h() as f32;
        let panel_h = 186.0 * s;
        let top = (ch - panel_h).max(bar + 60.0 * s);
        let pad = 16.0 * s;
        let (aw, ah) = ((cw - pad * 2.0).max(20.0), (top - bar - pad * 2.0).max(20.0));
        let k = (aw / self.src_w as f32).min(ah / self.src_h as f32);
        let (pw, ph) = (self.src_w as f32 * k, self.src_h as f32 * k);
        let pic = rect_f(((cw - pw) / 2.0).round(), (bar + pad + (ah - ph) / 2.0).round(), ((cw + pw) / 2.0).round(), (bar + pad + (ah + ph) / 2.0).round());
        let x0 = pad;
        let x1 = cw - pad;
        let tr = rect_f(0.0, top, cw, top + 44.0 * s);
        let bh = 30.0 * s;
        let by = tr.top + (tr.bottom - tr.top - bh) / 2.0;
        let play = rect_f(x0, by, x0 + bh, by + bh);
        let delete = rect_f(x1 - 86.0 * s, by, x1, by + bh);
        let split = rect_f(delete.left - 8.0 * s - 86.0 * s, by, delete.left - 8.0 * s, by + bh);
        let ruler = rect_f(x0, tr.bottom, x1, tr.bottom + 18.0 * s);
        let clips = rect_f(x0, ruler.bottom + 2.0 * s, x1, ruler.bottom + 2.0 * s + 64.0 * s);
        let items = rect_f(x0, clips.bottom + 8.0 * s, x1, clips.bottom + 8.0 * s + 40.0 * s);
        Lay { bar, pic, transport: tr, play, split, delete, ruler, clips, items, x0, x1 }
    }

    fn total(&self) -> u32 {
        self.timeline.total().max(1)
    }

    fn x_of(&self, l: &Lay, t: u32) -> f32 {
        l.x0 + (l.x1 - l.x0) * t as f32 / self.total() as f32
    }

    fn t_of(&self, l: &Lay, x: f32) -> u32 {
        let k = ((x - l.x0) / (l.x1 - l.x0)).clamp(0.0, 1.0);
        ((k * self.total() as f32) as u32).min(self.total().saturating_sub(1))
    }

    fn ms_per_px(&self, l: &Lay) -> f32 {
        self.total() as f32 / (l.x1 - l.x0).max(1.0)
    }

    /// Zoom tool or a selected zoom: show the whole frame to place zooms.
    fn show_full(&self) -> bool {
        self.canvas.tool == Tool::Zoom || self.canvas.selected.and_then(|i| self.canvas.items.get(i)).is_some_and(|it| matches!(it.shape, Shape::Zoom { .. }))
    }

    /// Items as the preview should show them right now (with the draft and the
    /// text being typed).
    fn render_items(&self) -> Vec<Item> {
        let c = &self.canvas;
        let mut v = c.items.clone();
        if let Some(d) = &c.draft {
            v.push(d.clone());
        }
        if let Some(t) = &c.text {
            v.push(Item { shape: Shape::Text { at: t.at, text: t.text.clone(), size: t.size }, color: t.color, width: 0.0, span: t.span });
        }
        if self.show_full() {
            v.retain(|it| !matches!(it.shape, Shape::Zoom { .. }));
        }
        v
    }

    fn cam(&self) -> Cam {
        let (fw, fh) = (self.src_w as f32, self.src_h as f32);
        if self.show_full() { Cam::full(fw, fh) } else { vedit::camera(&self.canvas.items, self.playhead, fw, fh) }
    }

    fn to_src(&self, l: &Lay, x: f32, y: f32) -> P {
        let c = self.cam();
        let p = l.pic;
        P::new(c.x + (x - p.left) / (p.right - p.left) * c.w, c.y + (y - p.top) / (p.bottom - p.top) * c.h)
    }

    fn to_screen(&self, l: &Lay, q: P) -> (f32, f32) {
        let c = self.cam();
        let p = l.pic;
        (p.left + (q.x - c.x) / c.w * (p.right - p.left), p.top + (q.y - c.y) / c.h * (p.bottom - p.top))
    }

    fn push_view(&mut self) {
        let (ow, oh) = self.view_size;
        if ow < 2 || oh < 2 {
            return;
        }
        self.player.send(Cmd::View(View { timeline: self.timeline.clone(), items: self.render_items(), ow, oh }));
    }

    fn seek(&mut self, t: u32) {
        self.playhead = t.min(self.total().saturating_sub(1));
        self.canvas.set_time(self.playhead);
        self.player.send(Cmd::Show(self.playhead));
        self.invalidate();
    }

    fn pause(&mut self) {
        if self.player.playing() {
            self.player.send(Cmd::Pause);
        }
        self.audio = None;
    }

    fn toggle_play(&mut self) {
        if self.player.playing() {
            self.player.send(Cmd::Pause);
            self.audio = None;
        } else {
            let _ = self.canvas.finish_text();
            let from = if self.playhead + 50 >= self.total() { 0 } else { self.playhead };
            self.audio = Some(AudioPlay::start(self.src.clone(), self.timeline.clone(), from));
            self.player.send(Cmd::Play(from));
            self.player.shared.playing.store(true, Ordering::Release);
        }
        self.invalidate();
    }

    fn snapshot(&self) -> Snap {
        Snap { timeline: self.timeline.clone(), items: self.canvas.items.clone() }
    }

    /// Records a finished change for undo and updates the preview.
    fn commit(&mut self) {
        let now = self.snapshot();
        if now != self.last {
            self.undo.push(std::mem::replace(&mut self.last, now));
            self.redo.clear();
            self.edits += 1;
        }
        if let Some(i) = self.sel_clip {
            if i >= self.timeline.clips.len() {
                self.sel_clip = None;
            }
        }
        self.push_view();
        let t = self.playhead.min(self.total().saturating_sub(1));
        self.seek(t);
    }

    fn restore(&mut self, s: Snap) {
        self.timeline = s.timeline.clone();
        self.canvas.set_items(s.items.clone());
        self.last = s;
        self.sel_clip = None;
        self.edits += 1;
        self.push_view();
        let t = self.playhead;
        self.seek(t);
    }

    fn do_undo(&mut self) {
        let _ = self.canvas.finish_text();
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.snapshot());
            self.restore(s);
        }
    }

    fn do_redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.snapshot());
            self.restore(s);
        }
    }

    fn apply(&mut self, c: Change) {
        match c {
            Change::None => {}
            Change::Redraw => {
                self.push_view();
                self.invalidate();
            }
            Change::Committed => self.commit(),
        }
    }

    fn split(&mut self) {
        if self.timeline.split(self.playhead) {
            self.set_status("Split".into());
            self.commit();
        }
    }

    fn delete(&mut self) {
        if self.canvas.selected.is_some() {
            let c = self.canvas.key(Key::Delete);
            self.apply(c);
            return;
        }
        if let Some(i) = self.sel_clip {
            if self.timeline.delete(i) {
                self.sel_clip = None;
                self.commit();
            } else {
                self.set_status("The last clip stays".into());
            }
        }
    }

    pub fn set_color(&mut self, col: Color) {
        let c = self.canvas.set_color(col);
        self.apply(c);
        self.invalidate();
    }

    fn state(&self) -> StripState {
        let c = &self.canvas;
        StripState { tool: c.tool, color: c.color, width_idx: c.width_idx, blur_mode: c.blur_mode, can_undo: !self.undo.is_empty(), can_redo: !self.redo.is_empty(), custom: self.custom }
    }

    fn button(&mut self, b: Btn) -> Option<Event> {
        match b {
            Btn::Tool(t) => {
                let c = self.canvas.set_tool(t);
                self.apply(c);
                self.push_view();
            }
            Btn::Color(i) => self.set_color(PALETTE[i]),
            Btn::Custom => return Some(Event::CustomColor),
            Btn::Width(i) => {
                let c = self.canvas.set_width(i);
                self.apply(c);
            }
            Btn::BlurMode => {
                let m = if self.canvas.blur_mode == BlurMode::Blur { BlurMode::Pixelate } else { BlurMode::Blur };
                let c = self.canvas.set_blur_mode(m);
                self.apply(c);
            }
            Btn::Undo => self.do_undo(),
            Btn::Redo => self.do_redo(),
            Btn::Export => self.start_export(false),
            Btn::Gif => self.start_export(true),
            _ => {}
        }
        self.invalidate();
        None
    }

    // ---- export ----

    fn start_export(&mut self, gif: bool) {
        if self.work.is_some() {
            return;
        }
        self.pause();
        let _ = self.canvas.finish_text();
        if gif {
            if let Some((v, p)) = &self.last_export {
                if *v == self.edits && p.exists() {
                    let p = p.clone();
                    self.start_gif(p);
                    return;
                }
            }
        }
        let out = vedit::edited_path(&self.src, |p| p.exists());
        let prog = Arc::new(Progress::default());
        let job = Job { src: self.src.clone(), out, timeline: self.timeline.clone(), items: self.canvas.items.clone(), fonts: font::shared() };
        let pr = prog.clone();
        let handle = std::thread::spawn(move || vexport::run(job, pr));
        self.work = Some(Work { prog, handle, then_gif: gif, gif: false });
        self.status = "Exporting 0%".into();
        self.invalidate();
    }

    fn start_gif(&mut self, video: PathBuf) {
        let prog = Arc::new(Progress::default());
        let pr = prog.clone();
        let est = (self.total() as u64 * self.fps.0 as u64 / (self.fps.1.max(1) as u64 * 1000)).max(1);
        let handle = std::thread::spawn(move || make_gif(video, est, pr));
        self.work = Some(Work { prog, handle, then_gif: false, gif: true });
        self.status = "Making GIF 0%".into();
        self.invalidate();
    }

    /// Polls running work; returns an event when an export lands.
    fn tick_work(&mut self) -> Option<Event> {
        let done = self.work.as_ref().is_some_and(|w| w.handle.is_finished());
        if !done {
            if let Some(w) = &self.work {
                let pct = w.prog.permille.load(Ordering::Acquire) / 10;
                self.status = format!("{} {pct}%", if w.gif { "Making GIF" } else { "Exporting" });
                self.invalidate();
            }
            return None;
        }
        let w = self.work.take()?;
        let result = w.handle.join().unwrap_or_else(|_| Err("crashed".into()));
        let name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match result {
            Ok(p) if w.gif => {
                self.set_status(format!("GIF saved: {}", name(&p)));
                None
            }
            Ok(p) => {
                self.last_export = Some((self.edits, p.clone()));
                if w.then_gif {
                    self.start_gif(p.clone());
                    return Some(Event::Exported(p));
                }
                let ok = super::clipboard::put_file(self.hwnd, &p);
                self.set_status(format!("Saved {}{}", name(&p), if ok { ", copied to the clipboard" } else { "" }));
                Some(Event::Exported(p))
            }
            Err(e) => {
                log(&format!("editor work failed: {e}"));
                self.set_status(format!("{} failed: {e}", if w.gif { "GIF" } else { "Export" }));
                None
            }
        }
    }

    // ---- input ----

    fn on_size(&mut self, gfx: &Gfx) {
        let (w, h) = self.client();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let (w, h) = (w as u32, h as u32);
        match &self.rt {
            Some(rt) if self.rt_size != (w, h) => unsafe {
                let _ = rt.Resize(&D2D_SIZE_U { width: w, height: h });
                self.rt_size = (w, h);
            },
            None => {
                self.rt = gfx.target(self.hwnd, w, h).ok();
                self.rt_size = (w, h);
                self.frame_bmp = None;
                self.thumb_bmps.clear();
            }
            _ => {}
        }
        let bar = self.bar_h();
        self.strip.layout(gfx, &VSLOTS, Point::new(0, 0), bar, w as i32, self.scale);
        let l = self.lay();
        let vs = (((l.pic.right - l.pic.left) as u32).min(self.src_w).max(2), ((l.pic.bottom - l.pic.top) as u32).min(self.src_h).max(2));
        if vs != self.view_size {
            self.view_size = vs;
            self.push_view();
        }
        self.invalidate();
    }

    /// Which annotation bar (index into canvas.items) and part is under (x, y)
    /// in the items track.
    fn bar_at(&self, l: &Lay, x: f32, y: f32) -> Option<(usize, SpanMode)> {
        if !inside(&l.items, x, y) {
            return None;
        }
        let lanes = self.lanes();
        let n = lanes.iter().map(|(_, ln)| ln + 1).max().unwrap_or(1).max(1);
        let lh = (l.items.bottom - l.items.top) / n as f32;
        for (i, lane) in lanes.iter().rev() {
            let Some((a, b)) = self.canvas.items[*i].span else { continue };
            let (bx0, bx1) = (self.x_of(l, a), self.x_of(l, b.min(self.total())));
            let (by0, by1) = (l.items.top + lh * *lane as f32, l.items.top + lh * (*lane + 1) as f32);
            if y >= by0 && y < by1 && x >= bx0 - EDGE_PX && x <= bx1 + EDGE_PX {
                let e = EDGE_PX * self.scale;
                let mode = if (x - bx0).abs() <= e && bx1 - bx0 > e * 2.0 {
                    SpanMode::Start
                } else if (x - bx1).abs() <= e && bx1 - bx0 > e * 2.0 {
                    SpanMode::End
                } else if x >= bx0 && x <= bx1 {
                    SpanMode::Move
                } else {
                    continue;
                };
                return Some((*i, mode));
            }
        }
        None
    }

    /// Lane for every timed item, so overlapping bars stack (max 3 lanes).
    fn lanes(&self) -> Vec<(usize, usize)> {
        let mut idx: Vec<usize> = (0..self.canvas.items.len()).filter(|&i| self.canvas.items[i].span.is_some()).collect();
        idx.sort_by_key(|&i| self.canvas.items[i].span.unwrap().0);
        let mut ends: Vec<u32> = Vec::new();
        let mut out = Vec::new();
        for i in idx {
            let (a, b) = self.canvas.items[i].span.unwrap();
            let lane = match ends.iter().position(|&e| e <= a) {
                Some(k) => k,
                None if ends.len() < 3 => {
                    ends.push(0);
                    ends.len() - 1
                }
                None => ends.len() - 1,
            };
            ends[lane] = b;
            out.push((i, lane));
        }
        out
    }

    fn clip_at(&self, l: &Lay, x: f32) -> Option<(usize, Option<bool>)> {
        let e = EDGE_PX * self.scale;
        for i in 0..self.timeline.clips.len() {
            let a = self.x_of(l, self.timeline.start_of(i));
            let b = self.x_of(l, self.timeline.start_of(i) + self.timeline.clips[i].len());
            if x >= a - 1.0 && x <= b + 1.0 {
                let edge = if (x - a).abs() <= e && b - a > e * 3.0 {
                    Some(false)
                } else if (x - b).abs() <= e && b - a > e * 3.0 {
                    Some(true)
                } else {
                    None
                };
                return Some((i, edge));
            }
        }
        None
    }

    fn insert_index(&self, l: &Lay, idx: usize, x: f32) -> usize {
        let mut n = 0;
        for i in 0..self.timeline.clips.len() {
            if i == idx {
                continue;
            }
            let mid = self.x_of(l, self.timeline.start_of(i) + self.timeline.clips[i].len() / 2);
            if mid < x {
                n += 1;
            }
        }
        n
    }

    pub fn handle(&mut self, gfx: &Gfx, msg: u32, wp: WPARAM, lp: LPARAM) -> (Option<LRESULT>, Option<Event>) {
        let shift_key = || unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        match msg {
            WM_SIZE => {
                self.on_size(gfx);
                (Some(LRESULT(0)), None)
            }
            WM_DPICHANGED => {
                self.scale = dpi_scale(self.hwnd);
                let r = unsafe { &*(lp.0 as *const RECT) };
                unsafe {
                    let _ = SetWindowPos(self.hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
                }
                self.fonts_ui = None;
                self.on_size(gfx);
                (Some(LRESULT(0)), None)
            }
            WM_GETMINMAXINFO => {
                let mmi = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
                let mut r = RECT { left: 0, top: 0, right: self.min_w.max((640.0 * self.scale) as i32), bottom: (480.0 * self.scale) as i32 };
                unsafe {
                    let _ = AdjustWindowRectEx(&mut r, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0));
                }
                mmi.ptMinTrackSize = POINT { x: r.right - r.left, y: r.bottom - r.top };
                (Some(LRESULT(0)), None)
            }
            WM_PAINT => {
                self.render(gfx);
                unsafe {
                    let _ = ValidateRect(Some(self.hwnd), None);
                }
                (Some(LRESULT(0)), None)
            }
            WM_ERASEBKGND => (Some(LRESULT(1)), None),
            m if m == WM_VFRAME => {
                if self.player.playing() {
                    self.playhead = self.player.shared.t.load(Ordering::Acquire);
                    self.canvas.set_time(self.playhead);
                } else {
                    self.audio = None;
                }
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            m if m == WM_VSTRIPS => {
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            WM_TIMER => {
                let mut ev = None;
                if wp.0 == TICK_TIMER {
                    ev = self.tick_work();
                    self.caret_on = !self.caret_on;
                    if self.canvas.text.is_some() {
                        self.invalidate();
                    }
                } else if wp.0 == STATUS_TIMER {
                    unsafe {
                        let _ = KillTimer(Some(self.hwnd), STATUS_TIMER);
                    }
                    if self.work.is_none() {
                        self.status.clear();
                    }
                    self.invalidate();
                }
                (Some(LRESULT(0)), ev)
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                    self.set_cursor();
                    return (Some(LRESULT(1)), None);
                }
                (None, None)
            }
            WM_MOUSEMOVE => {
                let p = lparam_point(lp.0);
                self.last_cursor = p;
                let (x, y) = (p.x as f32, p.y as f32);
                let hov = if p.y < self.bar_h() { self.strip.hit(p) } else { None };
                if hov != self.strip.hover {
                    self.strip.hover = hov;
                    self.invalidate();
                }
                let l = self.lay();
                match self.drag {
                    Drag::None => {}
                    Drag::Canvas => {
                        let q = self.to_src(&l, x, y);
                        let c = self.canvas.mouse_move(q, wp.0 & 0x0004 != 0);
                        self.apply(c);
                    }
                    Drag::Scrub => {
                        let t = self.t_of(&l, x);
                        self.seek(t);
                    }
                    Drag::Clip { idx, x0, moved, .. } => {
                        let moved = moved || (x - x0).abs() > 5.0 * self.scale;
                        if moved {
                            let t = self.insert_index(&l, idx, x);
                            self.drag = Drag::Clip { idx, x0, moved, target: t };
                            self.invalidate();
                        }
                    }
                    Drag::Trim { idx, right, x0, orig, ms_px } => {
                        let d = ((x - x0) * ms_px) as i64;
                        let (a, b) = if right { (orig.a as i64, orig.b as i64 + d) } else { (orig.a as i64 + d, orig.b as i64) };
                        let (a, b) = (a.max(0) as u32, b.max(0) as u32);
                        let (a, b) = if right { (a, b.max(a + MIN_CLIP_MS)) } else { (a.min(b.saturating_sub(MIN_CLIP_MS)), b) };
                        if self.timeline.trim(idx, a, b) {
                            self.push_view();
                            let t = if right { self.timeline.start_of(idx) + self.timeline.clips[idx].len() - 1 } else { self.timeline.start_of(idx) };
                            self.seek(t);
                        }
                    }
                    Drag::Span { idx, mode, x0, orig: (oa, ob), ms_px } => {
                        let d = ((x - x0) * ms_px) as i64;
                        let span = match mode {
                            SpanMode::Move => {
                                let a = (oa as i64 + d).max(0) as u32;
                                (a, a + (ob - oa))
                            }
                            SpanMode::Start => ((oa as i64 + d).clamp(0, ob as i64 - 100) as u32, ob),
                            SpanMode::End => (oa, (ob as i64 + d).max(oa as i64 + 100) as u32),
                        };
                        if let Some(it) = self.canvas.items.get_mut(idx) {
                            it.span = Some(span);
                        }
                        self.canvas.version += 1;
                        let t = if mode == SpanMode::End { span.1.saturating_sub(1) } else { span.0 };
                        self.push_view();
                        self.seek(t);
                    }
                }
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONDOWN => {
                let p = lparam_point(lp.0);
                let (x, y) = (p.x as f32, p.y as f32);
                unsafe {
                    SetCapture(self.hwnd);
                    let _ = SetFocus(Some(self.hwnd));
                }
                if p.y < self.bar_h() {
                    self.strip.press = self.strip.hit(p);
                    self.invalidate();
                    return (Some(LRESULT(0)), None);
                }
                let l = self.lay();
                if inside(&l.play, x, y) {
                    self.toggle_play();
                    return (Some(LRESULT(0)), None);
                }
                if inside(&l.split, x, y) {
                    self.pause();
                    self.split();
                    return (Some(LRESULT(0)), None);
                }
                if inside(&l.delete, x, y) {
                    self.pause();
                    self.delete();
                    return (Some(LRESULT(0)), None);
                }
                self.pause();
                if inside(&l.ruler, x, y) {
                    self.drag = Drag::Scrub;
                    let t = self.t_of(&l, x);
                    self.seek(t);
                    return (Some(LRESULT(0)), None);
                }
                if inside(&l.clips, x, y) {
                    let _ = self.canvas.finish_text();
                    self.canvas.selected = None;
                    if let Some((i, edge)) = self.clip_at(&l, x) {
                        self.sel_clip = Some(i);
                        match edge {
                            Some(right) => {
                                self.drag = Drag::Trim { idx: i, right, x0: x, orig: self.timeline.clips[i], ms_px: self.ms_per_px(&l) };
                            }
                            None => {
                                self.drag = Drag::Clip { idx: i, x0: x, moved: false, target: i };
                                let t = self.t_of(&l, x);
                                self.seek(t);
                            }
                        }
                    }
                    self.invalidate();
                    return (Some(LRESULT(0)), None);
                }
                if inside(&l.items, x, y) {
                    let _ = self.canvas.finish_text();
                    match self.bar_at(&l, x, y) {
                        Some((i, mode)) => {
                            let span = self.canvas.items[i].span.unwrap_or((0, 0));
                            self.sel_clip = None;
                            if self.playhead < span.0 || self.playhead >= span.1 {
                                self.seek(span.0);
                            }
                            self.canvas.selected = Some(i);
                            self.canvas.version += 1;
                            self.drag = Drag::Span { idx: i, mode, x0: x, orig: span, ms_px: self.ms_per_px(&l) };
                            self.push_view();
                        }
                        None => {
                            self.drag = Drag::Scrub;
                            let t = self.t_of(&l, x);
                            self.seek(t);
                        }
                    }
                    self.invalidate();
                    return (Some(LRESULT(0)), None);
                }
                if y < l.transport.top {
                    self.sel_clip = None;
                    self.drag = Drag::Canvas;
                    let q = self.to_src(&l, x, y);
                    let c = self.canvas.mouse_down(q, wp.0 & 0x0004 != 0);
                    self.caret_on = true;
                    self.apply(c);
                    self.push_view();
                }
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONUP => {
                let p = lparam_point(lp.0);
                let (x, y) = (p.x as f32, p.y as f32);
                unsafe {
                    let _ = ReleaseCapture();
                }
                if let Some(pressed) = self.strip.press.take() {
                    self.invalidate();
                    if self.strip.hit(p) == Some(pressed) {
                        return (Some(LRESULT(0)), self.button(pressed));
                    }
                    return (Some(LRESULT(0)), None);
                }
                let l = self.lay();
                match std::mem::replace(&mut self.drag, Drag::None) {
                    Drag::Canvas => {
                        let q = self.to_src(&l, x, y);
                        let c = self.canvas.mouse_up(q, wp.0 & 0x0004 != 0);
                        self.apply(c);
                        self.push_view();
                    }
                    Drag::Clip { idx, moved: true, target, .. } => {
                        if self.timeline.move_clip(idx, target) {
                            self.sel_clip = Some(target);
                            let t = self.timeline.start_of(target);
                            self.playhead = t;
                            self.commit();
                        }
                        self.invalidate();
                    }
                    Drag::Trim { .. } | Drag::Span { .. } => self.commit(),
                    _ => self.invalidate(),
                }
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONDBLCLK => {
                let p = lparam_point(lp.0);
                let l = self.lay();
                if (p.y as f32) < l.transport.top && p.y >= self.bar_h() {
                    let q = self.to_src(&l, p.x as f32, p.y as f32);
                    let c = self.canvas.double_click(q);
                    self.apply(c);
                    self.push_view();
                }
                (Some(LRESULT(0)), None)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
                let typing = self.canvas.text.is_some();
                let step = (1000 * self.fps.1 / self.fps.0.max(1)).max(1);
                match vk {
                    VK_Z if ctrl => self.do_undo(),
                    VK_Y if ctrl => self.do_redo(),
                    VK_E if ctrl => self.start_export(false),
                    VK_ESCAPE => {
                        let c = self.canvas.key(Key::Escape);
                        if c == Change::None {
                            self.sel_clip = None;
                            self.invalidate();
                        }
                        self.apply(c);
                        self.push_view();
                    }
                    VK_RETURN if typing => {
                        let c = self.canvas.key(Key::Enter);
                        self.apply(c);
                    }
                    VK_BACK if typing => {
                        let c = self.canvas.key(Key::Backspace);
                        self.apply(c);
                    }
                    _ if typing => {}
                    VK_SPACE => self.toggle_play(),
                    VK_DELETE | VK_BACK => {
                        self.pause();
                        self.delete();
                    }
                    VK_S if !ctrl => {
                        self.pause();
                        self.split();
                    }
                    VK_LEFT => {
                        self.pause();
                        let t = self.playhead.saturating_sub(if shift_key() { 1000 } else { step });
                        self.seek(t);
                    }
                    VK_RIGHT => {
                        self.pause();
                        let t = self.playhead + if shift_key() { 1000 } else { step };
                        self.seek(t);
                    }
                    VK_HOME => {
                        self.pause();
                        self.seek(0);
                    }
                    VK_END => {
                        self.pause();
                        let t = self.total();
                        self.seek(t);
                    }
                    _ if !ctrl => {
                        let ch = char::from_u32(vk.0 as u32).unwrap_or('\0');
                        let tool = if ch == 'Z' { Some(Tool::Zoom) } else { Tool::from_key(ch).filter(|t| *t != Tool::Crop) };
                        if let Some(t) = tool {
                            self.pause();
                            let c = self.canvas.set_tool(t);
                            self.apply(c);
                            self.push_view();
                        }
                    }
                    _ => {}
                }
                self.caret_on = true;
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            WM_CHAR => {
                if self.canvas.text.is_some() {
                    if let Some(ch) = char::from_u32(wp.0 as u32) {
                        let c = self.canvas.char(ch);
                        self.caret_on = true;
                        self.apply(c);
                    }
                }
                (Some(LRESULT(0)), None)
            }
            _ => (None, None),
        }
    }

    fn set_cursor(&self) {
        let p = self.last_cursor;
        let (x, y) = (p.x as f32, p.y as f32);
        let l = self.lay();
        let id = if p.y < self.bar_h() {
            if self.strip.hit(p).is_some() { IDC_HAND } else { IDC_ARROW }
        } else if inside(&l.play, x, y) || inside(&l.split, x, y) || inside(&l.delete, x, y) {
            IDC_HAND
        } else if inside(&l.clips, x, y) {
            match self.clip_at(&l, x) {
                Some((_, Some(_))) => IDC_SIZEWE,
                Some(_) => IDC_SIZEALL,
                None => IDC_ARROW,
            }
        } else if inside(&l.items, x, y) {
            match self.bar_at(&l, x, y) {
                Some((_, SpanMode::Move)) => IDC_SIZEALL,
                Some(_) => IDC_SIZEWE,
                None => IDC_ARROW,
            }
        } else if y >= l.transport.top {
            IDC_ARROW
        } else {
            match self.canvas.tool {
                Tool::Select => {
                    if self.canvas.wants(self.to_src(&l, x, y)) { IDC_SIZEALL } else { IDC_ARROW }
                }
                Tool::Text => IDC_IBEAM,
                _ => IDC_CROSS,
            }
        };
        unsafe {
            if let Ok(c) = LoadCursorW(None, id) {
                SetCursor(Some(c));
            }
        }
    }

    // ---- drawing ----

    fn render(&mut self, gfx: &Gfx) {
        let Some(rt) = self.rt.clone() else { return };
        let rt1: &ID2D1RenderTarget = &rt;
        let s = self.scale;
        if self.fonts_ui.as_ref().map(|f| f.0) != Some(s) {
            if let (Ok(small), Ok(label), Ok(icon)) = (gfx.ui_font(11.0 * s, false), gfx.ui_font(13.0 * s, false), gfx.icon_font(14.0 * s)) {
                self.fonts_ui = Some((s, small, label, icon));
            }
        }
        // New preview frame?
        let seq = self.player.shared.seq.load(Ordering::Acquire);
        if self.frame_bmp.as_ref().map(|b| b.0) != Some(seq) {
            let f = self.player.shared.frame.lock().ok().and_then(|g| g.as_ref().map(|(_, f)| f.clone()));
            if let Some(f) = f {
                if let Ok(b) = d2d::bitmap(rt1, f.width() as u32, f.height() as u32, &f.pixels) {
                    self.frame_bmp = Some((seq, b));
                }
            }
        }
        let l = self.lay();
        let (cw, _ch) = (self.rt_size.0 as f32, self.rt_size.1 as f32);
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&rgba(BG, 1.0)));
            if let Some((_, b)) = &self.frame_bmp {
                if let Ok(sh) = d2d::brush(rt1, rgba(0x000000, 0.35)) {
                    rt.FillRectangle(&rect_f(l.pic.left + 3.0, l.pic.top + 4.0, l.pic.right + 3.0, l.pic.bottom + 4.0), &sh);
                }
                rt.DrawBitmap(b, Some(&l.pic), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            }
            self.draw_marks_ui(rt1, &l);
            // Toolbar.
            if let Ok(bar_bg) = d2d::brush(rt1, rgba(PANEL, 1.0)) {
                rt.FillRectangle(&rect_f(0.0, 0.0, cw, l.bar), &bar_bg);
                rt.FillRectangle(&rect_f(0.0, l.transport.top, cw, self.rt_size.1 as f32), &bar_bg);
            }
            if let Ok(line) = d2d::brush(rt1, rgba(0xFFFFFF, 0.08)) {
                rt.FillRectangle(&rect_f(0.0, l.bar - 1.0, cw, l.bar), &line);
                rt.FillRectangle(&rect_f(0.0, l.transport.top, cw, l.transport.top + 1.0), &line);
            }
            let st = self.state();
            self.strip.draw(rt1, gfx, &st, (0.0, 0.0), false);
            self.draw_panel(rt1, gfx, &l);
            let _ = rt.EndDraw(None, None);
        }
    }

    unsafe fn draw_panel(&mut self, rt: &ID2D1RenderTarget, gfx: &Gfx, l: &Lay) {
        let s = self.scale;
        let Some((_, small, label, icon)) = self.fonts_ui.clone() else { return };
        let (Ok(fg), Ok(dim), Ok(acc), Ok(btn), Ok(clip_bg)) = (
            d2d::brush(rt, rgba(0xFFFFFF, 0.9)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.45)),
            d2d::brush(rt, rgba(ACCENT, 1.0)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.08)),
            d2d::brush(rt, rgba(0x2A2B30, 1.0)),
        ) else {
            return;
        };
        unsafe {
            // Transport: play, time, split, delete, status.
            rt.FillRoundedRectangle(&d2d::round_rect(l.play, 6.0 * s), &btn);
            let playing = self.player.playing();
            d2d::draw_text(rt, &icon, if playing { "\u{E769}" } else { "\u{E768}" }, l.play, &fg);
            let time = format!("{} / {}", vedit::clock(self.playhead), vedit::clock(self.timeline.total()));
            d2d::draw_text(rt, &label, &time, rect_f(l.play.right + 12.0 * s, l.play.top + 5.0 * s, l.play.right + 220.0 * s, l.play.bottom), &fg);
            for (r, text) in [(l.split, "\u{E8C6}  Split  S"), (l.delete, "\u{E74D}  Delete")] {
                rt.FillRoundedRectangle(&d2d::round_rect(r, 6.0 * s), &btn);
                let w = gfx.measure(&label, text);
                d2d::draw_text(rt, &label, text, rect_f(r.left + ((r.right - r.left) - w) / 2.0, r.top + 5.0 * s, r.right, r.bottom), &fg);
            }
            if !self.status.is_empty() {
                let w = gfx.measure(&label, &self.status);
                let x = l.split.left - 16.0 * s - w;
                d2d::draw_text(rt, &label, &self.status, rect_f(x, l.play.top + 5.0 * s, l.split.left - 8.0 * s, l.play.bottom), &acc);
            }

            // Ruler.
            let total = self.timeline.total().max(1);
            let px_s = (l.x1 - l.x0) / (total as f32 / 1000.0);
            let step_s = [1u32, 2, 5, 10, 15, 30, 60, 120, 300, 600].into_iter().find(|st| *st as f32 * px_s >= 60.0 * s).unwrap_or(600);
            let mut sec = 0u32;
            while sec * 1000 <= total {
                let x = self.x_of(l, sec * 1000);
                rt.FillRectangle(&rect_f(x, l.ruler.bottom - 5.0 * s, x + 1.0, l.ruler.bottom), &dim);
                d2d::draw_text(rt, &small, &format!("{}:{:02}", sec / 60, sec % 60), rect_f(x + 3.0 * s, l.ruler.top, x + 60.0 * s, l.ruler.bottom), &dim);
                sec += step_s;
            }

            // Clips: thumbnails, waveform, borders.
            let thumbs: Vec<(u32, Arc<crate::image::Frame>)> = self.strips.thumbs.lock().map(|g| g.clone()).unwrap_or_default();
            let peaks: Vec<u8> = self.strips.peaks.lock().map(|g| g.clone()).unwrap_or_default();
            let track_h = l.clips.bottom - l.clips.top;
            let wave_h = 16.0 * s;
            let th = track_h - wave_h;
            let tw = if let Some((_, f)) = thumbs.first() { th * f.width() as f32 / f.height().max(1) as f32 } else { th * 16.0 / 9.0 };
            let moving = match &self.drag {
                Drag::Clip { idx, moved: true, target, .. } => Some((*idx, *target)),
                _ => None,
            };
            for (i, c) in self.timeline.clips.clone().iter().enumerate() {
                let (a, b) = (self.x_of(l, self.timeline.start_of(i)), self.x_of(l, self.timeline.start_of(i) + c.len()));
                let r = rect_f(a + 1.0, l.clips.top, (b - 1.0).max(a + 2.0), l.clips.bottom);
                rt.FillRoundedRectangle(&d2d::round_rect(r, 5.0 * s), &clip_bg);
                rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
                if !thumbs.is_empty() {
                    let mut x = r.left;
                    while x < r.right {
                        let k = ((x + tw / 2.0 - a) / (b - a).max(1.0)).clamp(0.0, 1.0);
                        let src_ms = c.a + (k * c.len() as f32) as u32;
                        let j = match thumbs.binary_search_by_key(&src_ms, |(m, _)| *m) {
                            Ok(j) => j,
                            Err(j) => {
                                if j == 0 {
                                    0
                                } else if j >= thumbs.len() {
                                    thumbs.len() - 1
                                } else if src_ms - thumbs[j - 1].0 < thumbs[j].0 - src_ms {
                                    j - 1
                                } else {
                                    j
                                }
                            }
                        };
                        let (ms, f) = &thumbs[j];
                        if !self.thumb_bmps.contains_key(ms) {
                            if let Ok(bm) = d2d::bitmap(rt, f.width() as u32, f.height() as u32, &f.pixels) {
                                self.thumb_bmps.insert(*ms, bm);
                            }
                        }
                        if let Some(bm) = self.thumb_bmps.get(ms) {
                            rt.DrawBitmap(bm, Some(&rect_f(x, r.top, x + tw, r.top + th)), 0.9, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                        }
                        x += tw;
                    }
                }
                if !peaks.is_empty() {
                    if let Ok(wb) = d2d::brush(rt, rgba(0x5EEAD4, 0.8)) {
                        let mid = r.bottom - wave_h / 2.0;
                        let mut x = r.left;
                        while x < r.right {
                            let k0 = ((x - a) / (b - a).max(1.0)).clamp(0.0, 1.0);
                            let k1 = ((x + 2.0 - a) / (b - a).max(1.0)).clamp(0.0, 1.0);
                            let (m0, m1) = (c.a + (k0 * c.len() as f32) as u32, c.a + (k1 * c.len() as f32) as u32);
                            let (p0, p1) = ((m0 / PEAK_MS) as usize, ((m1 / PEAK_MS) as usize).max(m0 as usize / PEAK_MS as usize + 1));
                            let v = peaks.get(p0..p1.min(peaks.len())).and_then(|sl| sl.iter().max().copied()).unwrap_or(0);
                            let hh = (v as f32 / 255.0) * (wave_h / 2.0 - 1.0);
                            rt.FillRectangle(&rect_f(x, mid - hh, x + 1.2, mid + hh.max(0.5)), &wb);
                            x += 2.0;
                        }
                    }
                }
                rt.PopAxisAlignedClip();
                let sel = self.sel_clip == Some(i);
                let border = if sel { &acc } else { &btn };
                rt.DrawRoundedRectangle(&d2d::round_rect(r, 5.0 * s), border, if sel { 2.0 * s } else { 1.0 }, None);
                if moving.is_some_and(|(m, _)| m == i) {
                    if let Ok(veil) = d2d::brush(rt, rgba(0x000000, 0.5)) {
                        rt.FillRoundedRectangle(&d2d::round_rect(r, 5.0 * s), &veil);
                    }
                }
            }
            if let Some((idx, target)) = moving {
                // Where the clip will land.
                let others: Vec<usize> = (0..self.timeline.clips.len()).filter(|&k| k != idx).collect();
                let x = if target == 0 {
                    l.x0
                } else {
                    let k = others[(target - 1).min(others.len() - 1)];
                    self.x_of(l, self.timeline.start_of(k) + self.timeline.clips[k].len())
                };
                rt.FillRectangle(&rect_f(x - 1.5 * s, l.clips.top - 4.0 * s, x + 1.5 * s, l.clips.bottom + 4.0 * s), &acc);
            }

            // Annotation track.
            rt.FillRoundedRectangle(&d2d::round_rect(l.items, 5.0 * s), &btn);
            let lanes = self.lanes();
            if lanes.is_empty() {
                d2d::draw_text(rt, &small, "Annotations and zooms show up here. Draw on the video to add one.", rect_f(l.items.left + 10.0 * s, l.items.top + 12.0 * s, l.items.right, l.items.bottom), &dim);
            }
            let n = lanes.iter().map(|(_, ln)| ln + 1).max().unwrap_or(1).max(1);
            let lh = (l.items.bottom - l.items.top) / n as f32;
            for (i, lane) in lanes {
                let it = &self.canvas.items[i];
                let Some((a, b)) = it.span else { continue };
                let (x0, x1) = (self.x_of(l, a), self.x_of(l, b.min(total)).max(self.x_of(l, a) + 4.0));
                let r = rect_f(x0, l.items.top + lh * lane as f32 + 2.0, x1, l.items.top + lh * (lane + 1) as f32 - 2.0);
                let (col, name) = match &it.shape {
                    Shape::Zoom { .. } => (ZOOM_COLOR, "Zoom".to_string()),
                    Shape::Text { text, .. } => (it.color.hex(), format!("T  {}", text.lines().next().unwrap_or(""))),
                    Shape::Step { .. } => (it.color.hex(), "Step".into()),
                    Shape::Arrow { .. } => (it.color.hex(), "Arrow".into()),
                    Shape::Blur { .. } => (0x9CA3AF, "Blur".into()),
                    Shape::Rect { .. } => (it.color.hex(), "Rectangle".into()),
                    Shape::Ellipse { .. } => (it.color.hex(), "Ellipse".into()),
                    Shape::Line { .. } => (it.color.hex(), "Line".into()),
                    Shape::Pen { .. } => (it.color.hex(), "Pen".into()),
                    Shape::Highlighter { .. } => (it.color.hex(), "Marker".into()),
                };
                if let Ok(bb) = d2d::brush(rt, rgba(col, 0.85)) {
                    rt.FillRoundedRectangle(&d2d::round_rect(r, 4.0 * s), &bb);
                }
                if self.canvas.selected == Some(i) {
                    if let Ok(wb) = d2d::brush(rt, rgba(0xFFFFFF, 1.0)) {
                        rt.DrawRoundedRectangle(&d2d::round_rect(r, 4.0 * s), &wb, 2.0 * s, None);
                    }
                }
                let ink = if Color::from_hex(col).luma() > 0.6 { 0x111111 } else { 0xFFFFFF };
                if let Ok(tb) = d2d::brush(rt, rgba(ink, 0.95)) {
                    d2d::draw_text(rt, &small, &name, rect_f(r.left + 6.0 * s, r.top + ((r.bottom - r.top) - 14.0 * s) / 2.0, r.right - 2.0, r.bottom), &tb);
                }
            }

            // Playhead.
            let x = self.x_of(l, self.playhead);
            rt.FillRectangle(&rect_f(x - 1.0, l.ruler.top, x + 1.0, l.items.bottom), &acc);
            rt.FillRoundedRectangle(&d2d::round_rect(rect_f(x - 5.0 * s, l.ruler.top, x + 5.0 * s, l.ruler.top + 10.0 * s), 2.0 * s), &acc);
        }
    }

    /// Selection handles, text box and caret, zoom areas.
    unsafe fn draw_marks_ui(&self, rt: &ID2D1RenderTarget, l: &Lay) {
        let s = self.scale;
        let (Ok(acc), Ok(white), Ok(zb)) = (d2d::brush(rt, rgba(ACCENT, 1.0)), d2d::brush(rt, rgba(0xFFFFFF, 1.0)), d2d::brush(rt, rgba(ZOOM_COLOR, 1.0))) else { return };
        let c = &self.canvas;
        unsafe {
            if self.show_full() {
                let (fw, fh) = (self.src_w as f32, self.src_h as f32);
                for (i, it) in c.items.iter().enumerate() {
                    if let (Shape::Zoom { a, b }, true) = (&it.shape, it.visible_at(c.time)) {
                        let z = vedit::zoom_rect(*a, *b, fw, fh);
                        let (x0, y0) = self.to_screen(l, P::new(z.x, z.y));
                        let (x1, y1) = self.to_screen(l, P::new(z.x + z.w, z.y + z.h));
                        let w = if c.selected == Some(i) { 2.5 * s } else { 1.5 * s };
                        rt.DrawRectangle(&rect_f(x0, y0, x1, y1), &zb, w, None);
                    }
                }
                if let Some(d) = &c.draft {
                    if let Shape::Zoom { a, b } = &d.shape {
                        let z = vedit::zoom_rect(*a, *b, fw, fh);
                        let (x0, y0) = self.to_screen(l, P::new(z.x, z.y));
                        let (x1, y1) = self.to_screen(l, P::new(z.x + z.w, z.y + z.h));
                        rt.DrawRectangle(&rect_f(x0, y0, x1, y1), &zb, 2.0 * s, None);
                    }
                }
            }
            if let Some(i) = c.selected {
                if let Some(it) = c.items.get(i).filter(|it| it.visible_at(c.time)) {
                    if !matches!(it.shape, Shape::Zoom { .. }) {
                        let (l0, t0, r0, b0) = bbox(it, c.fonts());
                        let (x0, y0) = self.to_screen(l, P::new(l0, t0));
                        let (x1, y1) = self.to_screen(l, P::new(r0, b0));
                        rt.DrawRectangle(&rect_f(x0 - 2.0, y0 - 2.0, x1 + 2.0, y1 + 2.0), &acc, 1.0, None);
                    }
                    for (_, hp) in c.handles() {
                        let (x, y) = self.to_screen(l, hp);
                        let hs = 4.5 * s;
                        let r = rect_f(x - hs, y - hs, x + hs, y + hs);
                        rt.FillRectangle(&r, &white);
                        rt.DrawRectangle(&r, &acc, 1.0, None);
                    }
                }
            }
            if let Some(t) = &c.text {
                if let Some(f) = c.fonts() {
                    let (l0, tp, r0, b0) = font::text_box(&f.text, t.size, &t.text, t.at.x, t.at.y);
                    let (x0, y0) = self.to_screen(l, P::new(l0, tp));
                    let (x1, y1) = self.to_screen(l, P::new(r0, b0));
                    rt.DrawRectangle(&rect_f(x0 - 4.0, y0 - 3.0, x1 + 4.0, y1 + 3.0), &acc, 1.0, None);
                    if self.caret_on {
                        let last = t.text.split('\n').last().unwrap_or("");
                        let lines = t.text.split('\n').count().max(1) as f32;
                        let lh = font::line_height(&f.text, t.size);
                        let cx = t.at.x + font::line_width(&f.text, t.size, last);
                        let cy = t.at.y + lh * (lines - 1.0);
                        let (sx, sy) = self.to_screen(l, P::new(cx, cy));
                        let (_, ey) = self.to_screen(l, P::new(cx, cy + lh));
                        rt.FillRectangle(&rect_f(sx + 1.0, sy + 2.0, sx + 1.0 + 1.5 * s, ey - 2.0), &acc);
                    }
                }
            }
        }
    }
}

/// GIF of an exported video, next to it.
fn make_gif(src: PathBuf, est_frames: u64, prog: Arc<Progress>) -> Result<PathBuf, String> {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let out = src.with_extension("gif");
    let r = (|| -> Result<(), String> {
        let mut r = Reader::open(&src)?;
        let file = std::io::BufWriter::new(std::fs::File::create(&out).map_err(|e| e.to_string())?);
        let mut g = crate::gifenc::GifWriter::new(file, r.w, r.h)?;
        let mut n = 0u64;
        while let Some((t, f)) = r.next() {
            g.push(t, &f)?;
            n += 1;
            prog.permille.store(((n * 1000) / est_frames.max(1)).min(999) as u32, Ordering::Release);
        }
        if n == 0 {
            return Err("no frames".into());
        }
        g.finish(333_333)?;
        Ok(())
    })();
    if let Err(e) = r {
        let _ = std::fs::remove_file(&out);
        return Err(e);
    }
    Ok(out)
}

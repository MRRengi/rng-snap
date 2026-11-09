//! Video editor window: the preview (annotations are drawn and edited right
//! on it), a transport row, and the timeline: clips that can be split,
//! deleted, trimmed and dragged into a new order, plus a track with every
//! annotation and zoom as a bar that can be moved and stretched in time.

use super::class_name;
use super::d2d::{self, Gfx, rect_f, rgba};
use super::mp4read::Reader;
use super::theme;
use windows::core::Interface;
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
use crate::vedit::{self, Cam, Clip, MIN_CLIP_MS, TimeView, Timeline};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// A new preview frame is ready.
pub const WM_VFRAME: u32 = WM_APP + 40;
/// More thumbnails / waveform arrived.
pub const WM_VSTRIPS: u32 = WM_APP + 41;
const TICK_TIMER: usize = 1;
const STATUS_TIMER: usize = 2;
const ANIM_TIMER: usize = 3;
/// A tooltip is due.
const TIP_TIMER: usize = 4;
/// Trim handles and the selected clip (Final Cut's yellow).
const TRIM: u32 = 0xFFCC00;
/// Pixels around a clip or bar edge that grab the edge instead of the body.
const EDGE_PX: f32 = 7.0;

pub enum Event {
    CustomColor,
    /// A finished export (already on the clipboard).
    Exported(PathBuf),
}

pub const VSLOTS: [Slot; 30] = [
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
    Slot::B(Btn::WidthCycle),
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
    /// The zoom slider.
    Zoom,
    /// The scrollbar thumb under the tracks.
    Scroll { x0: f32, start0: f32 },
}

/// What a tooltip is about: an id (so it restarts when the target changes),
/// the text and the shortcut.
#[derive(Clone, PartialEq)]
struct Tip {
    id: u32,
    text: String,
    key: &'static str,
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
    /// The glass sheet holding transport and timeline.
    sheet: D2D_RECT_F,
    /// Where the video picture is drawn.
    pic: D2D_RECT_F,
    transport: D2D_RECT_F,
    play: D2D_RECT_F,
    split: D2D_RECT_F,
    delete: D2D_RECT_F,
    /// Zoom controls: out, the slider, in, show all.
    zout: D2D_RECT_F,
    slider: D2D_RECT_F,
    zin: D2D_RECT_F,
    zfit: D2D_RECT_F,
    ruler: D2D_RECT_F,
    /// Track names on the left.
    head: D2D_RECT_F,
    clips: D2D_RECT_F,
    audio: D2D_RECT_F,
    items: D2D_RECT_F,
    scroll: D2D_RECT_F,
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
    hover_ctl: Option<u8>,
    /// Visible part of the timeline.
    tv: TimeView,
    /// Time under the cursor on the timeline (hover line and frame preview).
    hover_t: Option<u32>,
    hover_clip: Option<usize>,
    tip: Option<Tip>,
    tip_since: std::time::Instant,
    status_at: std::time::Instant,
    tracking: bool,
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
        let title = super::util::wide(&format!("{} - Snap video editor", src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
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
        theme::apply_window(hwnd);
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
        // 135 px tall: sharp on the timeline and good enough as an instant
        // preview while scrubbing.
        make_strips(src.clone(), duration_ms, 135, strips.clone(), hwnd, WM_VSTRIPS);
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
            hover_ctl: None,
            tv: TimeView::default(),
            hover_t: None,
            hover_clip: None,
            tip: None,
            tip_since: std::time::Instant::now(),
            status_at: std::time::Instant::now(),
            tracking: false,
        };
        ed.strip.animate = true;
        ed.strip.layout(gfx, &VSLOTS, Point::new(0, 0), ed.bar_h(), 0, scale);
        ed.min_w = (ed.strip.rect.width() as f32 * 0.75) as i32 + (20.0 * scale) as i32;
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

    /// Height of the floating toolbar capsule.
    fn bar_h(&self) -> i32 {
        (48.0 * self.scale).round() as i32
    }

    fn margin(&self) -> f32 {
        10.0 * self.scale
    }

    fn in_bar(&self, p: Point) -> bool {
        self.strip.rect.contains(p)
    }

    pub fn theme_changed(&mut self) {
        theme::apply_window(self.hwnd);
        self.invalidate();
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
        self.status_at = std::time::Instant::now();
        unsafe {
            SetTimer(Some(self.hwnd), STATUS_TIMER, 3000, None);
        }
        self.invalidate();
    }

    fn lay(&self) -> Lay {
        let s = self.scale;
        let (cw, ch) = self.client();
        let m = self.margin();
        let bar = self.bar_h() as f32 + m * 2.0;
        let panel_h = 252.0 * s;
        let sheet_top = (ch - panel_h - m).max(bar + 60.0 * s);
        let sheet = rect_f(m, sheet_top, cw - m, ch - m);
        let top = sheet.top;
        let pad = 16.0 * s;
        let (aw, ah) = ((cw - pad * 2.0).max(20.0), (top - bar - pad).max(20.0));
        let k = (aw / self.src_w as f32).min(ah / self.src_h as f32);
        let (pw, ph) = (self.src_w as f32 * k, self.src_h as f32 * k);
        let pic = rect_f(((cw - pw) / 2.0).round(), (bar + (ah - ph) / 2.0).round(), ((cw + pw) / 2.0).round(), (bar + (ah + ph) / 2.0).round());
        let left = sheet.left + 14.0 * s;
        let head_w = 78.0 * s;
        let x0 = left + head_w;
        let x1 = sheet.right - 16.0 * s;
        let tr = rect_f(sheet.left, top + 6.0 * s, sheet.right, top + 52.0 * s);
        let bh = 34.0 * s;
        let by = tr.top + (tr.bottom - tr.top - bh) / 2.0;
        let play = rect_f(left, by, left + bh, by + bh);
        let delete = rect_f(x1 - 92.0 * s, by, x1, by + bh);
        let split = rect_f(delete.left - 6.0 * s - 96.0 * s, by, delete.left - 6.0 * s, by + bh);
        // Zoom: [-] ----o---- [+] [fit], left of Split.
        let zb = 26.0 * s;
        let zy = by + (bh - zb) / 2.0;
        let zfit = rect_f(split.left - 14.0 * s - zb, zy, split.left - 14.0 * s, zy + zb);
        let zin = rect_f(zfit.left - 6.0 * s - zb, zy, zfit.left - 6.0 * s, zy + zb);
        let slider = rect_f(zin.left - 4.0 * s - 96.0 * s, zy, zin.left - 4.0 * s, zy + zb);
        let zout = rect_f(slider.left - 4.0 * s - zb, zy, slider.left - 4.0 * s, zy + zb);
        let ruler = rect_f(x0, tr.bottom, x1, tr.bottom + 20.0 * s);
        let clips = rect_f(x0, ruler.bottom + 2.0 * s, x1, ruler.bottom + 2.0 * s + 58.0 * s);
        let audio = rect_f(x0, clips.bottom + 4.0 * s, x1, clips.bottom + 4.0 * s + 30.0 * s);
        let items = rect_f(x0, audio.bottom + 8.0 * s, x1, audio.bottom + 8.0 * s + 44.0 * s);
        let scroll = rect_f(x0, items.bottom + 8.0 * s, x1, items.bottom + 14.0 * s);
        let head = rect_f(left, clips.top, x0 - 8.0 * s, items.bottom);
        let tr = rect_f(tr.left, top, tr.right, tr.bottom);
        Lay { bar, sheet, pic, transport: tr, play, split, delete, zout, slider, zin, zfit, ruler, head, clips, audio, items, scroll, x0, x1 }
    }

    fn total(&self) -> u32 {
        self.timeline.total().max(1)
    }

    fn x_of(&self, l: &Lay, t: u32) -> f32 {
        l.x0 + (t as f32 - self.tv.start) / self.ms_per_px(l)
    }

    fn t_of(&self, l: &Lay, x: f32) -> u32 {
        let t = self.tv.start + (x.clamp(l.x0, l.x1) - l.x0) * self.ms_per_px(l);
        (t.max(0.0) as u32).min(self.total().saturating_sub(1))
    }

    fn ms_per_px(&self, l: &Lay) -> f32 {
        self.tv.visible(self.total()) / (l.x1 - l.x0).max(1.0)
    }

    /// Zoom by `factor` around time `t` (kept where it is on screen).
    fn zoom_by(&mut self, factor: f32, t: u32) {
        let l = self.lay();
        let frac = ((self.x_of(&l, t) - l.x0) / (l.x1 - l.x0).max(1.0)).clamp(0.0, 1.0);
        let total = self.total();
        self.tv.zoom_at(factor, t as f32, frac, total);
        self.invalidate();
    }

    /// Slider position 0..1 for the zoom (log scale).
    fn zoom_frac(&self) -> f32 {
        let max = TimeView::max_zoom(self.total());
        if max <= 1.0 { 0.0 } else { (self.tv.zoom.ln() / max.ln()).clamp(0.0, 1.0) }
    }

    fn set_zoom_frac(&mut self, k: f32) {
        let max = TimeView::max_zoom(self.total());
        let z = (k.clamp(0.0, 1.0) * max.ln()).exp();
        let factor = z / self.tv.zoom;
        let t = self.playhead;
        self.zoom_by(factor, t);
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
            Btn::WidthCycle => {
                let c = self.canvas.set_width((self.canvas.width_idx + 1) % 3);
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
        let mi = self.margin().round() as i32;
        self.strip.layout_fit(gfx, &VSLOTS, mi, mi, self.bar_h(), w as i32 - mi * 2, self.scale);
        let mut o = POINT::default();
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut o);
        }
        self.strip.publish("veditor", |r| r.offset(o.x, o.y));
        let l = self.lay();
        let pr = |r: D2D_RECT_F| Rect::new(r.left as i32 + o.x, r.top as i32 + o.y, r.right as i32 + o.x, r.bottom as i32 + o.y);
        super::uimap::put("veditor.play", pr(l.play));
        super::uimap::put("veditor.split", pr(l.split));
        super::uimap::put("veditor.delete", pr(l.delete));
        super::uimap::put("veditor.ruler", pr(l.ruler));
        super::uimap::put("veditor.clips", pr(l.clips));
        super::uimap::put("veditor.audio", pr(l.audio));
        super::uimap::put("veditor.items", pr(l.items));
        super::uimap::put("veditor.zoomout", pr(l.zout));
        super::uimap::put("veditor.zoomslider", pr(l.slider));
        super::uimap::put("veditor.zoomin", pr(l.zin));
        super::uimap::put("veditor.zoomfit", pr(l.zfit));
        super::uimap::put("veditor.heads", pr(l.head));
        super::uimap::put("veditor.pic", pr(l.pic));
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
                    let total = self.total();
                    self.tv.follow(self.playhead as f32, total);
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
                } else if wp.0 == ANIM_TIMER {
                    if !self.strip.step() {
                        unsafe {
                            let _ = KillTimer(Some(self.hwnd), ANIM_TIMER);
                        }
                    }
                    self.invalidate();
                } else if wp.0 == TIP_TIMER {
                    unsafe {
                        let _ = KillTimer(Some(self.hwnd), TIP_TIMER);
                    }
                    self.invalidate();
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
                let hov = if self.in_bar(p) { self.strip.hit(p) } else { None };
                let l = self.lay();
                let (fx, fy) = (p.x as f32, p.y as f32);
                let ctl = if inside(&l.play, fx, fy) {
                    Some(1)
                } else if inside(&l.split, fx, fy) {
                    Some(2)
                } else if inside(&l.delete, fx, fy) {
                    Some(3)
                } else if inside(&l.zout, fx, fy) {
                    Some(4)
                } else if inside(&l.zin, fx, fy) {
                    Some(5)
                } else if inside(&l.zfit, fx, fy) {
                    Some(6)
                } else if inside(&l.scroll, fx, fy) {
                    Some(7)
                } else {
                    None
                };
                if !self.tracking {
                    let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                    unsafe {
                        let _ = TrackMouseEvent(&mut t);
                    }
                    self.tracking = true;
                }
                // Hover line + frame preview over the timeline.
                let over_tl = matches!(self.drag, Drag::None) && fx >= l.x0 && fx <= l.x1 && fy >= l.ruler.top && fy < l.items.bottom;
                let ht = over_tl.then(|| self.t_of(&l, fx));
                if ht != self.hover_t {
                    self.hover_t = ht;
                    self.invalidate();
                }
                let hc = if inside(&l.clips, fx, fy) || inside(&l.audio, fx, fy) { self.clip_at(&l, fx).map(|c| c.0) } else { None };
                if hc != self.hover_clip {
                    self.hover_clip = hc;
                    self.invalidate();
                }
                let tip = self.tip_at(p);
                if tip.as_ref().map(|t| t.id) != self.tip.as_ref().map(|t| t.id) {
                    let was = self.tip.is_some() && self.tip_since.elapsed().as_millis() >= 480;
                    self.tip = tip;
                    // Moving between controls keeps tips showing right away.
                    self.tip_since = if was && self.tip.is_some() { std::time::Instant::now() - std::time::Duration::from_millis(480) } else { std::time::Instant::now() };
                    unsafe {
                        SetTimer(Some(self.hwnd), TIP_TIMER, 500, None);
                    }
                    self.invalidate();
                } else if self.tip.is_some() && self.tip_since.elapsed().as_millis() >= 480 {
                    self.invalidate();
                }
                if ctl != self.hover_ctl {
                    self.hover_ctl = ctl;
                    self.invalidate();
                }
                if hov != self.strip.hover {
                    if self.strip.set_hover(hov) {
                        unsafe {
                            SetTimer(Some(self.hwnd), ANIM_TIMER, 15, None);
                        }
                    }
                    self.invalidate();
                }
                let l = self.lay();
                match self.drag {
                    Drag::None => {}
                    Drag::Zoom => {
                        let k = (x - l.slider.left) / (l.slider.right - l.slider.left).max(1.0);
                        self.set_zoom_frac(k);
                    }
                    Drag::Scroll { x0, start0 } => {
                        let per_px = self.total() as f32 / (l.scroll.right - l.scroll.left).max(1.0);
                        self.tv.start = start0 + (x - x0) * per_px;
                        let total = self.total();
                        self.tv.clamp(total);
                        self.invalidate();
                    }
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
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.hover_t = None;
                self.hover_clip = None;
                self.tip = None;
                self.hover_ctl = None;
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                let mut pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
                unsafe {
                    let _ = ScreenToClient(self.hwnd, &mut pt);
                }
                let l = self.lay();
                let (x, y) = (pt.x as f32, pt.y as f32);
                if y < l.sheet.top {
                    return (None, None);
                }
                let delta = ((wp.0 >> 16) & 0xFFFF) as i16 as f32 / 120.0;
                let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
                let total = self.total();
                if ctrl && msg == WM_MOUSEWHEEL {
                    let t = self.t_of(&l, x);
                    self.zoom_by(1.25f32.powf(delta), t);
                } else {
                    let sign = if msg == WM_MOUSEHWHEEL { 1.0 } else { -1.0 };
                    self.tv.start += sign * delta * self.tv.visible(total) * 0.12;
                    self.tv.clamp(total);
                    self.invalidate();
                }
                self.hover_t = (x >= l.x0 && x <= l.x1 && y >= l.ruler.top && y < l.items.bottom).then(|| self.t_of(&l, x));
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONDOWN => {
                let p = lparam_point(lp.0);
                let (x, y) = (p.x as f32, p.y as f32);
                unsafe {
                    SetCapture(self.hwnd);
                    let _ = SetFocus(Some(self.hwnd));
                }
                if self.in_bar(p) {
                    self.strip.press = self.strip.hit(p);
                    self.invalidate();
                    return (Some(LRESULT(0)), None);
                }
                let l = self.lay();
                self.tip = None;
                if inside(&l.play, x, y) {
                    self.toggle_play();
                    return (Some(LRESULT(0)), None);
                }
                if inside(&l.zout, x, y) || inside(&l.zin, x, y) {
                    let f = if inside(&l.zin, x, y) { 1.6 } else { 1.0 / 1.6 };
                    let t = self.playhead;
                    self.zoom_by(f, t);
                    return (Some(LRESULT(0)), None);
                }
                if inside(&l.zfit, x, y) {
                    self.tv = TimeView::default();
                    self.invalidate();
                    return (Some(LRESULT(0)), None);
                }
                if inside(&rect_f(l.slider.left - 8.0 * self.scale, l.slider.top, l.slider.right + 8.0 * self.scale, l.slider.bottom), x, y) {
                    self.drag = Drag::Zoom;
                    let k = (x - l.slider.left) / (l.slider.right - l.slider.left).max(1.0);
                    self.set_zoom_frac(k);
                    return (Some(LRESULT(0)), None);
                }
                if inside(&l.scroll, x, y) && self.tv.zoom > 1.001 {
                    let total = self.total() as f32;
                    let w = (l.scroll.right - l.scroll.left) / self.tv.zoom;
                    let tx = l.scroll.left + (l.scroll.right - l.scroll.left) * (self.tv.start / total);
                    if x < tx || x > tx + w {
                        // Click beside the thumb: jump there.
                        self.tv.start = (x - l.scroll.left) / (l.scroll.right - l.scroll.left) * total - self.tv.visible(self.total()) / 2.0;
                        let t = self.total();
                        self.tv.clamp(t);
                    }
                    self.drag = Drag::Scroll { x0: x, start0: self.tv.start };
                    self.invalidate();
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
                if inside(&l.clips, x, y) || inside(&l.audio, x, y) {
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
                if (p.y as f32) < l.transport.top && !self.in_bar(p) {
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
        let id = if self.in_bar(p) {
            if self.strip.hit(p).is_some() { IDC_HAND } else { IDC_ARROW }
        } else if inside(&l.play, x, y) || inside(&l.split, x, y) || inside(&l.delete, x, y) || inside(&l.zout, x, y) || inside(&l.zin, x, y) || inside(&l.zfit, x, y) || inside(&l.slider, x, y) || (inside(&l.scroll, x, y) && self.tv.zoom > 1.001) {
            IDC_HAND
        } else if inside(&l.clips, x, y) || inside(&l.audio, x, y) {
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

    /// The cached D2D bitmap of the strip thumbnail nearest `src_ms`.
    fn thumb_near(&mut self, rt: &ID2D1RenderTarget, thumbs: &[(u32, Arc<crate::image::Frame>)], src_ms: u32) -> Option<ID2D1Bitmap> {
        if thumbs.is_empty() {
            return None;
        }
        let j = match thumbs.binary_search_by_key(&src_ms, |(m, _)| *m) {
            Ok(j) => j,
            Err(0) => 0,
            Err(j) if j >= thumbs.len() => thumbs.len() - 1,
            Err(j) => {
                if src_ms - thumbs[j - 1].0 < thumbs[j].0 - src_ms {
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
        self.thumb_bmps.get(ms).cloned()
    }

    fn render(&mut self, gfx: &Gfx) {
        let Some(rt) = self.rt.clone() else { return };
        let rt1: &ID2D1RenderTarget = &rt;
        let s = self.scale;
        let p = theme::pal();
        if self.fonts_ui.as_ref().map(|f| f.0) != Some(s) {
            if let (Ok(small), Ok(label), Ok(icon)) = (gfx.ui_font(11.0 * s, false), gfx.ui_font(13.0 * s, true), gfx.icon_font(14.0 * s)) {
                self.fonts_ui = Some((s, small, label, icon));
            }
        }
        // New preview frame?
        let seq = self.player.shared.seq.load(Ordering::Acquire);
        let mut frame_t = None;
        if self.frame_bmp.as_ref().map(|b| b.0) != Some(seq) {
            let f = self.player.shared.frame.lock().ok().and_then(|g| g.as_ref().map(|(t, f)| (*t, f.clone())));
            if let Some((_, f)) = f {
                if let Ok(b) = d2d::bitmap(rt1, f.width() as u32, f.height() as u32, &f.pixels) {
                    self.frame_bmp = Some((seq, b));
                }
            }
        }
        if let Ok(g) = self.player.shared.frame.lock() {
            frame_t = g.as_ref().map(|(t, _)| *t);
        }
        let l = self.lay();
        // While scrubbing the exact frame lags behind: show the nearest strip
        // frame at once (same picture when the camera shows the whole frame).
        let frame_ms = (1000 * self.fps.1 / self.fps.0.max(1)).max(1);
        let stale = !self.player.playing() && frame_t.is_some_and(|t| t.abs_diff(self.playhead) > frame_ms * 2);
        let quick = if stale && self.cam().is_full(self.src_w as f32, self.src_h as f32) {
            let thumbs: Vec<(u32, Arc<crate::image::Frame>)> = self.strips.thumbs.lock().map(|g| g.clone()).unwrap_or_default();
            let (_, src_ms) = self.timeline.locate(self.playhead);
            self.thumb_near(rt1, &thumbs, src_ms)
        } else {
            None
        };
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&theme::color(p.canvas)));
            theme::shadow(rt1, l.pic, 2.0 * s, s, 0.8);
            match (&quick, &self.frame_bmp) {
                (Some(b), _) => rt.DrawBitmap(b, Some(&l.pic), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
                (None, Some((_, b))) => rt.DrawBitmap(b, Some(&l.pic), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
                _ => {}
            }
            self.draw_marks_ui(rt1, &l);
            // The picture never sits under the glass, so there is nothing to
            // blur: no offscreen scene, no blur pass per video frame.
            theme::glass(rt1, gfx, l.sheet, 18.0 * s, s, None);
            let st = self.state();
            self.strip.draw(rt1, gfx, &st, (0.0, 0.0), Some(None));
            self.draw_panel(rt1, gfx, &l);
            self.draw_hover_preview(rt1, gfx, &l);
            self.draw_toast(rt1, gfx, &l);
            self.draw_tip(rt1, gfx);
            let _ = rt.EndDraw(None, None);
        }
        // Test hook: the visible time range (start ms, zoom x100).
        super::uimap::put("veditor.view", Rect::new(self.tv.start as i32, (self.tv.zoom * 100.0) as i32, 0, 0));
    }

    /// Fills `r` with rounded corners and runs `f` clipped to that shape.
    unsafe fn clipped(&self, rt: &ID2D1RenderTarget, r: D2D_RECT_F, radius: f32, f: impl FnOnce()) {
        let dc = rt.cast::<ID2D1DeviceContext>().ok();
        let geo = unsafe { rt.GetFactory().ok().and_then(|fa| fa.CreateRoundedRectangleGeometry(&theme::round(r, radius)).ok()) };
        match (&dc, geo) {
            (Some(dc), Some(geo)) => unsafe {
                let params = D2D1_LAYER_PARAMETERS1 {
                    contentBounds: r,
                    geometricMask: std::mem::ManuallyDrop::new(geo.cast::<ID2D1Geometry>().ok()),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: windows_numerics::Matrix3x2::identity(),
                    opacity: 1.0,
                    opacityBrush: std::mem::ManuallyDrop::new(None),
                    layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                };
                dc.PushLayer(&params, None);
                let _ = std::mem::ManuallyDrop::into_inner(params.geometricMask);
                f();
                dc.PopLayer();
            },
            _ => unsafe {
                rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
                f();
                rt.PopAxisAlignedClip();
            },
        }
    }

    unsafe fn draw_panel(&mut self, rt: &ID2D1RenderTarget, gfx: &Gfx, l: &Lay) {
        let s = self.scale;
        let p = theme::pal();
        let Some((_, small, label, icon)) = self.fonts_ui.clone() else { return };
        let (Some(fg), Some(fg2), Some(dim), Some(acc), Some(btn), Some(btn_hover), Some(track), Some(on_acc)) = (
            theme::brush(rt, p.label),
            theme::brush(rt, p.label2),
            theme::brush(rt, p.label3),
            theme::brush(rt, p.accent),
            theme::brush(rt, p.hover),
            theme::brush(rt, p.selected),
            theme::brush(rt, theme::with_alpha(p.hover, 1.4)),
            theme::brush(rt, p.on_accent),
        ) else {
            return;
        };
        let Ok(trim) = d2d::brush(rt, rgba(TRIM, 1.0)) else { return };
        let total = self.total();
        unsafe {
            // ---- Transport: play, time, zoom, Split, Delete ----
            let playing = self.player.playing();
            let pr = if self.hover_ctl == Some(1) { l.play } else { theme::inset(l.play, 0.5 * s) };
            let e = D2D1_ELLIPSE {
                point: windows_numerics::Vector2 { X: (pr.left + pr.right) / 2.0, Y: (pr.top + pr.bottom) / 2.0 },
                radiusX: (pr.right - pr.left) / 2.0,
                radiusY: (pr.bottom - pr.top) / 2.0,
            };
            rt.FillEllipse(&e, &acc);
            let glyph_r = if playing { l.play } else { rect_f(l.play.left + 1.5 * s, l.play.top, l.play.right + 1.5 * s, l.play.bottom) };
            d2d::draw_text(rt, &icon, if playing { "\u{E769}" } else { "\u{E768}" }, glyph_r, &on_acc);
            let now = vedit::clock(self.playhead);
            let tot = format!("  /  {}", vedit::clock(total));
            let tx = l.play.right + 14.0 * s;
            let ty = l.play.top + 7.0 * s;
            d2d::draw_text(rt, &label, &now, rect_f(tx, ty, tx + 120.0 * s, l.play.bottom), &fg);
            let nw = gfx.measure(&label, &now);
            d2d::draw_text(rt, &label, &tot, rect_f(tx + nw, ty, tx + nw + 140.0 * s, l.play.bottom), &fg2);
            // Zoom controls.
            for (k, r, glyph) in [(4u8, l.zout, "\u{E71F}"), (5, l.zin, "\u{E8A3}"), (6, l.zfit, "\u{E9A6}")] {
                if self.hover_ctl == Some(k) {
                    rt.FillRoundedRectangle(&theme::round(r, (r.bottom - r.top) / 2.0), &btn_hover);
                }
                d2d::draw_text(rt, &icon, glyph, rect_f(r.left + (r.right - r.left - 14.0 * s) / 2.0, r.top + (r.bottom - r.top - 18.0 * s) / 2.0, r.right, r.bottom), &fg2);
            }
            let sy = (l.slider.top + l.slider.bottom) / 2.0;
            rt.FillRoundedRectangle(&theme::round(rect_f(l.slider.left, sy - 2.0 * s, l.slider.right, sy + 2.0 * s), 2.0 * s), &track);
            let kx = l.slider.left + (l.slider.right - l.slider.left) * self.zoom_frac();
            rt.FillRoundedRectangle(&theme::round(rect_f(l.slider.left, sy - 2.0 * s, kx, sy + 2.0 * s), 2.0 * s), &acc);
            if let (Ok(w), Ok(sh)) = (d2d::brush(rt, rgba(0xFFFFFF, 1.0)), d2d::brush(rt, rgba(0x000000, 0.28))) {
                rt.FillEllipse(&D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: kx, Y: sy + 0.8 * s }, radiusX: 8.0 * s, radiusY: 8.0 * s }, &sh);
                rt.FillEllipse(&D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: kx, Y: sy }, radiusX: 7.5 * s, radiusY: 7.5 * s }, &w);
            }
            for (r, glyph, text, key) in [(l.split, "\u{E8C6}", "Split", "S"), (l.delete, "\u{E74D}", "Delete", "")] {
                let k = if r.left == l.split.left { 2 } else { 3 };
                let rr = theme::round(r, (r.bottom - r.top) / 2.0);
                rt.FillRoundedRectangle(&rr, if self.hover_ctl == Some(k) { &btn_hover } else { &btn });
                let gw = 22.0 * s;
                let kw = if key.is_empty() { 0.0 } else { gfx.measure(&small, key) + 12.0 * s };
                let w = gw + gfx.measure(&label, text) + kw;
                let x = r.left + ((r.right - r.left) - w) / 2.0;
                d2d::draw_text(rt, &icon, glyph, rect_f(x, r.top, x + gw - 6.0 * s, r.bottom), &fg);
                let lx = x + gw;
                d2d::draw_text(rt, &label, text, rect_f(lx, r.top + 8.0 * s, r.right, r.bottom), &fg);
                if !key.is_empty() {
                    let kx = lx + gfx.measure(&label, text) + 7.0 * s;
                    let kr = rect_f(kx, r.top + 9.0 * s, kx + gfx.measure(&small, key) + 8.0 * s, r.bottom - 9.0 * s);
                    rt.DrawRoundedRectangle(&theme::round(kr, 4.0 * s), &dim, 1.0, None);
                    d2d::draw_text(rt, &small, key, rect_f(kr.left + 4.0 * s, kr.top + 1.0 * s, kr.right, kr.bottom), &fg2);
                }
            }

            // ---- Track names ----
            for (r, glyph, name) in [(l.clips, "\u{E714}", "Video"), (l.audio, "\u{E767}", "Audio"), (l.items, "\u{E70F}", "Marks")] {
                let cy = (r.top + r.bottom) / 2.0;
                d2d::draw_text(rt, &icon, glyph, rect_f(l.head.left, cy - 9.0 * s, l.head.left + 20.0 * s, cy + 10.0 * s), &fg2);
                d2d::draw_text(rt, &small, name, rect_f(l.head.left + 22.0 * s, cy - 7.5 * s, l.head.right, cy + 9.0 * s), &fg2);
            }

            // Everything on the timeline is clipped to it (it scrolls).
            let tl = rect_f(l.x0 - 2.0 * s, l.ruler.top, l.x1 + 2.0 * s, l.items.bottom);
            rt.PushAxisAlignedClip(&tl, D2D1_ANTIALIAS_MODE_ALIASED);

            // ---- Ruler: labelled ticks and small ones between ----
            let mspp = self.ms_per_px(l);
            let (major, minor) = vedit::ruler_steps(mspp, 70.0 * s);
            let t0 = self.tv.start.max(0.0) as u32;
            let t1 = (self.tv.start + self.tv.visible(total)).min(total as f32) as u32;
            let mut t = t0 / minor * minor;
            while t <= t1 {
                let x = self.x_of(l, t);
                if t % major == 0 {
                    rt.FillRectangle(&rect_f(x, l.ruler.bottom - 7.0 * s, x + 1.0, l.ruler.bottom), &fg2);
                    d2d::draw_text(rt, &small, &vedit::ruler_label(t, major), rect_f(x + 3.0 * s, l.ruler.top + 1.0 * s, x + 80.0 * s, l.ruler.bottom), &fg2);
                } else {
                    rt.FillRectangle(&rect_f(x, l.ruler.bottom - 3.0 * s, x + 1.0, l.ruler.bottom), &dim);
                }
                t += minor;
            }

            // ---- Video clips: filmstrip, duration, trim handles ----
            let thumbs: Vec<(u32, Arc<crate::image::Frame>)> = self.strips.thumbs.lock().map(|g| g.clone()).unwrap_or_default();
            let peaks: Vec<u8> = self.strips.peaks.lock().map(|g| g.clone()).unwrap_or_default();
            let th = l.clips.bottom - l.clips.top;
            let tw = if let Some((_, f)) = thumbs.first() { th * f.width() as f32 / f.height().max(1) as f32 } else { th * 16.0 / 9.0 };
            let moving = match &self.drag {
                Drag::Clip { idx, moved: true, target, .. } => Some((*idx, *target)),
                _ => None,
            };
            let radius = 7.0 * s;
            let wave = theme::brush(rt, theme::with_alpha(p.teal, 0.95));
            let clips = self.timeline.clips.clone();
            for (i, c) in clips.iter().enumerate() {
                let (a, b) = (self.x_of(l, self.timeline.start_of(i)), self.x_of(l, self.timeline.start_of(i) + c.len()));
                if b < l.x0 - 20.0 || a > l.x1 + 20.0 {
                    continue;
                }
                let r = rect_f(a + 1.5, l.clips.top, (b - 1.5).max(a + 3.0), l.clips.bottom);
                rt.FillRoundedRectangle(&theme::round(r, radius), &track);
                let mut bmps: Vec<(D2D_RECT_F, ID2D1Bitmap)> = Vec::new();
                // Thumbnails on a fixed grid in time, so they don't swim when scrolling.
                let ms_per_thumb = (tw * mspp).max(1.0);
                let first = ((self.tv.start - self.timeline.start_of(i) as f32) / ms_per_thumb).floor().max(0.0) as u32;
                let mut k = first;
                loop {
                    let local = k as f32 * ms_per_thumb;
                    if local >= c.len() as f32 {
                        break;
                    }
                    let x = a + local / mspp;
                    if x > l.x1 + tw {
                        break;
                    }
                    let src_ms = c.a + (local + ms_per_thumb / 2.0).min(c.len() as f32 - 1.0) as u32;
                    if let Some(bm) = self.thumb_near(rt, &thumbs, src_ms) {
                        bmps.push((rect_f(x, r.top, x + tw, r.bottom), bm));
                    }
                    k += 1;
                }
                self.clipped(rt, r, radius, || {
                    for (d, bm) in &bmps {
                        rt.DrawBitmap(bm, Some(d), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                    }
                });
                // Duration pill.
                let dur = format!("{:.1}s", c.len() as f32 / 1000.0);
                let dw = gfx.measure(&small, &dur) + 10.0 * s;
                if r.right - r.left > dw + 16.0 * s {
                    let dr = rect_f(r.left.max(l.x0) + 5.0 * s, r.bottom - 19.0 * s, r.left.max(l.x0) + 5.0 * s + dw, r.bottom - 5.0 * s);
                    if let (Ok(bg), Ok(wt)) = (d2d::brush(rt, rgba(0x000000, 0.55)), d2d::brush(rt, rgba(0xFFFFFF, 0.95))) {
                        rt.FillRoundedRectangle(&theme::round(dr, 7.0 * s), &bg);
                        d2d::draw_text(rt, &small, &dur, rect_f(dr.left + 5.0 * s, dr.top, dr.right, dr.bottom), &wt);
                    }
                }
                let sel = self.sel_clip == Some(i);
                let hot = self.hover_clip == Some(i) || matches!(self.drag, Drag::Trim { idx, .. } if idx == i);
                if sel || hot {
                    // Final Cut style: a yellow frame with grips on both edges.
                    let w = if sel { 2.5 * s } else { 1.5 * s };
                    let col = if sel { &trim } else { &fg2 };
                    rt.DrawRoundedRectangle(&theme::round(theme::inset(r, w / 2.0), radius), col, w, None);
                    let hw = 9.0 * s;
                    for (hx, left) in [(r.left, true), (r.right - hw, false)] {
                        let hr = rect_f(hx, r.top, hx + hw, r.bottom);
                        let (rl, rr) = if left { (radius, 2.0 * s) } else { (2.0 * s, radius) };
                        let _ = (rl, rr);
                        rt.FillRoundedRectangle(&theme::round(hr, 3.0 * s), col);
                        if let Ok(g) = d2d::brush(rt, rgba(0x000000, 0.45)) {
                            let gx = hx + hw / 2.0;
                            let cy = (r.top + r.bottom) / 2.0;
                            rt.FillRoundedRectangle(&theme::round(rect_f(gx - 0.8 * s, cy - 8.0 * s, gx + 0.8 * s, cy + 8.0 * s), 0.8 * s), &g);
                        }
                    }
                } else if let Some(h) = theme::brush(rt, p.hairline) {
                    rt.DrawRoundedRectangle(&theme::round(theme::inset(r, 0.5), radius), &h, 1.0, None);
                }
                if moving.is_some_and(|(m, _)| m == i) {
                    if let Ok(veil) = d2d::brush(rt, rgba(0x000000, 0.45)) {
                        rt.FillRoundedRectangle(&theme::round(r, radius), &veil);
                    }
                }

                // ---- Audio under the clip: mirrored waveform ----
                let ar = rect_f(r.left, l.audio.top, r.right, l.audio.bottom);
                rt.FillRoundedRectangle(&theme::round(ar, 6.0 * s), &btn);
                if !peaks.is_empty() {
                    let mid = (ar.top + ar.bottom) / 2.0;
                    let half = (ar.bottom - ar.top) / 2.0 - 3.0 * s;
                    let step = 3.0 * s;
                    let mut bars: Vec<D2D_RECT_F> = Vec::new();
                    let mut x = ar.left.max(l.x0 - step) + 2.0 * s;
                    while x < ar.right.min(l.x1 + step) - 2.0 * s {
                        let m0 = c.a as f32 + (x - a) * mspp;
                        let m1 = m0 + step * mspp;
                        let (p0, p1) = ((m0.max(0.0) as u32 / PEAK_MS) as usize, ((m1.max(0.0) as u32 / PEAK_MS) as usize).max(m0.max(0.0) as usize / PEAK_MS as usize + 1));
                        let v = peaks.get(p0..p1.min(peaks.len())).and_then(|sl| sl.iter().max().copied()).unwrap_or(0);
                        let hh = (v as f32 / 255.0).sqrt() * half;
                        bars.push(rect_f(x, mid - hh.max(0.6), x + 1.8 * s, mid + hh.max(0.6)));
                        x += step;
                    }
                    if let Some(wb) = &wave {
                        self.clipped(rt, ar, 6.0 * s, || {
                            for q in &bars {
                                rt.FillRoundedRectangle(&theme::round(*q, 0.9 * s), wb);
                            }
                        });
                    }
                } else if self.strips.done.load(Ordering::Acquire) && i == 0 {
                    d2d::draw_text(rt, &small, "No sound in this video", rect_f(ar.left.max(l.x0) + 10.0 * s, ar.top + 7.0 * s, ar.right, ar.bottom), &fg2);
                }
            }
            if let Some((idx, target)) = moving {
                // Where the clip will land.
                let others: Vec<usize> = (0..self.timeline.clips.len()).filter(|&k| k != idx).collect();
                let x = if target == 0 {
                    self.x_of(l, 0)
                } else {
                    let k = others[(target - 1).min(others.len() - 1)];
                    self.x_of(l, self.timeline.start_of(k) + self.timeline.clips[k].len())
                };
                rt.FillRoundedRectangle(&theme::round(rect_f(x - 1.5 * s, l.clips.top - 4.0 * s, x + 1.5 * s, l.audio.bottom + 4.0 * s), 1.5 * s), &acc);
            }

            // ---- Marks: a bar per annotation, with its icon ----
            rt.FillRoundedRectangle(&theme::round(rect_f(l.x0, l.items.top, l.x1, l.items.bottom), radius), &btn);
            let lanes = self.lanes();
            if lanes.is_empty() {
                d2d::draw_text(rt, &small, "Draw on the video (arrows, text, blur, zoom) and each mark shows up here as a bar you can move and stretch in time.", rect_f(l.x0 + 12.0 * s, l.items.top + 14.0 * s, l.x1, l.items.bottom), &fg2);
            }
            let n = lanes.iter().map(|(_, ln)| ln + 1).max().unwrap_or(1).max(1);
            let lh = (l.items.bottom - l.items.top) / n as f32;
            for (i, lane) in lanes {
                let it = &self.canvas.items[i];
                let Some((a, b)) = it.span else { continue };
                let (x0, x1) = (self.x_of(l, a), self.x_of(l, b.min(total)).max(self.x_of(l, a) + 4.0));
                let r = rect_f(x0, l.items.top + lh * lane as f32 + 3.0 * s, x1, l.items.top + lh * (lane + 1) as f32 - 3.0 * s);
                let (col, glyph, name) = mark_look(it, &p);
                let cr = ((r.bottom - r.top) / 2.0).min(8.0 * s);
                if let Ok(bb) = d2d::brush(rt, rgba(col, 0.92)) {
                    rt.FillRoundedRectangle(&theme::round(r, cr), &bb);
                }
                if self.canvas.selected == Some(i) {
                    rt.DrawRoundedRectangle(&theme::round(theme::inset(r, -1.5 * s), cr + 1.5 * s), &fg, 2.0 * s, None);
                }
                let ink = if Color::from_hex(col).luma() > 0.6 { 0x111111 } else { 0xFFFFFF };
                if let Ok(tb) = d2d::brush(rt, rgba(ink, 0.95)) {
                    let cy = (r.top + r.bottom) / 2.0;
                    let tx = r.left.max(l.x0) + 8.0 * s;
                    if r.right - tx > 18.0 * s {
                        d2d::draw_text(rt, &icon, glyph, rect_f(tx, cy - 9.0 * s, tx + 18.0 * s, cy + 10.0 * s), &tb);
                    }
                    if r.right - tx > 44.0 * s {
                        d2d::draw_text(rt, &small, &name, rect_f(tx + 20.0 * s, cy - 7.5 * s, r.right - 6.0 * s, cy + 9.0 * s), &tb);
                    }
                    // Edge grips once it is wide enough to grab them.
                    if r.right - r.left > 30.0 * s {
                        if let Ok(g) = d2d::brush(rt, rgba(ink, 0.45)) {
                            for gx in [r.left + 3.5 * s, r.right - 4.5 * s] {
                                rt.FillRoundedRectangle(&theme::round(rect_f(gx, cy - 5.0 * s, gx + 1.2 * s, cy + 5.0 * s), 0.6 * s), &g);
                            }
                        }
                    }
                }
            }

            // ---- Hover line ----
            if let (Some(ht), Drag::None) = (self.hover_t, &self.drag) {
                let x = self.x_of(l, ht);
                if let Some(hb) = theme::brush(rt, theme::with_alpha(p.label, 0.45)) {
                    rt.FillRectangle(&rect_f(x - 0.5, l.ruler.top, x + 0.5, l.items.bottom), &hb);
                }
            }

            // ---- Playhead: a line through every track and a time bubble ----
            let x = self.x_of(l, self.playhead);
            rt.FillRoundedRectangle(&theme::round(rect_f(x - 1.0 * s, l.ruler.top + 4.0 * s, x + 1.0 * s, l.items.bottom), 1.0 * s), &acc);
            rt.PopAxisAlignedClip();
            if x >= l.x0 - 1.0 && x <= l.x1 + 1.0 {
                let tt = vedit::clock_fine(self.playhead);
                let bw = gfx.measure(&small, &tt) + 14.0 * s;
                let bx = (x - bw / 2.0).clamp(l.x0 - 30.0 * s, l.x1 - bw + 30.0 * s);
                let br = rect_f(bx, l.ruler.top - 2.0 * s, bx + bw, l.ruler.top + 15.0 * s);
                if let Ok(sh) = d2d::brush(rt, rgba(0x000000, 0.2)) {
                    rt.FillRoundedRectangle(&theme::round(rect_f(br.left, br.top + 1.0 * s, br.right, br.bottom + 1.0 * s), 8.5 * s), &sh);
                }
                rt.FillRoundedRectangle(&theme::round(br, 8.5 * s), &acc);
                d2d::draw_text(rt, &small, &tt, rect_f(br.left + 7.0 * s, br.top + 1.0 * s, br.right, br.bottom), &on_acc);
            }

            // ---- Scrollbar when zoomed in ----
            if self.tv.zoom > 1.001 {
                let sr = l.scroll;
                rt.FillRoundedRectangle(&theme::round(sr, 3.0 * s), &btn);
                let w = (sr.right - sr.left) / self.tv.zoom;
                let tx = sr.left + (sr.right - sr.left) * (self.tv.start / total as f32);
                let hot = matches!(self.drag, Drag::Scroll { .. }) || self.hover_ctl == Some(7);
                rt.FillRoundedRectangle(&theme::round(rect_f(tx, sr.top, (tx + w).max(tx + 16.0 * s), sr.bottom), 3.0 * s), if hot { &fg2 } else { &dim });
            }
        }
    }

    /// The frame under the cursor, in a card above the timeline.
    unsafe fn draw_hover_preview(&mut self, rt: &ID2D1RenderTarget, gfx: &Gfx, l: &Lay) {
        let (Some(ht), Drag::None) = (self.hover_t, &self.drag) else {
            super::uimap::remove("veditor.hoverpreview");
            return;
        };
        let s = self.scale;
        let p = theme::pal();
        let Some((_, small, _, _)) = self.fonts_ui.clone() else { return };
        let thumbs: Vec<(u32, Arc<crate::image::Frame>)> = self.strips.thumbs.lock().map(|g| g.clone()).unwrap_or_default();
        let (_, src_ms) = self.timeline.locate(ht);
        let Some(bm) = self.thumb_near(rt, &thumbs, src_ms) else { return };
        let w = 176.0 * s;
        let h = w * self.src_h as f32 / self.src_w.max(1) as f32;
        let x = self.x_of(l, ht);
        let cx = (x - w / 2.0 - 6.0 * s).clamp(l.sheet.left, l.sheet.right - w - 12.0 * s);
        let card = rect_f(cx, l.sheet.top - h - 36.0 * s, cx + w + 12.0 * s, l.sheet.top - 6.0 * s);
        unsafe {
            theme::glass(rt, gfx, card, 12.0 * s, s, None);
            let pic = rect_f(card.left + 6.0 * s, card.top + 6.0 * s, card.right - 6.0 * s, card.top + 6.0 * s + h);
            self.clipped(rt, pic, 7.0 * s, || {
                rt.DrawBitmap(&bm, Some(&pic), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            });
            if let Some(fg) = theme::brush(rt, p.label) {
                let tt = vedit::clock_fine(ht);
                let tw = gfx.measure(&small, &tt);
                d2d::draw_text(rt, &small, &tt, rect_f((card.left + card.right - tw) / 2.0, pic.bottom + 5.0 * s, card.right, card.bottom), &fg);
            }
        }
        let mut o = POINT::default();
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut o);
        }
        super::uimap::put("veditor.hoverpreview", Rect::new(card.left as i32 + o.x, card.top as i32 + o.y, card.right as i32 + o.x, card.bottom as i32 + o.y));
    }

    /// Status ("Split", "Saved ...", export progress) as a pill above the timeline.
    unsafe fn draw_toast(&self, rt: &ID2D1RenderTarget, gfx: &Gfx, l: &Lay) {
        if self.status.is_empty() {
            return;
        }
        let s = self.scale;
        let p = theme::pal();
        let Some((_, _, label, _)) = self.fonts_ui.clone() else { return };
        let w = gfx.measure(&label, &self.status) + 32.0 * s;
        let cx = (l.sheet.left + l.sheet.right) / 2.0;
        let h = 32.0 * s;
        let r = rect_f(cx - w / 2.0, l.sheet.top - h - 10.0 * s, cx + w / 2.0, l.sheet.top - 10.0 * s);
        unsafe {
            theme::glass(rt, gfx, r, h / 2.0, s, None);
            if let Some(w) = &self.work {
                let k = w.prog.permille.load(Ordering::Acquire) as f32 / 1000.0;
                if let Some(a) = theme::brush(rt, theme::with_alpha(p.accent, 0.22)) {
                    rt.FillRoundedRectangle(&theme::round(rect_f(r.left, r.top, r.left + (r.right - r.left) * k, r.bottom), h / 2.0), &a);
                }
            }
            if let Some(fg) = theme::brush(rt, p.label) {
                d2d::draw_text(rt, &label, &self.status, rect_f(r.left + 16.0 * s, r.top + 7.0 * s, r.right, r.bottom), &fg);
            }
        }
    }

    /// The tooltip, once the cursor has rested on something for a moment.
    unsafe fn draw_tip(&self, rt: &ID2D1RenderTarget, gfx: &Gfx) {
        let Some(tip) = &self.tip else {
            super::uimap::remove("veditor.tooltip");
            return;
        };
        if self.tip_since.elapsed().as_millis() < 480 || !matches!(self.drag, Drag::None) {
            super::uimap::remove("veditor.tooltip");
            return;
        }
        let s = self.scale;
        let p = theme::pal();
        let Some((_, small, label, _)) = self.fonts_ui.clone() else { return };
        let (cw, ch) = self.client();
        let tw = gfx.measure(&label, &tip.text);
        let kw = if tip.key.is_empty() { 0.0 } else { gfx.measure(&small, tip.key) + 18.0 * s };
        let w = tw + kw + 24.0 * s;
        let h = 30.0 * s;
        let c = self.last_cursor;
        let mut x = c.x as f32 + 12.0 * s;
        let mut y = c.y as f32 + 22.0 * s;
        if x + w > cw - 6.0 * s {
            x = cw - 6.0 * s - w;
        }
        if y + h > ch - 6.0 * s {
            y = c.y as f32 - h - 10.0 * s;
        }
        let r = rect_f(x, y, x + w, y + h);
        unsafe {
            theme::glass(rt, gfx, r, 9.0 * s, s, None);
            if let (Some(fg), Some(fg2), Some(dim)) = (theme::brush(rt, p.label), theme::brush(rt, p.label2), theme::brush(rt, p.label3)) {
                d2d::draw_text(rt, &label, &tip.text, rect_f(r.left + 12.0 * s, r.top + 6.0 * s, r.right, r.bottom), &fg);
                if !tip.key.is_empty() {
                    let kx = r.left + 12.0 * s + tw + 8.0 * s;
                    let kr = rect_f(kx, r.top + 7.0 * s, kx + gfx.measure(&small, tip.key) + 10.0 * s, r.bottom - 7.0 * s);
                    rt.DrawRoundedRectangle(&theme::round(kr, 4.0 * s), &dim, 1.0, None);
                    d2d::draw_text(rt, &small, tip.key, rect_f(kr.left + 5.0 * s, kr.top + 1.0 * s, kr.right, kr.bottom), &fg2);
                }
            }
        }
        let mut o = POINT::default();
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut o);
        }
        super::uimap::put("veditor.tooltip", Rect::new(r.left as i32 + o.x, r.top as i32 + o.y, r.right as i32 + o.x, r.bottom as i32 + o.y));
    }

    /// What the cursor is on, for the tooltip.
    fn tip_at(&self, p: Point) -> Option<Tip> {
        let l = self.lay();
        let (x, y) = (p.x as f32, p.y as f32);
        let t = |id: u32, text: &str, key: &'static str| Some(Tip { id, text: text.into(), key });
        if self.in_bar(p) {
            let b = self.strip.hit(p)?;
            let key: &'static str = match b {
                Btn::Tool(Tool::Select) => "V",
                Btn::Tool(Tool::Arrow) => "A",
                Btn::Tool(Tool::Line) => "L",
                Btn::Tool(Tool::Rect) => "R",
                Btn::Tool(Tool::Ellipse) => "E",
                Btn::Tool(Tool::Pen) => "P",
                Btn::Tool(Tool::Highlighter) => "H",
                Btn::Tool(Tool::Text) => "T",
                Btn::Tool(Tool::Blur) => "B",
                Btn::Tool(Tool::Step) => "N",
                Btn::Tool(Tool::Zoom) => "Z",
                Btn::Undo => "Ctrl+Z",
                Btn::Redo => "Ctrl+Y",
                Btn::Export => "Ctrl+E",
                _ => "",
            };
            let text = match b {
                Btn::Tool(Tool::Zoom) => "Zoom: drag a box on the video to zoom in there".to_string(),
                Btn::Tool(Tool::Blur) => "Blur: hide something for as long as its bar lasts".to_string(),
                Btn::Export => "Export an MP4 (copied to the clipboard)".to_string(),
                Btn::Gif => "Export a GIF".to_string(),
                Btn::WidthCycle => "Line width".to_string(),
                Btn::BlurMode => "Blur or pixelate".to_string(),
                Btn::Color(_) | Btn::Custom => "Colour".to_string(),
                other => other.name().trim_start_matches("tool.").to_string(),
            };
            return Some(Tip { id: 100 + b.key() as u32 % 1000, text, key });
        }
        if inside(&l.play, x, y) {
            return t(1, if self.player.playing() { "Pause" } else { "Play" }, "Space");
        }
        if inside(&l.split, x, y) {
            return t(2, "Cut the clip in two at the playhead", "S");
        }
        if inside(&l.delete, x, y) {
            return t(3, "Delete the selected clip or mark", "Del");
        }
        if inside(&l.zout, x, y) || inside(&l.zin, x, y) || inside(&l.slider, x, y) {
            return t(4, "Zoom the timeline", "Ctrl + wheel");
        }
        if inside(&l.zfit, x, y) {
            return t(5, "Show the whole video", "");
        }
        if inside(&l.ruler, x, y) {
            return t(6, "Click or drag to move the playhead", "\u{2190} \u{2192}");
        }
        if inside(&l.head, x, y) && x < l.x0 {
            if y < l.clips.bottom {
                return t(7, "Video: the clips, in the order they play", "");
            }
            if y < l.audio.bottom + 4.0 * self.scale {
                return t(8, "Audio: the sound of the clip above it", "");
            }
            return t(9, "Marks: arrows, text, blur and zooms over time", "");
        }
        if inside(&l.clips, x, y) {
            return match self.clip_at(&l, x) {
                Some((i, Some(_))) => Some(Tip { id: 20 + i as u32 * 3, text: "Drag to trim the clip".into(), key: "" }),
                Some((i, None)) => Some(Tip { id: 21 + i as u32 * 3, text: "Drag to reorder, click to select".into(), key: "" }),
                None => None,
            };
        }
        if inside(&l.audio, x, y) {
            return t(10, "Sound of the clip above (moves and trims with it)", "");
        }
        if inside(&l.items, x, y) {
            return match self.bar_at(&l, x, y) {
                Some((i, SpanMode::Move)) => Some(Tip { id: 5000 + i as u32 * 2, text: format!("{}: drag to move it in time", mark_look(&self.canvas.items[i], &theme::pal()).2), key: "" }),
                Some((i, _)) => Some(Tip { id: 5001 + i as u32 * 2, text: "Drag to change when it shows".into(), key: "" }),
                None if self.canvas.items.iter().all(|it| it.span.is_none()) => t(11, "Draw on the video to add a mark", ""),
                None => None,
            };
        }
        if inside(&l.scroll, x, y) && self.tv.zoom > 1.001 {
            return t(12, "Scroll the timeline", "wheel");
        }
        None
    }

    /// Selection handles, text box and caret, zoom areas.
    unsafe fn draw_marks_ui(&self, rt: &ID2D1RenderTarget, l: &Lay) {
        let s = self.scale;
        let p = theme::pal();
        let (Some(acc), Ok(white), Some(zb)) = (theme::brush(rt, p.accent), d2d::brush(rt, rgba(0xFFFFFF, 1.0)), theme::brush(rt, p.purple)) else { return };
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
                        rt.DrawRoundedRectangle(&theme::round(rect_f(x0, y0, x1, y1), 6.0 * s), &zb, w, None);
                    }
                }
                if let Some(d) = &c.draft {
                    if let Shape::Zoom { a, b } = &d.shape {
                        let z = vedit::zoom_rect(*a, *b, fw, fh);
                        let (x0, y0) = self.to_screen(l, P::new(z.x, z.y));
                        let (x1, y1) = self.to_screen(l, P::new(z.x + z.w, z.y + z.h));
                        rt.DrawRoundedRectangle(&theme::round(rect_f(x0, y0, x1, y1), 6.0 * s), &zb, 2.0 * s, None);
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
                        super::editor::handle_dot(rt, x, y, s, &white, &acc);
                    }
                }
            }
            if let Some(t) = &c.text {
                if let Some(f) = c.fonts() {
                    let (l0, tp, r0, b0) = font::text_box(&f.text, t.size, &t.text, t.at.x, t.at.y);
                    let (x0, y0) = self.to_screen(l, P::new(l0, tp));
                    let (x1, y1) = self.to_screen(l, P::new(r0, b0));
                    rt.DrawRoundedRectangle(&theme::round(rect_f(x0 - 5.0, y0 - 4.0, x1 + 5.0, y1 + 4.0), 4.0 * s), &acc, 1.0, None);
                    if self.caret_on {
                        let last = t.text.split('\n').last().unwrap_or("");
                        let lines = t.text.split('\n').count().max(1) as f32;
                        let lh = font::line_height(&f.text, t.size);
                        let cx = t.at.x + font::line_width(&f.text, t.size, last);
                        let cy = t.at.y + lh * (lines - 1.0);
                        let (sx, sy) = self.to_screen(l, P::new(cx, cy));
                        let (_, ey) = self.to_screen(l, P::new(cx, cy + lh));
                        rt.FillRoundedRectangle(&theme::round(rect_f(sx + 1.0, sy + 2.0, sx + 1.0 + 2.0 * s, ey - 2.0), 1.0 * s), &acc);
                    }
                }
            }
        }
    }
}

/// Colour, icon and name of a mark's bar.
fn mark_look(it: &Item, p: &crate::palette::Palette) -> (u32, &'static str, String) {
    match &it.shape {
        Shape::Zoom { .. } => (p.purple.0, "\u{E71E}", "Zoom".into()),
        Shape::Text { text, .. } => (it.color.hex(), "\u{E8D2}", text.lines().next().unwrap_or("Text").to_string()),
        Shape::Step { .. } => (it.color.hex(), "\u{E9F5}", "Step".into()),
        Shape::Arrow { .. } => (it.color.hex(), "\u{E72A}", "Arrow".into()),
        Shape::Blur { .. } => (0x8E8E93, "\u{E7B3}", "Blur".into()),
        Shape::Rect { .. } => (it.color.hex(), "\u{E739}", "Rectangle".into()),
        Shape::Ellipse { .. } => (it.color.hex(), "\u{EA3A}", "Ellipse".into()),
        Shape::Line { .. } => (it.color.hex(), "\u{E738}", "Line".into()),
        Shape::Pen { .. } => (it.color.hex(), "\u{EDC6}", "Pen".into()),
        Shape::Highlighter { .. } => (it.color.hex(), "\u{E7E6}", "Marker".into()),
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

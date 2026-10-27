//! Full-desktop overlay: frozen frame, dimming, selection, loupe and the top
//! mini panel. One window spans the whole virtual desktop; client pixels map
//! 1:1 to desktop pixels (offset by the desktop origin).

use super::d2d::{self, Gfx, rect_f, rgba};
use super::util::lparam_point;
use super::{WM_APP_KEY, class_name};
use crate::geom::{Point, Rect};
use crate::image::{Frame, hex_color};
use crate::select::{Key, Outcome, Selection};
use super::elements::{Elements, Query};
use super::toolstrip::{Btn, Slot, Strip, StripState};
use crate::annot::edit::{Canvas, Change, Key as AKey};
use crate::annot::render::{self as arender, bbox};
use crate::annot::{BlurMode, Color, Item, P, PALETTE, Tool};
use super::util::log;
use super::WM_APP_ELEMENTS;
use std::sync::Arc;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const ACCENT: u32 = 0x4C8DFF;
const PANEL_BG: u32 = 0x1E1E20;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PanelItem {
    Photo,
    Video,
    Fps,
    Res,
    Sound,
    Mic,
    Folder,
    Close,
}

impl PanelItem {
    /// Items in the same group sit together; a separator goes between groups.
    fn group(&self) -> u8 {
        match self {
            PanelItem::Photo | PanelItem::Video => 0,
            PanelItem::Fps | PanelItem::Res | PanelItem::Sound | PanelItem::Mic => 1,
            PanelItem::Folder => 2,
            PanelItem::Close => 3,
        }
    }
}

/// What the panel shows; owned by the app.
#[derive(Clone, Debug, Default)]
pub struct PanelState {
    pub video: bool,
    pub fps_label: String,
    pub res_label: String,
    pub sound: bool,
    pub mic: bool,
}

pub enum Event {
    Confirm(Rect),
    Cancel,
    CopyColor(String),
    /// Open the folder dropdown at this desktop point.
    FolderMenu(Point),
    Mode(bool),
    FpsMenu(Point),
    ResMenu(Point),
    ToggleSound,
    ToggleMic,
    /// Finish and also save the file.
    ConfirmSave(Rect),
    /// Finish into the editor window.
    OpenEditor(Rect),
    /// Colour / thickness picked on the strip: remember them.
    Style(Color, usize),
}

/// Quick annotation mode (frame released with Ctrl held).
struct Quick {
    canvas: Canvas,
    base: tiny_skia::Pixmap,
    cache: Option<(u64, Rect, ID2D1Bitmap)>,
    strip: Strip,
    palette: Option<Strip>,
    dragging: bool,
}

const QUICK_SLOTS: [Slot; 18] = [
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
    Slot::Sep,
    Slot::B(Btn::Palette),
    Slot::B(Btn::WidthCycle),
    Slot::B(Btn::Undo),
    Slot::Sep,
    Slot::B(Btn::Copy),
    Slot::B(Btn::Save),
    Slot::B(Btn::OpenEditor),
];

#[derive(Clone)]
struct Fonts {
    ui: IDWriteTextFormat,
    small: IDWriteTextFormat,
    mono: IDWriteTextFormat,
    icon: IDWriteTextFormat,
}

pub struct Overlay {
    pub hwnd: HWND,
    rt: Option<ID2D1HwndRenderTarget>,
    rt_size: (u32, u32),
    bmp: Option<ID2D1Bitmap>,
    frame: Option<Arc<Frame>>,
    /// Top-level windows (raw HWND, rect clipped to the desktop), topmost first.
    windows: Vec<(isize, Rect)>,
    elements: Elements,
    capture: u64,
    /// Last point sent to the element worker.
    asked: Option<Point>,
    debug: bool,
    quick: Option<Quick>,
    gfx: Option<Gfx>,
    /// Colour and thickness for quick marks.
    pub style: (Color, usize),
    vd: Rect,
    sel: Option<Selection>,
    /// Monitor (desktop coords) where the panel sits, and its UI scale.
    monitor: Rect,
    scale: f32,
    fonts: Option<(f32, Fonts)>,
    loupe: bool,
    folder_label: String,
    state: PanelState,
    panel: Vec<(PanelItem, Rect)>,
    panel_rect: Rect,
    panel_hover: Option<PanelItem>,
    panel_press: Option<PanelItem>,
}

fn monitor_at(p: Point) -> (Rect, f32) {
    unsafe {
        let m = MonitorFromPoint(POINT { x: p.x, y: p.y }, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(m, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        (super::util::to_rect(info.rcMonitor), dx as f32 / 96.0)
    }
}

impl Overlay {
    pub fn create(instance: HINSTANCE) -> windows::core::Result<Overlay> {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class_name::OVERLAY,
                windows::core::w!("Rengi Snap"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )?
        };
        Ok(Overlay {
            hwnd,
            rt: None,
            rt_size: (0, 0),
            bmp: None,
            frame: None,
            windows: Vec::new(),
            elements: Elements::start(hwnd),
            capture: 0,
            asked: None,
            debug: std::env::var_os("RENGI_SNAP_DEBUG").is_some(),
            quick: None,
            gfx: None,
            style: (PALETTE[0], 1),
            vd: Rect::default(),
            sel: None,
            monitor: Rect::default(),
            scale: 1.0,
            fonts: None,
            loupe: true,
            folder_label: String::new(),
            state: PanelState::default(),
            panel: Vec::new(),
            panel_rect: Rect::default(),
            panel_hover: None,
            panel_press: None,
        })
    }

    pub fn frame(&self) -> Option<Arc<Frame>> {
        self.frame.clone()
    }

    pub fn is_active(&self) -> bool {
        self.sel.is_some()
    }

    pub fn set_state(&mut self, state: PanelState, gfx: &Gfx) {
        self.state = state;
        self.layout_panel(gfx);
        self.invalidate();
    }

    pub fn is_video(&self) -> bool {
        self.state.video
    }

    pub fn set_folder_label(&mut self, label: String, gfx: &Gfx) {
        self.folder_label = label;
        self.layout_panel(gfx);
        self.invalidate();
    }

    pub fn show(&mut self, gfx: &Gfx, frame: Arc<Frame>, windows: Vec<(isize, Rect)>, monitors: Vec<Rect>, loupe: bool, folder_label: String, state: PanelState) -> bool {
        let vd = frame.rect;
        let (w, h) = (vd.width() as u32, vd.height() as u32);
        if self.rt.is_none() || self.rt_size != (w, h) {
            self.rt = gfx.target(self.hwnd, w, h).ok();
            self.rt_size = (w, h);
        }
        let Some(rt) = self.rt.clone() else { return false };
        self.bmp = d2d::bitmap(&rt, w, h, &frame.pixels).ok();
        if self.bmp.is_none() {
            return false;
        }
        let mut cur = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cur);
        }
        let cursor = Point::new(cur.x, cur.y);
        let (monitor, scale) = monitor_at(cursor);
        self.monitor = monitor;
        self.scale = scale;
        self.vd = vd;
        self.frame = Some(frame);
        self.loupe = loupe;
        self.folder_label = folder_label;
        self.state = state;
        self.quick = None;
        self.gfx = Some(gfx.clone());
        let tol = (6.0 * scale).round() as i32;
        self.windows = windows.iter().map(|(h, r)| (*h, r.intersect(&vd))).filter(|(_, r)| !r.is_empty()).collect();
        let mut sel = Selection::new(vd, self.windows.iter().map(|w| w.1).collect(), monitors, tol);
        sel.mouse_move(cursor);
        self.sel = Some(sel);
        self.capture += 1;
        self.asked = None;
        self.ask_elements(cursor);
        self.panel_hover = None;
        self.panel_press = None;
        if self.fonts.as_ref().map(|f| f.0) != Some(scale) {
            self.fonts = make_fonts(gfx, scale).map(|f| (scale, f));
        }
        self.layout_panel(gfx);
        self.render();
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                vd.left,
                vd.top,
                vd.width(),
                vd.height(),
                SWP_SHOWWINDOW | SWP_NOACTIVATE,
            );
            self.render();
            force_foreground(self.hwnd);
        }
        true
    }

    /// Sends the point to the element worker (latest query wins there).
    fn ask_elements(&mut self, p: Point) {
        let Some(sel) = &self.sel else { return };
        if !sel.is_hovering() || self.asked == Some(p) {
            return;
        }
        let (Some(i), Some(frame)) = (sel.window_index(p), self.frame.clone()) else { return };
        let (top, window) = self.windows[i];
        self.asked = Some(p);
        self.elements.ask(Query { capture: self.capture, point: p, top, window, frame });
    }

    pub fn hide(&mut self) {
        self.quick = None;
        super::hook::set_passthrough(false);
        self.sel = None;
        self.windows.clear();
        self.bmp = None;
        self.frame = None;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn to_desktop(&self, p: Point) -> Point {
        Point::new(p.x + self.vd.left, p.y + self.vd.top)
    }

    fn layout_panel(&mut self, gfx: &Gfx) {
        self.panel.clear();
        let Some((_, f)) = &self.fonts else { return };
        let s = self.scale;
        let h = (40.0 * s).round() as i32;
        let pad = (6.0 * s).round() as i32;
        let icon_w = (18.0 * s).round() as i32;
        let gap = (6.0 * s).round() as i32;
        let side = (12.0 * s) as i32;
        let text_w = |label: &str| gfx.measure(&f.ui, label).ceil() as i32;
        let seg = |label: &str| side * 2 + icon_w + gap + text_w(label);
        let drop = |label: &str| side * 2 + icon_w + gap * 2 + text_w(label) + side;
        let square = h - pad * 2;
        let mut items = vec![(PanelItem::Photo, seg("Photo")), (PanelItem::Video, seg("Video"))];
        if self.state.video {
            let fps_w = side * 2 + text_w(&self.state.fps_label) + gap + side;
            let res_w = side * 2 + text_w(&self.state.res_label) + gap + side;
            items.extend([(PanelItem::Fps, fps_w), (PanelItem::Res, res_w), (PanelItem::Sound, square), (PanelItem::Mic, square)]);
        }
        items.push((PanelItem::Folder, drop(&self.folder_label)));
        items.push((PanelItem::Close, square));
        let sep = (13.0 * s).round() as i32;
        let groups = items.windows(2).filter(|w| w[0].0.group() != w[1].0.group()).count() as i32;
        let total: i32 = pad * 2 + items.iter().map(|w| w.1).sum::<i32>() + sep * groups;
        let left = self.monitor.left + (self.monitor.width() - total) / 2;
        let top = self.monitor.top + (16.0 * s).round() as i32;
        self.panel_rect = Rect::new(left, top, left + total, top + h);
        let mut x = left + pad;
        let mut prev: Option<u8> = None;
        for (item, w) in items {
            if prev.is_some_and(|g| g != item.group()) {
                x += sep;
            }
            prev = Some(item.group());
            self.panel.push((item, Rect::new(x, top + pad, x + w, top + h - pad)));
            x += w;
        }
    }

    fn panel_item_at(&self, p: Point) -> Option<PanelItem> {
        if !self.panel_rect.contains(p) {
            return None;
        }
        self.panel.iter().find(|(_, r)| r.contains(p)).map(|(i, _)| *i)
    }

    /// Returns `Some(result)` when the message was handled, plus an event for the app.
    pub fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> (Option<LRESULT>, Option<Event>) {
        if self.sel.is_none() {
            return match msg {
                WM_PAINT => {
                    unsafe {
                        let _ = ValidateRect(Some(self.hwnd), None);
                    }
                    (Some(LRESULT(0)), None)
                }
                _ => (None, None),
            };
        }
        if self.quick.is_some() {
            if let Some(r) = self.quick_handle(msg, wp, lp) {
                return r;
            }
        }
        match msg {
            WM_PAINT => {
                self.render();
                unsafe {
                    let _ = ValidateRect(Some(self.hwnd), None);
                }
                (Some(LRESULT(0)), None)
            }
            WM_ERASEBKGND => (Some(LRESULT(1)), None),
            WM_SETCURSOR => {
                self.set_cursor();
                (Some(LRESULT(1)), None)
            }
            WM_MOUSEMOVE => {
                let p = self.to_desktop(lparam_point(lp.0));
                let hover = self.panel_item_at(p);
                let mut redraw = hover != self.panel_hover;
                self.panel_hover = hover;
                if self.panel_press.is_none() {
                    let sel = self.sel.as_mut().unwrap();
                    redraw |= sel.mouse_move(p) != Outcome::Nothing;
                    if hover.is_none() {
                        self.ask_elements(p);
                    }
                }
                if redraw || self.loupe {
                    self.invalidate();
                }
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONDOWN => {
                let p = self.to_desktop(lparam_point(lp.0));
                unsafe {
                    SetCapture(self.hwnd);
                }
                if let Some(item) = self.panel_item_at(p) {
                    self.panel_press = Some(item);
                    self.invalidate();
                    return (Some(LRESULT(0)), None);
                }
                if self.panel_rect.contains(p) {
                    return (Some(LRESULT(0)), None);
                }
                let sel = self.sel.as_mut().unwrap();
                sel.mouse_down(p);
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONUP => {
                let p = self.to_desktop(lparam_point(lp.0));
                unsafe {
                    let _ = ReleaseCapture();
                }
                if let Some(pressed) = self.panel_press.take() {
                    self.invalidate();
                    if self.panel_item_at(p) == Some(pressed) {
                        return (Some(LRESULT(0)), self.panel_click(pressed));
                    }
                    return (Some(LRESULT(0)), None);
                }
                let ctrl = wp.0 & 0x0008 != 0; // MK_CONTROL
                let out = self.sel.as_mut().unwrap().mouse_up(p, ctrl);
                (Some(LRESULT(0)), self.outcome(out))
            }
            WM_LBUTTONDBLCLK => {
                let p = self.to_desktop(lparam_point(lp.0));
                let out = self.sel.as_mut().unwrap().double_click(p);
                (Some(LRESULT(0)), self.outcome(out))
            }
            WM_RBUTTONUP => (Some(LRESULT(0)), Some(Event::Cancel)),
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
                if delta != 0 {
                    let out = self.sel.as_mut().unwrap().wheel(delta > 0);
                    if self.debug {
                        log(&format!("wheel {delta} -> {out:?} frame {:?}", self.sel.as_ref().unwrap().view().frame));
                    }
                    return (Some(LRESULT(0)), self.outcome(out));
                }
                (Some(LRESULT(0)), None)
            }
            WM_APP_ELEMENTS => {
                if let Some(a) = self.elements.take() {
                    if self.debug {
                        log(&format!(
                            "elements {} ms: hwnd {} uia {} pixel {} | {}",
                            a.ms, a.hwnd_n, a.uia_n, a.pixel_n, a.trace
                        ));
                    }
                    if a.capture == self.capture {
                        let out = self.sel.as_mut().unwrap().set_elements(a.window, a.rects);
                        if self.debug {
                            log(&format!("chain now {:?}", self.sel.as_ref().map(|s| s.chain(s.pos()))));
                        }
                        return (Some(LRESULT(0)), self.outcome(out));
                    }
                }
                (Some(LRESULT(0)), None)
            }
            WM_APP_KEY => {
                let vk = wp.0 as u16;
                let shift = lp.0 != 0;
                let key = match VIRTUAL_KEY(vk) {
                    VK_ESCAPE => Some(Key::Escape),
                    VK_RETURN => Some(Key::Enter),
                    VK_LEFT => Some(Key::Left),
                    VK_RIGHT => Some(Key::Right),
                    VK_UP => Some(Key::Up),
                    VK_DOWN => Some(Key::Down),
                    _ => None,
                };
                if let Some(key) = key {
                    let out = self.sel.as_mut().unwrap().key(key, shift);
                    return (Some(LRESULT(0)), self.outcome(out));
                }
                if VIRTUAL_KEY(vk) == VK_C {
                    let pos = self.sel.as_ref().unwrap().pos();
                    if let Some(px) = self.frame.as_ref().and_then(|f| f.pixel(pos.x, pos.y)) {
                        return (Some(LRESULT(0)), Some(Event::CopyColor(hex_color(px))));
                    }
                }
                (Some(LRESULT(0)), None)
            }
            _ => (None, None),
        }
    }

    fn outcome(&mut self, out: Outcome) -> Option<Event> {
        if self.debug && matches!(out, Outcome::Confirm(_) | Outcome::Cancel) {
            log(&format!("outcome {out:?}"));
        }
        self.sync_quick();
        match out {
            Outcome::Confirm(r) => Some(Event::Confirm(r)),
            Outcome::Cancel => Some(Event::Cancel),
            Outcome::Redraw => {
                self.invalidate();
                None
            }
            Outcome::Nothing => None,
        }
    }

    fn to_local(&self, p: Point) -> P {
        P::new((p.x - self.vd.left) as f32, (p.y - self.vd.top) as f32)
    }

    fn frame_rect(&self) -> Option<Rect> {
        let sel = self.sel.as_ref()?;
        if sel.is_editing() { sel.view().frame } else { None }
    }

    /// Starts quick mode when the frame is being edited, keeps it in step with
    /// the frame, drops it when the frame is gone.
    fn sync_quick(&mut self) {
        let Some(fr) = self.frame_rect() else {
            if self.quick.as_ref().is_some_and(|q| q.canvas.items.is_empty()) || self.sel.as_ref().is_some_and(|s| !s.is_editing()) {
                self.quick = None;
                super::hook::set_passthrough(false);
            }
            return;
        };
        let lf = Rect::new(fr.left - self.vd.left, fr.top - self.vd.top, fr.right - self.vd.left, fr.bottom - self.vd.top);
        if self.quick.is_none() {
            let Some(frame) = self.frame.clone() else { return };
            let mut canvas = Canvas::new(lf, self.scale, crate::annot::font::shared());
            canvas.tool = Tool::Select;
            canvas.color = self.style.0;
            canvas.width_idx = self.style.1;
            self.quick = Some(Quick { canvas, base: arender::base_pixmap(&frame), cache: None, strip: Strip::new(), palette: None, dragging: false });
        }
        let s = self.scale;
        let mon = monitor_at(Point::new(fr.left + fr.width() / 2, fr.top + fr.height() / 2)).0;
        let Some(gfx) = self.gfx.clone() else { return };
        let q = self.quick.as_mut().unwrap();
        q.canvas.bounds = lf;
        let h = (44.0 * s).round() as i32;
        q.strip.layout(&gfx, &QUICK_SLOTS, Point::new(0, 0), h, 0, s);
        let (w, gap) = (q.strip.rect.width(), (10.0 * s) as i32);
        let x = (fr.left + fr.width() / 2 - w / 2).clamp(mon.left + gap, (mon.right - w - gap).max(mon.left + gap));
        let y = if fr.bottom + gap + h <= mon.bottom {
            fr.bottom + gap
        } else if fr.top - gap - h >= mon.top {
            fr.top - gap - h
        } else {
            fr.bottom - h - gap
        };
        q.strip.translate(x, y);
        if q.palette.is_some() {
            let mut pal = Strip::new();
            let slots: Vec<Slot> = (0..8).map(|i| Slot::B(Btn::Color(i))).collect();
            pal.layout(&gfx, &slots, Point::new(0, 0), (40.0 * s) as i32, 0, s);
            let anchor = q.strip.items.iter().find(|(b, _)| *b == Btn::Palette).map(|(_, r)| *r).unwrap_or(q.strip.rect);
            let py = if y >= fr.bottom { q.strip.rect.bottom + 6 } else { q.strip.rect.top - 6 - pal.rect.height() };
            pal.translate(anchor.left - (8.0 * s) as i32, py);
            q.palette = Some(pal);
        }
        self.invalidate();
    }

    fn quick_state(&self) -> Option<StripState> {
        let c = &self.quick.as_ref()?.canvas;
        Some(StripState { tool: c.tool, color: c.color, width_idx: c.width_idx, blur_mode: c.blur_mode, can_undo: c.can_undo(), can_redo: c.can_redo(), custom: c.color })
    }

    fn quick_change(&mut self, c: Change) {
        let typing = self.quick.as_ref().is_some_and(|q| q.canvas.text.is_some());
        super::hook::set_passthrough(typing);
        if c != Change::None {
            self.invalidate();
        }
    }

    /// Items translated to `rect`-local coordinates (rect in desktop coords).
    pub fn take_items(&mut self, rect: Rect) -> Vec<Item> {
        let Some(q) = self.quick.as_mut() else { return Vec::new() };
        let _ = q.canvas.finish_text();
        let (dx, dy) = ((rect.left - self.vd.left) as f32, (rect.top - self.vd.top) as f32);
        q.canvas
            .items
            .iter()
            .cloned()
            .map(|mut it| {
                it.translate(-dx, -dy);
                it
            })
            .collect()
    }

    fn quick_button(&mut self, b: Btn) -> Option<Event> {
        let fr = self.frame_rect()?;
        let q = self.quick.as_mut()?;
        let c = match b {
            Btn::Tool(t) => q.canvas.set_tool(t),
            Btn::Palette => {
                q.palette = if q.palette.is_some() { None } else { Some(Strip::new()) };
                self.sync_quick();
                return None;
            }
            Btn::Color(i) => {
                q.palette = None;
                let ch = q.canvas.set_color(PALETTE[i]);
                let style = (q.canvas.color, q.canvas.width_idx);
                self.style = style;
                self.quick_change(ch);
                self.sync_quick();
                return Some(Event::Style(style.0, style.1));
            }
            Btn::WidthCycle => {
                let ch = q.canvas.set_width((q.canvas.width_idx + 1) % 3);
                let style = (q.canvas.color, q.canvas.width_idx);
                self.style = style;
                self.quick_change(ch);
                return Some(Event::Style(style.0, style.1));
            }
            Btn::BlurMode => {
                let m = if q.canvas.blur_mode == BlurMode::Blur { BlurMode::Pixelate } else { BlurMode::Blur };
                q.canvas.set_blur_mode(m)
            }
            Btn::Undo => q.canvas.key(AKey::Undo),
            Btn::Copy => return Some(Event::Confirm(fr)),
            Btn::Save => return Some(Event::ConfirmSave(fr)),
            Btn::OpenEditor => return Some(Event::OpenEditor(fr)),
            _ => Change::None,
        };
        self.quick_change(c);
        self.invalidate();
        None
    }

    /// Quick-mode input. `None` = not handled here, fall through.
    fn quick_handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<(Option<LRESULT>, Option<Event>)> {
        let handled = |e: Option<Event>| Some((Some(LRESULT(0)), e));
        let fr = self.frame_rect()?;
        match msg {
            WM_MOUSEMOVE => {
                let p = self.to_desktop(lparam_point(lp.0));
                let lp_ = self.to_local(p);
                let shift = wp.0 & 0x0004 != 0;
                let q = self.quick.as_mut()?;
                let hov = q.palette.as_ref().and_then(|s| s.hit(p)).or_else(|| q.strip.hit(p));
                let mut redraw = false;
                if hov != q.strip.hover {
                    q.strip.hover = hov;
                    if let Some(pal) = &mut q.palette {
                        pal.hover = hov;
                    }
                    redraw = true;
                }
                if q.dragging {
                    let c = q.canvas.mouse_move(lp_, shift);
                    self.quick_change(c);
                    return handled(None);
                }
                if redraw {
                    self.invalidate();
                }
                None
            }
            WM_LBUTTONDOWN => {
                let p = self.to_desktop(lparam_point(lp.0));
                let lp_ = self.to_local(p);
                let shift = wp.0 & 0x0004 != 0;
                let q = self.quick.as_mut()?;
                if let Some(b) = q.palette.as_ref().and_then(|s| s.hit(p)).or_else(|| q.strip.hit(p)) {
                    q.strip.press = Some(b);
                    unsafe {
                        SetCapture(self.hwnd);
                    }
                    self.invalidate();
                    return handled(None);
                }
                if q.strip.rect.contains(p) || q.palette.as_ref().is_some_and(|s| s.rect.contains(p)) {
                    return handled(None);
                }
                let typing = q.canvas.text.is_some();
                let tool = q.canvas.tool;
                let on_mark = tool == Tool::Select && q.canvas.wants(lp_);
                if typing || on_mark || (tool != Tool::Select && fr.contains(p)) {
                    q.dragging = true;
                    unsafe {
                        SetCapture(self.hwnd);
                    }
                    let c = q.canvas.mouse_down(lp_, shift);
                    self.quick_change(c);
                    return handled(None);
                }
                if tool != Tool::Select {
                    return handled(None);
                }
                // Select tool off the marks: the frame itself. With marks on
                // it, a click outside must not throw them away.
                let near_frame = self.sel.as_ref().and_then(|s| s.hit(p)).is_some();
                if !near_frame && !q.canvas.items.is_empty() {
                    return handled(None);
                }
                None
            }
            WM_LBUTTONUP => {
                let p = self.to_desktop(lparam_point(lp.0));
                let lp_ = self.to_local(p);
                let shift = wp.0 & 0x0004 != 0;
                let q = self.quick.as_mut()?;
                if let Some(pressed) = q.strip.press.take() {
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    self.invalidate();
                    let q = self.quick.as_ref()?;
                    let over = q.palette.as_ref().and_then(|s| s.hit(p)).or_else(|| q.strip.hit(p));
                    if over == Some(pressed) {
                        let e = self.quick_button(pressed);
                        return handled(e);
                    }
                    return handled(None);
                }
                if q.dragging {
                    q.dragging = false;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    let c = q.canvas.mouse_up(lp_, shift);
                    self.quick_change(c);
                    return handled(None);
                }
                None
            }
            WM_LBUTTONDBLCLK => {
                let p = self.to_desktop(lparam_point(lp.0));
                let lp_ = self.to_local(p);
                let q = self.quick.as_mut()?;
                if q.strip.rect.contains(p) {
                    return handled(None);
                }
                let is_text = q.canvas.hit(lp_).is_some_and(|i| matches!(q.canvas.items[i].shape, crate::annot::Shape::Text { .. }));
                if is_text && matches!(q.canvas.tool, Tool::Select | Tool::Text) {
                    let c = q.canvas.double_click(lp_);
                    self.quick_change(c);
                    return handled(None);
                }
                if q.canvas.tool != Tool::Select {
                    return handled(None);
                }
                None
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
                let q = self.quick.as_mut()?;
                let typing = q.canvas.text.is_some();
                let c = match vk {
                    VK_Z if ctrl => q.canvas.key(AKey::Undo),
                    VK_Y if ctrl => q.canvas.key(AKey::Redo),
                    VK_DELETE => q.canvas.key(AKey::Delete),
                    VK_BACK => q.canvas.key(AKey::Backspace),
                    VK_ESCAPE if typing => q.canvas.key(AKey::Escape),
                    VK_RETURN if typing => q.canvas.key(AKey::Enter),
                    _ if !typing && !ctrl => {
                        let ch = char::from_u32(vk.0 as u32).unwrap_or('\0');
                        match Tool::from_key(ch) {
                            Some(t) if t != Tool::Crop => q.canvas.set_tool(t),
                            _ => return None,
                        }
                    }
                    _ => return None,
                };
                self.quick_change(c);
                self.invalidate();
                handled(None)
            }
            WM_CHAR => {
                let q = self.quick.as_mut()?;
                if q.canvas.text.is_none() {
                    return None;
                }
                let c = char::from_u32(wp.0 as u32).map(|ch| q.canvas.char(ch)).unwrap_or(Change::None);
                self.quick_change(c);
                handled(None)
            }
            WM_APP_KEY => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let q = self.quick.as_mut()?;
                match vk {
                    VK_ESCAPE if q.canvas.is_busy() || q.canvas.selected.is_some() || q.palette.is_some() => {
                        q.palette = None;
                        let c = q.canvas.key(AKey::Escape);
                        self.quick_change(c);
                        self.invalidate();
                        handled(None)
                    }
                    VK_C => handled(None),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    unsafe fn draw_quick(&mut self, rt: &ID2D1RenderTarget) {
        let Some(st) = self.quick_state() else { return };
        let Some(gfx) = self.gfx.clone() else { return };
        let (vx, vy) = (self.vd.left, self.vd.top);
        let s = self.scale;
        let q = self.quick.as_mut().unwrap();
        // Selection box, handles, text box and caret (image coords = vd-local).
        if let (Ok(acc), Ok(white)) = (d2d::brush(rt, rgba(ACCENT, 1.0)), d2d::brush(rt, rgba(0xFFFFFF, 1.0))) {
            let c = &q.canvas;
            if let Some(it) = c.selected.and_then(|i| c.items.get(i)) {
                let (l, t, r, b) = bbox(it, c.fonts());
                unsafe { rt.DrawRectangle(&rect_f(l - 2.0, t - 2.0, r + 2.0, b + 2.0), &acc, 1.0, None) };
                for (_, hp) in c.handles() {
                    let hs = 4.5 * s;
                    let rr = rect_f(hp.x - hs, hp.y - hs, hp.x + hs, hp.y + hs);
                    unsafe {
                        rt.FillRectangle(&rr, &white);
                        rt.DrawRectangle(&rr, &acc, 1.0, None);
                    }
                }
            }
            if let (Some(t), Some(f)) = (&c.text, c.fonts()) {
                let (l, tp, r, b) = crate::annot::font::text_box(&f.text, t.size, &t.text, t.at.x, t.at.y);
                unsafe { rt.DrawRectangle(&rect_f(l - 4.0, tp - 3.0, r + 4.0, b + 3.0), &acc, 1.0, None) };
                let last = t.text.split('\n').last().unwrap_or("");
                let lines = t.text.split('\n').count().max(1) as f32;
                let lh = crate::annot::font::line_height(&f.text, t.size);
                let cx = t.at.x + crate::annot::font::line_width(&f.text, t.size, last);
                let cy = t.at.y + lh * (lines - 1.0);
                unsafe { rt.FillRectangle(&rect_f(cx + 1.0, cy + 2.0, cx + 1.0 + 1.5 * s, cy + lh - 2.0), &acc) };
            }
        }
        let off = (-vx as f32, -vy as f32);
        q.strip.draw(rt, &gfx, &st, off, true);
        if let Some(pal) = &mut q.palette {
            pal.draw(rt, &gfx, &st, off, true);
        }
    }

    fn panel_click(&mut self, item: PanelItem) -> Option<Event> {
        let below = |me: &Self, it: PanelItem| -> Option<Point> {
            let r = me.panel.iter().find(|(i, _)| *i == it).map(|(_, r)| *r)?;
            Some(Point::new(r.left, me.panel_rect.bottom + (4.0 * me.scale) as i32))
        };
        match item {
            PanelItem::Photo => (self.state.video).then_some(Event::Mode(false)),
            PanelItem::Video => (!self.state.video).then_some(Event::Mode(true)),
            PanelItem::Close => Some(Event::Cancel),
            PanelItem::Folder => below(self, item).map(Event::FolderMenu),
            PanelItem::Fps => below(self, item).map(Event::FpsMenu),
            PanelItem::Res => below(self, item).map(Event::ResMenu),
            PanelItem::Sound => Some(Event::ToggleSound),
            PanelItem::Mic => Some(Event::ToggleMic),
        }
    }

    fn set_cursor(&self) {
        let mut cur = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cur);
        }
        let p = Point::new(cur.x, cur.y);
        let quick_cursor = self.quick.as_ref().map(|q| {
            if q.strip.rect.contains(p) || q.palette.as_ref().is_some_and(|s| s.rect.contains(p)) {
                IDC_HAND
            } else {
                let lp_ = P::new((p.x - self.vd.left) as f32, (p.y - self.vd.top) as f32);
                match q.canvas.tool {
                    Tool::Select if q.canvas.wants(lp_) => IDC_SIZEALL,
                    Tool::Select => IDC_ARROW,
                    Tool::Text => IDC_IBEAM,
                    _ => IDC_CROSS,
                }
            }
        });
        let id = if self.panel_rect.contains(p) {
            if self.panel_item_at(p).is_none() { IDC_ARROW } else { IDC_HAND }
        } else if let Some(c) = quick_cursor.filter(|c| *c != IDC_ARROW) {
            c
        } else {
            match self.sel.as_ref().and_then(|s| s.hit(p)) {
                Some(e) if (e.left && e.top) || (e.right && e.bottom) => IDC_SIZENWSE,
                Some(e) if (e.right && e.top) || (e.left && e.bottom) => IDC_SIZENESW,
                Some(e) if e.left || e.right => IDC_SIZEWE,
                Some(e) if e.top || e.bottom => IDC_SIZENS,
                Some(_) => IDC_SIZEALL,
                None => IDC_CROSS,
            }
        };
        unsafe {
            if let Ok(c) = LoadCursorW(None, id) {
                SetCursor(Some(c));
            }
        }
    }

    fn local(&self, r: Rect) -> D2D_RECT_F {
        rect_f(
            (r.left - self.vd.left) as f32,
            (r.top - self.vd.top) as f32,
            (r.right - self.vd.left) as f32,
            (r.bottom - self.vd.top) as f32,
        )
    }

    fn render(&mut self) {
        let (Some(rt), Some(bmp), Some(sel)) = (self.rt.clone(), self.bmp.clone(), self.sel.as_ref()) else { return };
        let Some(fonts) = self.fonts.as_ref().map(|f| f.1.clone()) else { return };
        let fonts = &fonts;
        let s = self.scale;
        let view = sel.view();
        let pos = sel.pos();
        let full = rect_f(0.0, 0.0, self.vd.width() as f32, self.vd.height() as f32);
        let rt1: &ID2D1RenderTarget = &rt;
        unsafe {
            rt.BeginDraw();
            rt.DrawBitmap(&bmp, Some(&full), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None);
            let dim = d2d::brush(rt1, rgba(0x000000, 0.45)).ok();
            let accent = d2d::brush(rt1, rgba(ACCENT, 1.0)).ok();
            let white = d2d::brush(rt1, rgba(0xFFFFFF, 1.0)).ok();
            if let Some(dim) = &dim {
                match view.frame {
                    Some(fr) => {
                        let f = self.local(fr);
                        for r in [
                            rect_f(0.0, 0.0, full.right, f.top),
                            rect_f(0.0, f.bottom, full.right, full.bottom),
                            rect_f(0.0, f.top, f.left, f.bottom),
                            rect_f(f.right, f.top, full.right, f.bottom),
                        ] {
                            if r.right > r.left && r.bottom > r.top {
                                rt.FillRectangle(&r, dim);
                            }
                        }
                    }
                    None => rt.FillRectangle(&full, dim),
                }
            }
            if let (Some(fr), Some(q)) = (view.frame, self.quick.as_mut()) {
                let lf = Rect::new(fr.left - self.vd.left, fr.top - self.vd.top, fr.right - self.vd.left, fr.bottom - self.vd.top);
                let fresh = matches!(&q.cache, Some((v, r, _)) if *v == q.canvas.version && *r == lf);
                if !fresh {
                    let items = q.canvas.preview();
                    if items.is_empty() {
                        q.cache = None;
                    } else {
                        let pm = arender::render(&q.base, &items, lf, q.canvas.fonts(), s);
                        let fr2 = arender::to_frame(&pm);
                        if let Ok(b) = d2d::bitmap(rt1, fr2.width() as u32, fr2.height() as u32, &fr2.pixels) {
                            q.cache = Some((q.canvas.version, lf, b));
                        }
                    }
                }
                if let Some((_, _, b)) = &q.cache {
                    let d = rect_f(lf.left as f32, lf.top as f32, lf.right as f32, lf.bottom as f32);
                    rt.DrawBitmap(b, Some(&d), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None);
                }
            }
            if let (Some(fr), Some(accent)) = (view.frame, &accent) {
                let f = self.local(fr);
                let wdt = if view.suggestion { (2.0 * s).round() } else { 1.0_f32.max(s.round()) };
                let half = wdt / 2.0;
                rt.DrawRectangle(&rect_f(f.left + half, f.top + half, f.right - half, f.bottom - half), accent, wdt, None);
                if view.editing {
                    if let Some(white) = &white {
                        let hs = (4.0 * s).round();
                        let (cx, cy) = ((f.left + f.right) / 2.0, (f.top + f.bottom) / 2.0);
                        for (x, y) in [
                            (f.left, f.top),
                            (cx, f.top),
                            (f.right, f.top),
                            (f.left, cy),
                            (f.right, cy),
                            (f.left, f.bottom),
                            (cx, f.bottom),
                            (f.right, f.bottom),
                        ] {
                            let r = rect_f(x - hs, y - hs, x + hs, y + hs);
                            rt.FillRectangle(&r, white);
                            rt.DrawRectangle(&r, accent, 1.0, None);
                        }
                    }
                }
                if !view.suggestion {
                    self.draw_size_label(rt1, fonts, f, fr);
                }
            }
            self.draw_panel(rt1, fonts);
            self.draw_quick(rt1);
            if self.loupe && self.quick.is_none() && !self.panel_rect.contains(pos) {
                self.draw_loupe(rt1, &bmp, fonts, pos, view.frame);
            }
            let _ = rt.EndDraw(None, None);
        }
    }

    unsafe fn draw_size_label(&self, rt: &ID2D1RenderTarget, fonts: &Fonts, f: D2D_RECT_F, fr: Rect) {
        let s = self.scale;
        let text = format!("{} \u{00D7} {}", fr.width(), fr.height());
        let h = 22.0 * s;
        let w = (text.chars().count() as f32 * 7.0 + 16.0) * s;
        let mut top = f.top - h - 6.0 * s;
        if top < (self.monitor.top - self.vd.top) as f32 {
            top = f.top + 6.0 * s;
        }
        let r = rect_f(f.left, top, f.left + w, top + h);
        if let (Ok(bg), Ok(fg)) = (d2d::brush(rt, rgba(PANEL_BG, 0.92)), d2d::brush(rt, rgba(0xFFFFFF, 1.0))) {
            unsafe {
                rt.FillRoundedRectangle(&d2d::round_rect(r, 5.0 * s), &bg);
            }
            d2d::draw_text(rt, &fonts.mono, &text, rect_f(r.left + 8.0 * s, r.top + 3.0 * s, r.right, r.bottom), &fg);
        }
    }

    unsafe fn draw_panel(&self, rt: &ID2D1RenderTarget, fonts: &Fonts) {
        let s = self.scale;
        let pr = self.local(self.panel_rect);
        let (Ok(bg), Ok(border), Ok(fg), Ok(dimfg), Ok(sel_bg), Ok(hover_bg), Ok(sep), Ok(on)) = (
            d2d::brush(rt, rgba(PANEL_BG, 0.97)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.10)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.95)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.40)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.14)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.07)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.12)),
            d2d::brush(rt, rgba(ACCENT, 1.0)),
        ) else {
            return;
        };
        let radius = 10.0 * s;
        unsafe {
            rt.FillRoundedRectangle(&d2d::round_rect(pr, radius), &bg);
            rt.DrawRoundedRectangle(&d2d::round_rect(rect_f(pr.left + 0.5, pr.top + 0.5, pr.right - 0.5, pr.bottom - 0.5), radius), &border, 1.0, None);
        }
        let icon_w = 18.0 * s;
        let gap = 6.0 * s;
        let mut prev: Option<u8> = None;
        for (item, r) in &self.panel {
            let lr = self.local(*r);
            if prev.is_some_and(|g| g != item.group()) {
                let x = lr.left - 6.5 * s;
                unsafe {
                    rt.FillRectangle(&rect_f(x, lr.top + 6.0 * s, x + 1.0, lr.bottom - 6.0 * s), &sep);
                }
            }
            prev = Some(item.group());
            let selected = match item {
                PanelItem::Photo => !self.state.video,
                PanelItem::Video => self.state.video,
                _ => false,
            };
            let pressed = self.panel_press == Some(*item);
            let hovered = self.panel_hover == Some(*item);
            let rr = d2d::round_rect(lr, 7.0 * s);
            unsafe {
                if selected || pressed {
                    rt.FillRoundedRectangle(&rr, &sel_bg);
                } else if hovered {
                    rt.FillRoundedRectangle(&rr, &hover_bg);
                }
            }
            match item {
                PanelItem::Sound | PanelItem::Mic => {
                    let active = if *item == PanelItem::Sound { self.state.sound } else { self.state.mic };
                    let glyph = match (item, active) {
                        (PanelItem::Sound, true) => "\u{E767}",
                        (PanelItem::Sound, false) => "\u{E74F}",
                        (_, true) => "\u{E720}",
                        _ => "\u{EC54}",
                    };
                    d2d::draw_text(rt, &fonts.icon, glyph, lr, if active { &on } else { &dimfg });
                    continue;
                }
                PanelItem::Close => {
                    d2d::draw_text(rt, &fonts.icon, "\u{E711}", lr, &fg);
                    continue;
                }
                PanelItem::Fps | PanelItem::Res => {
                    let label = if *item == PanelItem::Fps { &self.state.fps_label } else { &self.state.res_label };
                    let ty = (lr.top + lr.bottom) / 2.0 - 10.0 * s;
                    d2d::draw_text(rt, &fonts.ui, label, rect_f(lr.left + 12.0 * s, ty, lr.right, ty + 22.0 * s), &fg);
                    let cx = lr.right - 18.0 * s;
                    d2d::draw_text(rt, &fonts.small_icon(), "\u{E70D}", rect_f(cx, lr.top, cx + 12.0 * s, lr.bottom), &fg);
                    continue;
                }
                _ => {}
            }
            let (glyph, label) = match item {
                PanelItem::Photo => ("\u{E722}", "Photo"),
                PanelItem::Video => ("\u{E714}", "Video"),
                _ => ("\u{E8B7}", self.folder_label.as_str()),
            };
            let x0 = lr.left + 12.0 * s;
            d2d::draw_text(rt, &fonts.icon, glyph, rect_f(x0, lr.top, x0 + icon_w, lr.bottom), &fg);
            let tx = x0 + icon_w + gap;
            let ty = (lr.top + lr.bottom) / 2.0 - 10.0 * s;
            d2d::draw_text(rt, &fonts.ui, label, rect_f(tx, ty, lr.right, ty + 22.0 * s), &fg);
            if *item == PanelItem::Folder {
                let cx = lr.right - 18.0 * s;
                d2d::draw_text(rt, &fonts.small_icon(), "\u{E70D}", rect_f(cx, lr.top, cx + 12.0 * s, lr.bottom), &fg);
            }
        }
    }

    unsafe fn draw_loupe(&self, rt: &ID2D1RenderTarget, bmp: &ID2D1Bitmap, fonts: &Fonts, pos: Point, frame: Option<Rect>) {
        let s = self.scale;
        const CELLS: i32 = 17;
        let zoom = (8.0 * s).round();
        let box_px = CELLS as f32 * zoom;
        let info_h = 90.0 * s;
        let off = 22.0 * s;
        let cursor = Point::new(pos.x - self.vd.left, pos.y - self.vd.top);
        let mon = self.local(self.monitor);
        let mut x = cursor.x as f32 + off;
        let mut y = cursor.y as f32 + off;
        if x + box_px > mon.right {
            x = cursor.x as f32 - off - box_px;
        }
        if y + box_px + info_h > mon.bottom {
            y = cursor.y as f32 - off - box_px - info_h;
        }
        let half = CELLS / 2;
        let src = rect_f(
            (cursor.x - half) as f32,
            (cursor.y - half) as f32,
            (cursor.x + half + 1) as f32,
            (cursor.y + half + 1) as f32,
        );
        let dst = rect_f(x, y, x + box_px, y + box_px);
        let (Ok(bg), Ok(border), Ok(fg), Ok(sub), Ok(white), Ok(black)) = (
            d2d::brush(rt, rgba(PANEL_BG, 0.96)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.25)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.95)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.6)),
            d2d::brush(rt, rgba(0xFFFFFF, 1.0)),
            d2d::brush(rt, rgba(0x000000, 1.0)),
        ) else {
            return;
        };
        let card = rect_f(dst.left, dst.top, dst.right, dst.bottom + info_h);
        unsafe {
            rt.FillRoundedRectangle(&d2d::round_rect(card, 8.0 * s), &bg);
            rt.PushAxisAlignedClip(&dst, D2D1_ANTIALIAS_MODE_ALIASED);
            rt.FillRectangle(&dst, &black);
            rt.DrawBitmap(bmp, Some(&dst), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, Some(&src));
            let c0 = rect_f(x + half as f32 * zoom, y + half as f32 * zoom, x + (half + 1) as f32 * zoom, y + (half + 1) as f32 * zoom);
            rt.DrawRectangle(&rect_f(c0.left - 1.0, c0.top - 1.0, c0.right + 1.0, c0.bottom + 1.0), &black, 1.0, None);
            rt.DrawRectangle(&c0, &white, 1.0, None);
            rt.PopAxisAlignedClip();
            rt.DrawRoundedRectangle(&d2d::round_rect(card, 8.0 * s), &border, 1.0, None);
            if let Ok(outline) = d2d::brush(rt, rgba(0x000000, 0.55)) {
                let o = rect_f(card.left - 1.0, card.top - 1.0, card.right + 1.0, card.bottom + 1.0);
                rt.DrawRoundedRectangle(&d2d::round_rect(o, 9.0 * s), &outline, 1.0, None);
            }
        }
        let tx = dst.left + 10.0 * s;
        let mut ty = dst.bottom + 6.0 * s;
        let line = 16.0 * s;
        d2d::draw_text(rt, &fonts.mono, &format!("{}, {}", pos.x, pos.y), rect_f(tx, ty, dst.right, ty + line), &sub);
        ty += line;
        if let Some(fr) = frame {
            d2d::draw_text(rt, &fonts.mono, &format!("{} \u{00D7} {}", fr.width(), fr.height()), rect_f(tx, ty, dst.right, ty + line), &sub);
        }
        ty += line;
        if let Some(px) = self.frame.as_ref().and_then(|f| f.pixel(pos.x, pos.y)) {
            let hex = hex_color(px);
            let sw = 11.0 * s;
            let col = (px[2] as u32) << 16 | (px[1] as u32) << 8 | px[0] as u32;
            if let Ok(b) = d2d::brush(rt, rgba(col, 1.0)) {
                let r = rect_f(tx, ty + 2.0 * s, tx + sw, ty + 2.0 * s + sw);
                unsafe {
                    rt.FillRectangle(&r, &b);
                    rt.DrawRectangle(&r, &border, 1.0, None);
                }
            }
            d2d::draw_text(rt, &fonts.mono, &hex, rect_f(tx + sw + 6.0 * s, ty, dst.right, ty + line), &fg);
        }
        ty += line;
        if let Ok(hint) = d2d::brush(rt, rgba(0xFFFFFF, 0.38)) {
            d2d::draw_text(rt, &fonts.small, "Wheel  bigger / smaller", rect_f(tx, ty, dst.right, ty + line), &hint);
            d2d::draw_text(rt, &fonts.small, "C  copy color", rect_f(tx, ty + line, dst.right, ty + 2.0 * line), &hint);
        }
    }
}

impl Fonts {
    fn small_icon(&self) -> IDWriteTextFormat {
        self.icon.clone()
    }
}

fn make_fonts(gfx: &Gfx, s: f32) -> Option<Fonts> {
    let mono = gfx.text_format(&windows::core::HSTRING::from("Consolas"), 12.5 * s, false).ok()?;
    Some(Fonts {
        ui: gfx.ui_font(14.0 * s, false).ok()?,
        small: gfx.ui_font(12.0 * s, false).ok()?,
        mono,
        icon: gfx.icon_font(15.0 * s).ok()?,
    })
}

/// A background process may not be allowed to take focus; this is the
/// standard workaround (attach to the foreground thread for the call).
pub fn force_foreground(hwnd: HWND) {
    unsafe {
        if SetForegroundWindow(hwnd).as_bool() {
            return;
        }
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let me = windows::Win32::System::Threading::GetCurrentThreadId();
        use windows::Win32::System::Threading::AttachThreadInput;
        if fg_thread != 0 && fg_thread != me {
            let _ = AttachThreadInput(me, fg_thread, true);
            let _ = SetForegroundWindow(hwnd);
            let _ = BringWindowToTop(hwnd);
            let _ = AttachThreadInput(me, fg_thread, false);
        }
    }
}

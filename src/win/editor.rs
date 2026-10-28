//! Editor window: toolbar on top, the image on a dark canvas with zoom and
//! pan, all marks editable. Every committed change is reported so the app can
//! copy the result to the clipboard.

use super::class_name;
use super::d2d::{self, Gfx, rect_f};
use super::theme::{self, Backdrop, Blur};
use super::toolstrip::{Btn, Slot, Strip, StripState};
use super::util::lparam_point;
use crate::annot::edit::{Canvas, Change, Handle, Key};
use crate::annot::font::{self, Fonts};
use crate::annot::render::{self, bbox};
use crate::annot::{BlurMode, Color, Item, P, PALETTE, Shape, Tool};
use crate::geom::{Point, Rect};
use crate::image::Frame;
use std::path::PathBuf;
use std::sync::Arc;
use tiny_skia::Pixmap;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Gdi::*;
use windows::core::Interface;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const CARET_TIMER: usize = 1;
const STATUS_TIMER: usize = 2;
const ANIM_TIMER: usize = 3;

pub enum Event {
    /// The image changed: copy it to the clipboard soon.
    Changed,
    Copy,
    Save,
    SaveTo(Point),
    CustomColor,
    /// Tool / colour / thickness changed: remember them.
    Style,
}

pub const SLOTS: [Slot; 32] = [
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
    Slot::B(Btn::Tool(Tool::Crop)),
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
    Slot::B(Btn::Copy),
    Slot::B(Btn::Save),
];

pub struct Editor {
    pub hwnd: HWND,
    rt: Option<ID2D1HwndRenderTarget>,
    rt_size: (u32, u32),
    base: Pixmap,
    pub canvas: Canvas,
    cache: Option<(u64, Option<Rect>, ID2D1Bitmap)>,
    strip: Strip,
    scale: f32,
    zoom: f32,
    /// Client position of the displayed area's top-left.
    origin: (f32, f32),
    fit: bool,
    pan: Option<(Point, (f32, f32))>,
    space: bool,
    dragging: bool,
    caret_on: bool,
    pub saved_path: Option<PathBuf>,
    pub custom: Color,
    status: String,
    status_font: Option<(f32, windows::Win32::Graphics::DirectWrite::IDWriteTextFormat)>,
    last_cursor: Point,
    /// Natural toolbar width: the window never gets narrower than this.
    min_w: i32,
    /// Area shown last frame, to keep the view steady when it changes.
    last_area: Rect,
    /// Offscreen copy of everything under the floating chrome, for the glass blur.
    scene: Option<(u32, u32, ID2D1BitmapRenderTarget)>,
    blur: Blur,
}

fn dpi_scale(h: HWND) -> f32 {
    (unsafe { GetDpiForWindow(h) }.max(96)) as f32 / 96.0
}

impl Editor {
    /// Opens the editor for `base` with existing `items` (in base coordinates).
    pub fn open(instance: HINSTANCE, gfx: &Gfx, base: &Frame, items: Vec<Item>, fonts: Option<Arc<Fonts>>, style: (Tool, Color, usize), near: Rect) -> windows::core::Result<Editor> {
        let (bw, bh) = (base.width() as i32, base.height() as i32);
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name::EDITOR,
                windows::core::w!("Editor - Snap"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                800,
                600,
                None,
                None,
                Some(instance),
                None,
            )?
        };
        let scale = dpi_scale(hwnd);
        theme::apply_window(hwnd);
        let mut canvas = Canvas::new(Rect::new(0, 0, bw, bh), scale, fonts);
        canvas.items = items;
        canvas.tool = if style.0 == Tool::Crop { Tool::Arrow } else { style.0 };
        canvas.color = style.1;
        canvas.width_idx = style.2;
        let mut ed = Editor {
            hwnd,
            rt: None,
            rt_size: (0, 0),
            base: render::base_pixmap(base),
            canvas,
            cache: None,
            strip: Strip::new(),
            scale,
            zoom: 1.0,
            origin: (0.0, 0.0),
            fit: true,
            pan: None,
            space: false,
            dragging: false,
            caret_on: true,
            saved_path: None,
            custom: Color(0xFF, 0x4F, 0xB0),
            status: String::new(),
            status_font: None,
            last_cursor: Point::default(),
            min_w: 0,
            last_area: Rect::new(0, 0, bw, bh),
            scene: None,
            blur: Blur::default(),
        };
        ed.strip.animate = true;
        ed.strip.layout(gfx, &SLOTS, Point::new(0, 0), ed.bar_h(), 0, scale);
        ed.min_w = (ed.strip.rect.width() as f32 * 0.75) as i32 + (ed.margin() * 2.0) as i32;
        // Size: image + chrome, at most 85% of the work area, centred on the capture's monitor.
        unsafe {
            let m = MonitorFromPoint(POINT { x: near.left + near.width() / 2, y: near.top + near.height() / 2 }, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(m, &mut info);
            let work = super::util::to_rect(info.rcWork);
            let bar = ed.chrome_top();
            let mut want = RECT { left: 0, top: 0, right: (bw + (48.0 * scale) as i32).max(ed.min_w), bottom: bh + bar + (48.0 * scale) as i32 };
            let _ = AdjustWindowRectEx(&mut want, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0));
            let mut min_win = RECT { left: 0, top: 0, right: ed.min_w, bottom: 10 };
            let _ = AdjustWindowRectEx(&mut min_win, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0));
            let min_win_w = min_win.right - min_win.left;
            let w = (want.right - want.left).min(work.width() * 85 / 100).max(min_win_w.min(work.width()));
            let h = (want.bottom - want.top).min(work.height() * 85 / 100).max((480.0 * scale) as i32);
            let x = work.left + (work.width() - w) / 2;
            let y = work.top + (work.height() - h) / 2;
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER);
            let _ = ShowWindow(hwnd, SW_SHOW);
            super::overlay::force_foreground(hwnd);
            SetTimer(Some(hwnd), CARET_TIMER, 530, None);
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

    /// Gap between the window edge and the floating chrome.
    fn margin(&self) -> f32 {
        10.0 * self.scale
    }

    /// The image is fitted below this line so the toolbar doesn't cover it.
    fn chrome_top(&self) -> i32 {
        self.bar_h() + (self.margin() * 2.0) as i32
    }

    fn in_bar(&self, p: Point) -> bool {
        self.strip.rect.contains(p)
    }

    /// Lays the toolbar out at its natural width, centred at the top.
    fn place_strip(&mut self, gfx: &Gfx, cw: i32) {
        let m = self.margin().round() as i32;
        self.strip.layout_fit(gfx, &SLOTS, m, m, self.bar_h(), cw - m * 2, self.scale);
        let mut o = POINT::default();
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut o);
        }
        self.strip.publish("editor", |r| r.offset(o.x, o.y));
    }

    pub fn theme_changed(&mut self) {
        theme::apply_window(self.hwnd);
        self.invalidate();
    }

    /// The part of the image shown (crop or all of it).
    fn area(&self) -> Rect {
        self.canvas.crop.unwrap_or(self.canvas.bounds)
    }

    /// Final image: base + marks, cropped.
    pub fn final_frame(&self) -> Frame {
        let p = render::render(&self.base, &self.canvas.items, self.area(), self.canvas.fonts(), self.scale);
        render::to_frame(&p)
    }

    pub fn items(&self) -> Vec<Item> {
        self.canvas.items.clone()
    }

    pub fn set_status(&mut self, s: String) {
        self.status = s;
        unsafe {
            SetTimer(Some(self.hwnd), STATUS_TIMER, 3000, None);
        }
        self.invalidate();
    }

    pub fn set_title(&self, t: &str) {
        let w = super::util::wide(t);
        unsafe {
            let _ = SetWindowTextW(self.hwnd, windows::core::PCWSTR(w.as_ptr()));
        }
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn client(&self) -> (i32, i32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        (r.right - r.left, r.bottom - r.top)
    }

    fn on_size(&mut self, gfx: &Gfx) {
        let (w, h) = self.client();
        if w <= 0 || h <= 0 {
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
                self.cache = None;
            }
            _ => {}
        }
        self.place_strip(gfx, w as i32);
        if self.fit {
            self.fit_view();
        }
        self.invalidate();
    }

    fn fit_view(&mut self) {
        let (cw, ch) = self.client();
        let bar = self.chrome_top() as f32;
        let a = self.area();
        let pad = 24.0 * self.scale;
        let avail_w = (cw as f32 - pad * 2.0).max(50.0);
        let avail_h = (ch as f32 - bar - pad * 2.0).max(50.0);
        self.zoom = (avail_w / a.width() as f32).min(avail_h / a.height() as f32).min(1.0);
        let (dw, dh) = (a.width() as f32 * self.zoom, a.height() as f32 * self.zoom);
        self.origin = (((cw as f32 - dw) / 2.0).round(), (bar + (ch as f32 - bar - dh) / 2.0).round());
    }

    fn to_img(&self, p: Point) -> P {
        let a = self.area();
        P::new(a.left as f32 + (p.x as f32 - self.origin.0) / self.zoom, a.top as f32 + (p.y as f32 - self.origin.1) / self.zoom)
    }

    fn to_screen(&self, p: P) -> (f32, f32) {
        let a = self.area();
        (self.origin.0 + (p.x - a.left as f32) * self.zoom, self.origin.1 + (p.y - a.top as f32) * self.zoom)
    }

    fn zoom_at(&mut self, z: f32, at: Point) {
        let before = self.to_img(at);
        self.zoom = z.clamp(0.05, 8.0);
        let a = self.area();
        self.origin = (at.x as f32 - (before.x - a.left as f32) * self.zoom, at.y as f32 - (before.y - a.top as f32) * self.zoom);
        self.fit = false;
        self.invalidate();
    }

    fn state(&self) -> StripState {
        let c = &self.canvas;
        StripState { tool: c.tool, color: c.color, width_idx: c.width_idx, blur_mode: c.blur_mode, can_undo: c.can_undo(), can_redo: c.can_redo(), custom: self.custom }
    }

    fn apply(&mut self, c: Change) -> Option<Event> {
        match c {
            Change::None => None,
            Change::Redraw => {
                self.invalidate();
                None
            }
            Change::Committed => {
                self.invalidate();
                Some(Event::Changed)
            }
        }
    }

    pub fn set_color(&mut self, c: Color) -> Option<Event> {
        let ch = self.canvas.set_color(c);
        self.invalidate();
        self.apply(ch).or(Some(Event::Style))
    }

    fn button(&mut self, b: Btn, at: Point) -> Option<Event> {
        let c = &mut self.canvas;
        let r = match b {
            Btn::Tool(t) => {
                let ch = c.set_tool(t);
                let _ = ch;
                self.invalidate();
                return Some(Event::Style);
            }
            Btn::Color(i) => return self.set_color(PALETTE[i]),
            Btn::Custom => return Some(Event::CustomColor),
            Btn::Width(i) => {
                let ch = c.set_width(i);
                self.invalidate();
                return self.apply(ch).or(Some(Event::Style));
            }
            Btn::BlurMode => {
                let m = if c.blur_mode == BlurMode::Blur { BlurMode::Pixelate } else { BlurMode::Blur };
                c.set_blur_mode(m)
            }
            Btn::Undo => c.key(Key::Undo),
            Btn::Redo => c.key(Key::Redo),
            Btn::Copy => return Some(Event::Copy),
            Btn::Save => return Some(Event::Save),
            Btn::SaveTo => return Some(Event::SaveTo(at)),
            _ => Change::None,
        };
        self.invalidate();
        self.apply(r)
    }

    pub fn handle(&mut self, gfx: &Gfx, msg: u32, wp: WPARAM, lp: LPARAM) -> (Option<LRESULT>, Option<Event>) {
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
                self.on_size(gfx);
                (Some(LRESULT(0)), None)
            }
            WM_GETMINMAXINFO => {
                let mmi = unsafe { &mut *(lp.0 as *mut MINMAXINFO) };
                let mut r = RECT { left: 0, top: 0, right: self.min_w.max((480.0 * self.scale) as i32), bottom: (360.0 * self.scale) as i32 };
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
            WM_TIMER => {
                if wp.0 == CARET_TIMER {
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
                } else if wp.0 == STATUS_TIMER {
                    unsafe {
                        let _ = KillTimer(Some(self.hwnd), STATUS_TIMER);
                    }
                    self.status.clear();
                    self.invalidate();
                }
                (Some(LRESULT(0)), None)
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
                let hov = if self.in_bar(p) { self.strip.hit(p) } else { None };
                if hov != self.strip.hover {
                    if self.strip.set_hover(hov) {
                        unsafe {
                            SetTimer(Some(self.hwnd), ANIM_TIMER, 15, None);
                        }
                    }
                    self.invalidate();
                }
                if let Some((start, o)) = self.pan {
                    self.origin = (o.0 + (p.x - start.x) as f32, o.1 + (p.y - start.y) as f32);
                    self.fit = false;
                    self.invalidate();
                    return (Some(LRESULT(0)), None);
                }
                if self.dragging {
                    let shift = wp.0 & 0x0004 != 0;
                    let ch = self.canvas.mouse_move(self.to_img(p), shift);
                    return (Some(LRESULT(0)), self.apply(ch));
                }
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONDOWN => {
                let p = lparam_point(lp.0);
                unsafe {
                    SetCapture(self.hwnd);
                    let _ = SetFocus(Some(self.hwnd));
                }
                if self.in_bar(p) {
                    self.strip.press = self.strip.hit(p);
                    self.invalidate();
                    return (Some(LRESULT(0)), None);
                }
                if self.space {
                    self.pan = Some((p, self.origin));
                    return (Some(LRESULT(0)), None);
                }
                self.dragging = true;
                let shift = wp.0 & 0x0004 != 0;
                let ch = self.canvas.mouse_down(self.to_img(p), shift);
                self.caret_on = true;
                (Some(LRESULT(0)), self.apply(ch))
            }
            WM_LBUTTONUP => {
                let p = lparam_point(lp.0);
                unsafe {
                    let _ = ReleaseCapture();
                }
                if let Some(pressed) = self.strip.press.take() {
                    self.invalidate();
                    if self.strip.hit(p) == Some(pressed) {
                        let r = self.strip.items.iter().find(|(b, _)| *b == pressed).map(|(_, r)| *r).unwrap_or_default();
                        let mut pt = POINT { x: r.left, y: r.bottom };
                        unsafe {
                            let _ = ClientToScreen(self.hwnd, &mut pt);
                        }
                        return (Some(LRESULT(0)), self.button(pressed, Point::new(pt.x, pt.y)));
                    }
                    return (Some(LRESULT(0)), None);
                }
                if self.pan.take().is_some() {
                    return (Some(LRESULT(0)), None);
                }
                if self.dragging {
                    self.dragging = false;
                    let shift = wp.0 & 0x0004 != 0;
                    let ch = self.canvas.mouse_up(self.to_img(p), shift);
                    return (Some(LRESULT(0)), self.apply(ch));
                }
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONDBLCLK => {
                let p = lparam_point(lp.0);
                if !self.in_bar(p) {
                    let ch = self.canvas.double_click(self.to_img(p));
                    return (Some(LRESULT(0)), self.apply(ch));
                }
                (Some(LRESULT(0)), None)
            }
            WM_MBUTTONDOWN => {
                unsafe {
                    SetCapture(self.hwnd);
                }
                self.pan = Some((lparam_point(lp.0), self.origin));
                (Some(LRESULT(0)), None)
            }
            WM_MBUTTONUP => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                self.pan = None;
                (Some(LRESULT(0)), None)
            }
            WM_MOUSEWHEEL => {
                let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16;
                let mut pt = POINT { x: lparam_point(lp.0).x, y: lparam_point(lp.0).y };
                unsafe {
                    let _ = ScreenToClient(self.hwnd, &mut pt);
                }
                let ctrl = wp.0 & 0x0008 != 0;
                if ctrl {
                    let f = if delta > 0 { 1.15 } else { 1.0 / 1.15 };
                    self.zoom_at(self.zoom * f, Point::new(pt.x, pt.y));
                } else {
                    let shift = wp.0 & 0x0004 != 0;
                    let d = delta as f32 / 120.0 * 60.0 * self.scale;
                    if shift {
                        self.origin.0 += d;
                    } else {
                        self.origin.1 += d;
                    }
                    self.fit = false;
                    self.invalidate();
                }
                (Some(LRESULT(0)), None)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let vk = VIRTUAL_KEY(wp.0 as u16);
                let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
                let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
                let typing = self.canvas.text.is_some();
                let r = match vk {
                    VK_Z if ctrl => self.canvas.key(Key::Undo),
                    VK_Y if ctrl => self.canvas.key(Key::Redo),
                    VK_C if ctrl && !typing => return (Some(LRESULT(0)), Some(Event::Copy)),
                    VK_S if ctrl && shift => {
                        let r = self.strip.items.iter().find(|(b, _)| *b == Btn::Save).map(|(_, r)| *r).unwrap_or_default();
                        let mut pt = POINT { x: r.left, y: r.bottom };
                        unsafe {
                            let _ = ClientToScreen(self.hwnd, &mut pt);
                        }
                        return (Some(LRESULT(0)), Some(Event::SaveTo(Point::new(pt.x, pt.y))));
                    }
                    VK_S if ctrl => return (Some(LRESULT(0)), Some(Event::Save)),
                    VK_0 | VK_NUMPAD0 if ctrl => {
                        self.fit = true;
                        self.fit_view();
                        self.invalidate();
                        Change::None
                    }
                    VK_1 | VK_NUMPAD1 if ctrl => {
                        let (cw, ch) = self.client();
                        self.zoom_at(1.0, Point::new(cw / 2, ch / 2));
                        Change::None
                    }
                    VK_DELETE => self.canvas.key(Key::Delete),
                    VK_ESCAPE => self.canvas.key(Key::Escape),
                    VK_RETURN => self.canvas.key(Key::Enter),
                    VK_BACK => self.canvas.key(Key::Backspace),
                    VK_SPACE if !typing => {
                        self.space = true;
                        self.set_cursor();
                        Change::None
                    }
                    _ if !typing && !ctrl => {
                        let ch = char::from_u32(vk.0 as u32).unwrap_or('\0');
                        if let Some(t) = crate::annot::Tool::from_key(ch) {
                            let _ = self.canvas.set_tool(t);
                            self.invalidate();
                            return (Some(LRESULT(0)), Some(Event::Style));
                        }
                        Change::None
                    }
                    _ => Change::None,
                };
                self.caret_on = true;
                (Some(LRESULT(0)), self.apply(r))
            }
            WM_KEYUP => {
                if VIRTUAL_KEY(wp.0 as u16) == VK_SPACE {
                    self.space = false;
                    self.set_cursor();
                }
                (Some(LRESULT(0)), None)
            }
            WM_CHAR => {
                if self.canvas.text.is_some() {
                    if let Some(ch) = char::from_u32(wp.0 as u32) {
                        let r = self.canvas.char(ch);
                        self.caret_on = true;
                        return (Some(LRESULT(0)), self.apply(r));
                    }
                }
                (Some(LRESULT(0)), None)
            }
            _ => (None, None),
        }
    }

    fn set_cursor(&self) {
        let p = self.last_cursor;
        let id = if self.in_bar(p) {
            if self.strip.hit(p).is_some() { IDC_HAND } else { IDC_ARROW }
        } else if self.space || self.pan.is_some() {
            IDC_SIZEALL
        } else {
            match self.canvas.tool {
                Tool::Select => {
                    let ip = self.to_img(p);
                    if self.canvas.wants(ip) { IDC_SIZEALL } else { IDC_ARROW }
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

    /// Offscreen target the size of the window; the scene is drawn here so
    /// the glass chrome can blur it.
    fn scene_target(&mut self, rt: &ID2D1HwndRenderTarget) -> Option<ID2D1BitmapRenderTarget> {
        let (w, h) = self.rt_size;
        if let Some((sw, sh, t)) = &self.scene {
            if (*sw, *sh) == (w, h) {
                return Some(t.clone());
            }
        }
        let t = unsafe { rt.CreateCompatibleRenderTarget(None, Some(&D2D_SIZE_U { width: w, height: h }), None, D2D1_COMPATIBLE_RENDER_TARGET_OPTIONS_NONE).ok()? };
        unsafe { t.SetDpi(96.0, 96.0) };
        self.scene = Some((w, h, t.clone()));
        Some(t)
    }

    fn render(&mut self, gfx: &Gfx) {
        let Some(rt) = self.rt.clone() else { return };
        let s = self.scale;
        let p = theme::pal();
        let area = self.area();
        // Crop applied or undone: re-fit, or keep the same pixels in place.
        if area != self.last_area {
            if self.fit {
                self.fit_view();
            } else {
                self.origin.0 += (area.left - self.last_area.left) as f32 * self.zoom;
                self.origin.1 += (area.top - self.last_area.top) as f32 * self.zoom;
            }
            self.last_area = area;
        }
        // Re-render the image only when the marks changed.
        let need = !matches!(&self.cache, Some((v, a, _)) if *v == self.canvas.version && *a == self.canvas.crop);
        if need {
            let pm = render::render(&self.base, &self.canvas.preview(), area, self.canvas.fonts(), s);
            let f = render::to_frame(&pm);
            if let Ok(b) = d2d::bitmap(&rt, f.width() as u32, f.height() as u32, &f.pixels) {
                self.cache = Some((self.canvas.version, self.canvas.crop, b));
            }
        }
        let rt1: &ID2D1RenderTarget = &rt;
        let (cw, chh) = (self.rt_size.0 as f32, self.rt_size.1 as f32);
        let scene = self.scene_target(&rt);
        unsafe {
            // 1. The scene: canvas, image, selection UI.
            let target: &ID2D1RenderTarget = match &scene {
                Some(t) => t,
                None => rt1,
            };
            if scene.is_some() {
                target.BeginDraw();
            } else {
                rt.BeginDraw();
            }
            target.Clear(Some(&theme::color(p.canvas)));
            if let Some((_, _, bmp)) = &self.cache {
                let (x, y) = self.origin;
                let d = rect_f(x, y, x + area.width() as f32 * self.zoom, y + area.height() as f32 * self.zoom);
                let mut o = POINT::default();
                let _ = ClientToScreen(self.hwnd, &mut o);
                super::uimap::put("editor.image", Rect::new(d.left.round() as i32 + o.x, d.top.round() as i32 + o.y, d.right.round() as i32 + o.x, d.bottom.round() as i32 + o.y));
                theme::shadow(target, d, 2.0 * s, s, 0.8);
                let mode = if self.zoom >= 2.0 { D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR } else { D2D1_BITMAP_INTERPOLATION_MODE_LINEAR };
                match target.cast::<ID2D1DeviceContext>() {
                    Ok(dc) if self.zoom < 1.0 => dc.DrawBitmap(bmp, Some(&d), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None),
                    _ => target.DrawBitmap(bmp, Some(&d), 1.0, mode, None),
                }
            }
            self.draw_marks_ui(target);
            // 2. The scene onto the window, then the glass chrome over it.
            let mut blurred = None;
            if let Some(t) = &scene {
                let _ = t.EndDraw(None, None);
                rt.BeginDraw();
                if let Ok(b) = t.GetBitmap() {
                    rt.DrawBitmap(&b, None, 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None);
                    blurred = self.blur.image(rt1, &b, 22.0 * s);
                }
            }
            let back = blurred.as_ref().map(|image| Backdrop { image, origin: (0.0, 0.0), zoom: 1.0 });
            let st = self.state();
            self.strip.draw(rt1, gfx, &st, (0.0, 0.0), Some(back.as_ref()));
            // Status bottom-left, zoom bottom-right: small glass pills.
            if self.status_font.as_ref().map(|f| f.0) != Some(s) {
                if let Ok(f) = gfx.ui_font(12.5 * s, true) {
                    self.status_font = Some((s, f));
                }
            }
            if let (Some((_, f)), Some(b)) = (&self.status_font, theme::brush(rt1, p.label)) {
                let m = self.margin();
                let ph = 30.0 * s;
                let z = format!("{}%", (self.zoom * 100.0).round());
                let zw = gfx.measure(f, &z) + 26.0 * s;
                let zr = rect_f(cw - zw - m, chh - ph - m, cw - m, chh - m);
                theme::glass(rt1, gfx, zr, ph / 2.0, s, back.as_ref());
                d2d::draw_text(rt1, f, &z, rect_f(zr.left + 13.0 * s, zr.top + 6.5 * s, zr.right, zr.bottom), &b);
                if !self.status.is_empty() {
                    let w = gfx.measure(f, &self.status) + 30.0 * s;
                    let r = rect_f(m, chh - ph - m, m + w, chh - m);
                    theme::glass(rt1, gfx, r, ph / 2.0, s, back.as_ref());
                    if let Some(ok) = theme::brush(rt1, p.green) {
                        let e = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: r.left + 13.0 * s, Y: (r.top + r.bottom) / 2.0 }, radiusX: 3.0 * s, radiusY: 3.0 * s };
                        rt.FillEllipse(&e, &ok);
                    }
                    d2d::draw_text(rt1, f, &self.status, rect_f(r.left + 22.0 * s, r.top + 6.5 * s, r.right, r.bottom), &b);
                }
            }
            let _ = rt.EndDraw(None, None);
        }
    }

    /// Selection handles, text box and caret, crop draft.
    unsafe fn draw_marks_ui(&self, rt: &ID2D1RenderTarget) {
        let s = self.scale;
        let p = theme::pal();
        let (Some(acc), Some(white), Some(dim)) = (theme::brush(rt, p.accent), d2d::brush(rt, d2d::rgba(0xFFFFFF, 1.0)).ok(), d2d::brush(rt, d2d::rgba(0x000000, 0.5)).ok()) else { return };
        let c = &self.canvas;
        if let Some(i) = c.selected {
            if let Some(it) = c.items.get(i) {
                let (l, t, r, b) = bbox(it, c.fonts());
                let (x0, y0) = self.to_screen(P::new(l, t));
                let (x1, y1) = self.to_screen(P::new(r, b));
                unsafe { rt.DrawRectangle(&rect_f(x0 - 2.0, y0 - 2.0, x1 + 2.0, y1 + 2.0), &acc, 1.0, None) };
            }
            for (_, hp) in c.handles() {
                let (x, y) = self.to_screen(hp);
                handle_dot(rt, x, y, s, &white, &acc);
            }
        }
        if let Some(t) = &c.text {
            if let Some(f) = c.fonts() {
                let (l, tp, r, b) = font::text_box(&f.text, t.size, &t.text, t.at.x, t.at.y);
                let (x0, y0) = self.to_screen(P::new(l, tp));
                let (x1, y1) = self.to_screen(P::new(r, b));
                unsafe { rt.DrawRoundedRectangle(&theme::round(rect_f(x0 - 5.0, y0 - 4.0, x1 + 5.0, y1 + 4.0), 4.0 * s), &acc, 1.0, None) };
                if self.caret_on {
                    let last = t.text.split('\n').last().unwrap_or("");
                    let lines = t.text.split('\n').count().max(1) as f32;
                    let lh = font::line_height(&f.text, t.size);
                    let cx = t.at.x + font::line_width(&f.text, t.size, last);
                    let cy = t.at.y + lh * (lines - 1.0);
                    let (sx, sy) = self.to_screen(P::new(cx, cy));
                    let (_, ey) = self.to_screen(P::new(cx, cy + lh));
                    unsafe { rt.FillRoundedRectangle(&theme::round(rect_f(sx + 1.0, sy + 2.0, sx + 1.0 + 2.0 * s, ey - 2.0), 1.0 * s), &acc) };
                }
            }
        }
        if let Some((a, b)) = c.crop_draft {
            let r = crate::annot::to_rect(a, b);
            let area = self.area();
            let (ax0, ay0) = self.to_screen(P::new(area.left as f32, area.top as f32));
            let (ax1, ay1) = self.to_screen(P::new(area.right as f32, area.bottom as f32));
            let (x0, y0) = self.to_screen(P::new(r.left as f32, r.top as f32));
            let (x1, y1) = self.to_screen(P::new(r.right as f32, r.bottom as f32));
            unsafe {
                for q in [rect_f(ax0, ay0, ax1, y0), rect_f(ax0, y1, ax1, ay1), rect_f(ax0, y0, x0, y1), rect_f(x1, y0, ax1, y1)] {
                    if q.right > q.left && q.bottom > q.top {
                        rt.FillRectangle(&q, &dim);
                    }
                }
                rt.DrawRectangle(&rect_f(x0 + 0.5, y0 + 0.5, x1 - 0.5, y1 - 0.5), &white, 1.5, None);
            }
        }
        let _ = Handle::A;
        let _ = Shape::Step { c: P::default(), r: 0.0 };
    }
}

/// Apple-style resize handle: a white dot with a soft shadow and an accent ring.
pub fn handle_dot(rt: &ID2D1RenderTarget, x: f32, y: f32, s: f32, white: &ID2D1SolidColorBrush, ring: &ID2D1SolidColorBrush) {
    let r = 5.0 * s;
    unsafe {
        if let Ok(sh) = d2d::brush(rt, d2d::rgba(0x000000, 0.28)) {
            let e = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: x, Y: y + 1.0 * s }, radiusX: r + 1.5 * s, radiusY: r + 1.5 * s };
            rt.FillEllipse(&e, &sh);
        }
        let e = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: x, Y: y }, radiusX: r, radiusY: r };
        rt.FillEllipse(&e, white);
        rt.DrawEllipse(&e, ring, 1.2 * s, None);
    }
}

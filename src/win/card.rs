//! Preview card shown in the corner after a capture (Snipping Tool style):
//! thumbnail, a status line and the action buttons. Never takes focus.

use super::d2d::{self, Gfx, rect_f};
use super::theme::{self, Material};
use super::util::lparam_point;
use super::class_name;
use crate::geom::{Point, Rect};
use crate::image::Frame;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute};
use windows::core::Interface;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;

const HIDE_TIMER: usize = 1;
const SLIDE_TIMER: usize = 2;
const SLIDE_MS: f32 = 200.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Copy,
    Save,
    SaveTo,
    Folder,
    Edit,
    Gif,
    Close,
    Thumb,
}

pub enum Event {
    Clicked(Button, Point),
}

pub struct Card {
    pub hwnd: HWND,
    rt: Option<ID2D1HwndRenderTarget>,
    rt_size: (u32, u32),
    bmp: Option<ID2D1Bitmap>,
    img_size: (f32, f32),
    scale: f32,
    size: (i32, i32),
    thumb: Rect,
    buttons: Vec<(Button, Rect)>,
    hover: Option<Button>,
    press: Option<Button>,
    status: String,
    seconds: u32,
    tracking: bool,
    visible: bool,
    fonts: Option<(f32, IDWriteTextFormat, IDWriteTextFormat, IDWriteTextFormat)>,
    /// DWM draws acrylic behind the card.
    acrylic: bool,
    /// Slide-in: final position and start time.
    slide: Option<(i32, i32, std::time::Instant)>,
    small_label: Option<(f32, IDWriteTextFormat)>,
}

impl Card {
    pub fn create(instance: HINSTANCE) -> windows::core::Result<Card> {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class_name::CARD,
                windows::core::w!("Rengi Snap preview"),
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
        unsafe {
            let round = 2u32; // DWMWCP_ROUND
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const _, 4);
        }
        theme::apply_window(hwnd);
        let acrylic = theme::material(hwnd, Material::Acrylic);
        Ok(Card {
            hwnd,
            rt: None,
            rt_size: (0, 0),
            bmp: None,
            img_size: (0.0, 0.0),
            scale: 1.0,
            size: (0, 0),
            thumb: Rect::default(),
            buttons: Vec::new(),
            hover: None,
            press: None,
            status: String::new(),
            seconds: 6,
            tracking: false,
            visible: false,
            fonts: None,
            acrylic,
            slide: None,
            small_label: None,
        })
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_status(&mut self, text: String) {
        self.status = text;
        self.invalidate();
    }

    /// `anchor`: desktop rect of the capture; the card goes to that monitor's corner.
    pub fn show(&mut self, gfx: &Gfx, frame: &Frame, anchor: Rect, status: String, seconds: u32, video: bool) {
        let (work, scale) = unsafe {
            let c = anchor.left + anchor.width() / 2;
            let m = MonitorFromPoint(POINT { x: c, y: anchor.top + anchor.height() / 2 }, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(m, &mut info);
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            (super::util::to_rect(info.rcWork), dx as f32 / 96.0)
        };
        self.scale = scale;
        self.seconds = seconds;
        self.status = status;
        self.hover = None;
        self.press = None;
        let s = scale;
        let px = |v: f32| (v * s).round() as i32;
        let w = px(340.0);
        let pad = px(12.0);
        let inner_w = w - pad * 2;
        let (iw, ih) = (frame.width().max(1) as f32, frame.height().max(1) as f32);
        let max_h = px(190.0) as f32;
        let fit = (inner_w as f32 / iw).min(max_h / ih).min(1.0 * s.max(1.0));
        let th = ((ih * fit).round() as i32).clamp(px(60.0), px(190.0));
        let status_h = px(26.0);
        let btn_h = px(48.0);
        let h = pad + th + px(4.0) + status_h + btn_h + px(8.0);
        self.size = (w, h);
        self.thumb = Rect::new(pad, pad, w - pad, pad + th);
        self.buttons.clear();
        let row_top = pad + th + px(4.0) + status_h;
        let row: Vec<Button> = if video {
            vec![Button::Copy, Button::SaveTo, Button::Folder, Button::Gif, Button::Edit]
        } else {
            vec![Button::Copy, Button::Save, Button::SaveTo, Button::Folder, Button::Edit]
        };
        let bw = inner_w / row.len() as i32;
        for (i, b) in row.into_iter().enumerate() {
            let x = pad + bw * i as i32;
            self.buttons.push((b, Rect::new(x, row_top, x + bw, row_top + btn_h)));
        }
        let c = px(24.0);
        self.buttons.push((Button::Close, Rect::new(w - pad - c - px(8.0), pad + px(8.0), w - pad - px(8.0), pad + px(8.0) + c)));
        self.buttons.push((Button::Thumb, self.thumb));

        if self.rt.is_none() || self.rt_size != (w as u32, h as u32) {
            self.rt = gfx.target(self.hwnd, w as u32, h as u32).ok();
            self.rt_size = (w as u32, h as u32);
        }
        if let Some(rt) = &self.rt {
            self.bmp = d2d::bitmap(rt, frame.width() as u32, frame.height() as u32, &frame.pixels).ok();
            self.img_size = (iw, ih);
        }
        if self.fonts.as_ref().map(|f| f.0) != Some(s) {
            if let (Ok(a), Ok(b), Ok(c)) = (gfx.ui_font(12.5 * s, false), gfx.icon_font(17.0 * s), gfx.icon_font(10.0 * s)) {
                self.fonts = Some((s, a, b, c));
            }
            if let Ok(l) = gfx.ui_font(11.5 * s, true) {
                self.small_label = Some((s, l));
            }
        }
        let margin = px(16.0);
        let x = work.right - w - margin;
        let y = work.bottom - h - margin;
        // Slide up 12 px into place (ease-out, 200 ms); only on a fresh show.
        let from_y = if self.visible { y } else { y + px(12.0) };
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, from_y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        if from_y != y {
            self.slide = Some((x, y, std::time::Instant::now()));
            unsafe {
                SetTimer(Some(self.hwnd), SLIDE_TIMER, 15, None);
            }
        }
        self.visible = true;
        self.render();
        self.arm_timer();
        self.publish(x, y);
    }

    /// Button rects for the end-to-end test (card at `x, y` on screen).
    fn publish(&self, x: i32, y: i32) {
        super::uimap::clear("card");
        for (b, r) in &self.buttons {
            super::uimap::put(&format!("card.{b:?}"), r.offset(x, y));
        }
    }

    pub fn theme_changed(&mut self) {
        theme::apply_window(self.hwnd);
        self.invalidate();
    }

    pub fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.bmp = None;
        self.slide = None;
        super::uimap::clear("card");
        unsafe {
            let _ = KillTimer(Some(self.hwnd), SLIDE_TIMER);
            let _ = KillTimer(Some(self.hwnd), HIDE_TIMER);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn arm_timer(&self) {
        unsafe {
            if self.seconds > 0 {
                SetTimer(Some(self.hwnd), HIDE_TIMER, self.seconds * 1000, None);
            }
        }
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn button_at(&self, p: Point) -> Option<Button> {
        // Close sits on top of the thumbnail, so it wins.
        self.buttons.iter().find(|(_, r)| r.contains(p)).map(|(b, _)| *b)
    }

    pub fn handle(&mut self, msg: u32, _wp: WPARAM, lp: LPARAM) -> (Option<LRESULT>, Option<Event>) {
        match msg {
            WM_PAINT => {
                self.render();
                unsafe {
                    let _ = ValidateRect(Some(self.hwnd), None);
                }
                (Some(LRESULT(0)), None)
            }
            WM_ERASEBKGND => (Some(LRESULT(1)), None),
            WM_MOUSEACTIVATE => (Some(LRESULT(MA_NOACTIVATE as isize)), None),
            WM_TIMER if _wp.0 == SLIDE_TIMER => {
                if let Some((x, y, t0)) = self.slide {
                    let k = (t0.elapsed().as_secs_f32() * 1000.0 / SLIDE_MS).min(1.0);
                    let e = 1.0 - (1.0 - k).powi(3);
                    let dy = ((1.0 - e) * 12.0 * self.scale).round() as i32;
                    unsafe {
                        let _ = SetWindowPos(self.hwnd, None, x, y + dy, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER);
                        if k >= 1.0 {
                            let _ = KillTimer(Some(self.hwnd), SLIDE_TIMER);
                        }
                    }
                    if k >= 1.0 {
                        self.slide = None;
                    }
                }
                (Some(LRESULT(0)), None)
            }
            WM_TIMER => {
                self.hide();
                (Some(LRESULT(0)), None)
            }
            WM_SETCURSOR => {
                let id = if self.hover.is_some() { IDC_HAND } else { IDC_ARROW };
                unsafe {
                    if let Ok(c) = LoadCursorW(None, id) {
                        SetCursor(Some(c));
                    }
                }
                (Some(LRESULT(1)), None)
            }
            WM_MOUSEMOVE => {
                if !self.tracking {
                    self.tracking = true;
                    let mut t = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: self.hwnd,
                        dwHoverTime: 0,
                    };
                    unsafe {
                        let _ = TrackMouseEvent(&mut t);
                        let _ = KillTimer(Some(self.hwnd), HIDE_TIMER);
                    }
                }
                let h = self.button_at(lparam_point(lp.0));
                if h != self.hover {
                    self.hover = h;
                    self.invalidate();
                }
                (Some(LRESULT(0)), None)
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.hover = None;
                self.press = None;
                self.invalidate();
                self.arm_timer();
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONDOWN => {
                self.press = self.button_at(lparam_point(lp.0));
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONUP => {
                let p = lparam_point(lp.0);
                let pressed = self.press.take();
                self.invalidate();
                match (pressed, self.button_at(p)) {
                    (Some(a), Some(b)) if a == b => {
                        let r = self.buttons.iter().find(|(x, _)| *x == a).map(|(_, r)| *r).unwrap_or_default();
                        let mut origin = POINT::default();
                        unsafe {
                            let _ = ClientToScreen(self.hwnd, &mut origin);
                        }
                        let anchor = Point::new(origin.x + r.left, origin.y + r.bottom);
                        (Some(LRESULT(0)), Some(Event::Clicked(a, anchor)))
                    }
                    _ => (Some(LRESULT(0)), None),
                }
            }
            _ => (None, None),
        }
    }

    fn f(r: Rect) -> D2D_RECT_F {
        rect_f(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32)
    }

    fn render(&mut self) {
        let Some(rt) = self.rt.clone() else { return };
        let Some((_, ui, icon, small_icon)) = self.fonts.clone() else { return };
        let Some((_, small)) = self.small_label.clone() else { return };
        let s = self.scale;
        let p = theme::pal();
        let rt1: &ID2D1RenderTarget = &rt;
        let (Some(fg), Some(sub), Some(hover), Some(press), Some(well), Some(accent), Some(close_bg), Some(white), Some(hair)) = (
            theme::brush(rt1, p.label),
            theme::brush(rt1, p.label2),
            theme::brush(rt1, p.hover),
            theme::brush(rt1, p.selected),
            theme::brush(rt1, theme::with_alpha(p.hover, 1.2)),
            theme::brush(rt1, p.accent),
            d2d::brush(rt1, d2d::rgba(0x000000, 0.50)).ok(),
            d2d::brush(rt1, d2d::rgba(0xFFFFFF, 0.95)).ok(),
            theme::brush(rt1, p.hairline),
        ) else {
            return;
        };
        let (w, h) = (self.rt_size.0 as f32, self.rt_size.1 as f32);
        unsafe {
            rt.BeginDraw();
            if self.acrylic {
                rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                rt.Clear(Some(&d2d::rgba(0, 0.0)));
                if let Some(t) = theme::brush(rt1, p.glass) {
                    rt.FillRectangle(&rect_f(0.0, 0.0, w, h), &t);
                }
            } else {
                rt.Clear(Some(&theme::color(p.glass_solid.solid())));
            }
            theme::rim(rt1, rect_f(0.0, 0.0, w, h), 8.0 * s, s);
            // Thumbnail with rounded corners, letterboxed on a soft well.
            let t = Self::f(self.thumb);
            let radius = 10.0 * s;
            rt.FillRoundedRectangle(&theme::round(t, radius), &well);
            if let Some(bmp) = &self.bmp {
                let (iw, ih) = self.img_size;
                let fit = ((t.right - t.left) / iw).min((t.bottom - t.top) / ih).min(1.0 * s.max(1.0));
                let (dw, dh) = (iw * fit, ih * fit);
                let x = (t.left + t.right - dw) / 2.0;
                let y = (t.top + t.bottom - dh) / 2.0;
                let dst = rect_f(x.round(), y.round(), (x + dw).round(), (y + dh).round());
                let geo = rt.GetFactory().ok().and_then(|f| f.CreateRoundedRectangleGeometry(&theme::round(t, radius)).ok());
                let dc = d2d::as_device_context(&rt);
                let layered = match (&dc, &geo) {
                    (Some(dc), Some(geo)) => {
                        let params = D2D1_LAYER_PARAMETERS1 {
                            contentBounds: t,
                            geometricMask: std::mem::ManuallyDrop::new(geo.cast::<ID2D1Geometry>().ok()),
                            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                            maskTransform: windows_numerics::Matrix3x2::identity(),
                            opacity: 1.0,
                            opacityBrush: std::mem::ManuallyDrop::new(None),
                            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                        };
                        dc.PushLayer(&params, None);
                        let _ = std::mem::ManuallyDrop::into_inner(params.geometricMask);
                        true
                    }
                    _ => false,
                };
                match &dc {
                    Some(dc) => dc.DrawBitmap(bmp, Some(&dst), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None),
                    None => rt.DrawBitmap(bmp, Some(&dst), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
                }
                if layered {
                    if let Some(dc) = &dc {
                        dc.PopLayer();
                    }
                }
                rt.DrawRoundedRectangle(&theme::round(theme::inset(t, 0.5), radius), &hair, 1.0, None);
                if self.hover == Some(Button::Thumb) {
                    rt.DrawRoundedRectangle(&theme::round(theme::inset(t, 1.0), radius), &accent, 2.0 * s, None);
                }
            }
            // Round close button over the thumbnail (Quick Look style).
            if let Some((_, r)) = self.buttons.iter().find(|(b, _)| *b == Button::Close) {
                let c = Self::f(*r);
                let e = D2D1_ELLIPSE {
                    point: windows_numerics::Vector2 { X: (c.left + c.right) / 2.0, Y: (c.top + c.bottom) / 2.0 },
                    radiusX: (c.right - c.left) / 2.0,
                    radiusY: (c.bottom - c.top) / 2.0,
                };
                rt.FillEllipse(&e, &close_bg);
                if self.hover == Some(Button::Close) {
                    rt.FillEllipse(&e, &close_bg);
                }
                d2d::draw_text(rt1, &small_icon, "\u{E711}", c, &white);
            }
            // Status line.
            let sy = self.thumb.bottom as f32 + 6.0 * s;
            let status = rect_f(self.thumb.left as f32 + 4.0 * s, sy + 2.0 * s, self.thumb.right as f32, sy + 22.0 * s);
            d2d::draw_text(rt1, &ui, &self.status, status, &sub);
            // Buttons: icon over a small label.
            small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
            for (b, r) in &self.buttons {
                let (glyph, label) = match b {
                    Button::Copy => ("\u{E8C8}", "Copy"),
                    Button::Save => ("\u{E74E}", "Save"),
                    Button::SaveTo => ("\u{E792}", "Save to"),
                    Button::Folder => ("\u{E838}", "Folder"),
                    Button::Edit => ("\u{E70F}", "Edit"),
                    Button::Gif => ("\u{E8B9}", "GIF"),
                    _ => continue,
                };
                let lr = Self::f(*r);
                let cell = rect_f(lr.left + 3.0 * s, lr.top + 2.0 * s, lr.right - 3.0 * s, lr.bottom - 2.0 * s);
                if self.press == Some(*b) {
                    rt.FillRoundedRectangle(&theme::round(theme::inset(cell, 1.0 * s), 10.0 * s), &press);
                } else if self.hover == Some(*b) {
                    rt.FillRoundedRectangle(&theme::round(cell, 10.0 * s), &hover);
                }
                let ink = if *b == Button::Copy { &accent } else { &fg };
                let icon_r = rect_f(cell.left, cell.top + 3.0 * s, cell.right, cell.top + 24.0 * s);
                d2d::draw_text(rt1, &icon, glyph, icon_r, ink);
                let lab = rect_f(cell.left, cell.top + 26.0 * s, cell.right, cell.bottom);
                d2d::draw_text(rt1, &small, label, lab, &sub);
            }
            small.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING).ok();
            let _ = rt.EndDraw(None, None);
        }
    }
}

//! Recording chrome: an accent outline around the region and a small control
//! bar. Both are excluded from screen capture, so they never show up in the
//! video even when they sit inside the recorded area.

use super::class_name;
use super::d2d::{self, Gfx, rect_f};
use super::theme::{self, Material};
use super::util::{lparam_point, to_win_rect};
use crate::geom::{Point, Rect};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::{DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;

const TICK: usize = 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarButton {
    Pause,
    Stop,
    Mic,
    Discard,
}

pub struct RecBar {
    pub hwnd: HWND,
    outline: HWND,
    rt: Option<ID2D1HwndRenderTarget>,
    rt_size: (u32, u32),
    fonts: Option<(f32, IDWriteTextFormat, IDWriteTextFormat)>,
    scale: f32,
    buttons: Vec<(BarButton, Rect)>,
    hover: Option<BarButton>,
    press: Option<BarButton>,
    pub show_mic: bool,
    pub paused: bool,
    pub muted: bool,
    pub seconds: f64,
    visible: bool,
    acrylic: bool,
}

fn exclude_from_capture(h: HWND) {
    // Test hook: lets the e2e film the bar to review its look.
    if std::env::var_os("RENGI_SNAP_BAR_VISIBLE").is_some() {
        return;
    }
    unsafe {
        let _ = SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE);
    }
}

impl RecBar {
    pub fn create(instance: HINSTANCE) -> windows::core::Result<RecBar> {
        let (hwnd, outline) = unsafe {
            let bar = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class_name::BAR,
                windows::core::w!("Snap recording"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )?;
            let outline = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                class_name::OUTLINE,
                windows::core::w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )?;
            let round = 2u32;
            let _ = DwmSetWindowAttribute(bar, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const _, 4);
            (bar, outline)
        };
        theme::apply_window(hwnd);
        let acrylic = theme::material(hwnd, Material::Acrylic);
        exclude_from_capture(hwnd);
        exclude_from_capture(outline);
        Ok(RecBar {
            hwnd,
            outline,
            rt: None,
            rt_size: (0, 0),
            fonts: None,
            scale: 1.0,
            buttons: Vec::new(),
            hover: None,
            press: None,
            show_mic: false,
            paused: false,
            muted: false,
            seconds: 0.0,
            visible: false,
            acrylic,
        })
    }

    pub fn show(&mut self, gfx: &Gfx, region: Rect, show_mic: bool) {
        let (mon, scale) = unsafe {
            let m = MonitorFromPoint(POINT { x: region.left + region.width() / 2, y: region.top + region.height() / 2 }, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(m, &mut info);
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            (super::util::to_rect(info.rcMonitor), dx as f32 / 96.0)
        };
        self.scale = scale;
        self.show_mic = show_mic;
        self.paused = false;
        self.muted = false;
        self.seconds = 0.0;
        self.hover = None;
        self.press = None;
        let px = |v: f32| (v * scale).round() as i32;
        let btn = px(36.0);
        let pad = px(7.0);
        let timer_w = px(96.0);
        let n = if show_mic { 4 } else { 3 };
        let w = pad + timer_w + n * btn + (n - 1) * px(2.0) + pad + px(4.0);
        let h = px(50.0);
        self.buttons.clear();
        let mut x = pad + timer_w;
        let mut order = vec![BarButton::Pause, BarButton::Stop];
        if show_mic {
            order.push(BarButton::Mic);
        }
        order.push(BarButton::Discard);
        for b in order {
            self.buttons.push((b, Rect::new(x, pad, x + btn, h - pad)));
            x += btn + px(2.0);
        }
        // Below the region, else above, else inside the bottom edge.
        let gap = px(10.0);
        let bx = (region.left + (region.width() - w) / 2).clamp(mon.left + gap, mon.right - w - gap);
        let by = if region.bottom + gap + h <= mon.bottom {
            region.bottom + gap
        } else if region.top - gap - h >= mon.top {
            region.top - gap - h
        } else {
            region.bottom - h - px(16.0)
        };
        if self.rt.is_none() || self.rt_size != (w as u32, h as u32) {
            self.rt = gfx.target(self.hwnd, w as u32, h as u32).ok();
            self.rt_size = (w as u32, h as u32);
        }
        if self.fonts.as_ref().map(|f| f.0) != Some(scale) {
            if let (Ok(a), Ok(b)) = (gfx.ui_font(15.0 * scale, true), gfx.icon_font(14.0 * scale)) {
                self.fonts = Some((scale, a, b));
            }
        }
        // Outline: a 2 px frame just outside the region, via a window region.
        let t = px(2.0).max(2);
        let outer = Rect::new(region.left - t, region.top - t, region.right + t, region.bottom + t);
        unsafe {
            let o = CreateRectRgn(0, 0, outer.width(), outer.height());
            let i = CreateRectRgn(t, t, t + region.width(), t + region.height());
            let _ = CombineRgn(Some(o), Some(o), Some(i), RGN_DIFF);
            let _ = DeleteObject(i.into());
            SetWindowRgn(self.outline, Some(o), false);
            let _ = SetWindowPos(self.outline, Some(HWND_TOPMOST), outer.left, outer.top, outer.width(), outer.height(), SWP_NOACTIVATE | SWP_SHOWWINDOW);
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), bx, by, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            SetTimer(Some(self.hwnd), TICK, 250, None);
        }
        let _ = to_win_rect;
        self.visible = true;
        self.render();
        super::uimap::clear("bar");
        for (b, r) in &self.buttons {
            super::uimap::put(&format!("bar.{b:?}"), r.offset(bx, by));
        }
    }

    pub fn theme_changed(&mut self) {
        theme::apply_window(self.hwnd);
        self.invalidate();
        unsafe {
            let _ = InvalidateRect(Some(self.outline), None, true);
        }
    }

    pub fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TICK);
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            let _ = ShowWindow(self.outline, SW_HIDE);
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn button_at(&self, p: Point) -> Option<BarButton> {
        self.buttons.iter().find(|(_, r)| r.contains(p)).map(|(b, _)| *b)
    }

    /// Outline window procedure: paints the accent frame.
    pub fn outline_proc(hwnd: HWND, msg: u32) -> Option<LRESULT> {
        if msg == WM_PAINT {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let a = theme::pal().accent.0;
                let b = CreateSolidBrush(COLORREF(((a & 0xFF) << 16) | (a & 0xFF00) | ((a >> 16) & 0xFF)));
                let mut r = RECT::default();
                let _ = GetClientRect(hwnd, &mut r);
                FillRect(dc, &r, b);
                let _ = DeleteObject(b.into());
                let _ = EndPaint(hwnd, &ps);
            }
            return Some(LRESULT(0));
        }
        if msg == WM_NCHITTEST {
            return Some(LRESULT(HTTRANSPARENT as isize));
        }
        None
    }

    /// Returns the handled result and a clicked button.
    pub fn handle(&mut self, msg: u32, _wp: WPARAM, lp: LPARAM) -> (Option<LRESULT>, Option<BarButton>, bool) {
        match msg {
            WM_PAINT => {
                self.render();
                unsafe {
                    let _ = ValidateRect(Some(self.hwnd), None);
                }
                (Some(LRESULT(0)), None, false)
            }
            WM_ERASEBKGND => (Some(LRESULT(1)), None, false),
            WM_MOUSEACTIVATE => (Some(LRESULT(MA_NOACTIVATE as isize)), None, false),
            WM_TIMER => (Some(LRESULT(0)), None, true),
            WM_SETCURSOR => {
                let id = if self.hover.is_some() { IDC_HAND } else { IDC_ARROW };
                unsafe {
                    if let Ok(c) = LoadCursorW(None, id) {
                        SetCursor(Some(c));
                    }
                }
                (Some(LRESULT(1)), None, false)
            }
            WM_MOUSEMOVE => {
                let h = self.button_at(lparam_point(lp.0));
                if h != self.hover {
                    self.hover = h;
                    self.invalidate();
                }
                (Some(LRESULT(0)), None, false)
            }
            WM_LBUTTONDOWN => {
                self.press = self.button_at(lparam_point(lp.0));
                self.invalidate();
                (Some(LRESULT(0)), None, false)
            }
            WM_LBUTTONUP => {
                let p = lparam_point(lp.0);
                let pressed = self.press.take();
                self.invalidate();
                match (pressed, self.button_at(p)) {
                    (Some(a), Some(b)) if a == b => (Some(LRESULT(0)), Some(a), false),
                    _ => (Some(LRESULT(0)), None, false),
                }
            }
            _ => (None, None, false),
        }
    }

    fn render(&mut self) {
        let (Some(rt), Some((_, ui, icon))) = (self.rt.clone(), self.fonts.clone()) else { return };
        let s = self.scale;
        let p = theme::pal();
        let rt1: &ID2D1RenderTarget = &rt;
        let (Some(fg), Some(hover), Some(press), Some(red), Some(dim), Some(sep)) = (
            theme::brush(rt1, p.label),
            theme::brush(rt1, p.hover),
            theme::brush(rt1, p.selected),
            theme::brush(rt1, p.red),
            theme::brush(rt1, p.label2),
            theme::brush(rt1, p.separator),
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
            // Red dot (a ring while paused) and the timer in fixed-width digits.
            let cx = 21.0 * s;
            let e = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: cx, Y: h / 2.0 }, radiusX: 5.5 * s, radiusY: 5.5 * s };
            if self.paused {
                rt.DrawEllipse(&e, &red, 1.6 * s, None);
            } else {
                if let Some(glow) = theme::brush(rt1, theme::with_alpha(p.red, 0.25)) {
                    let g = D2D1_ELLIPSE { radiusX: 9.0 * s, radiusY: 9.0 * s, ..e };
                    rt.FillEllipse(&g, &glow);
                }
                rt.FillEllipse(&e, &red);
            }
            let secs = self.seconds.max(0.0) as u64;
            let text = format!("{}:{:02}", secs / 60, secs % 60);
            let cell = 9.6 * s;
            let mut x = 36.0 * s;
            for ch in text.chars() {
                let cw = if ch == ':' { cell * 0.55 } else { cell };
                ui.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                d2d::draw_text(rt1, &ui, &ch.to_string(), rect_f(x, h / 2.0 - 11.0 * s, x + cw, h / 2.0 + 12.0 * s), if self.paused { &dim } else { &fg });
                x += cw;
            }
            ui.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING).ok();
            if let Some((_, r)) = self.buttons.first() {
                let sx = r.left as f32 - 6.0 * s;
                rt.FillRectangle(&rect_f(sx, h * 0.28, sx + 1.0, h * 0.72), &sep);
            }
            for (b, r) in &self.buttons {
                let lr = rect_f(r.left as f32, r.top as f32, r.right as f32, r.bottom as f32);
                let e = D2D1_ELLIPSE {
                    point: windows_numerics::Vector2 { X: (lr.left + lr.right) / 2.0, Y: (lr.top + lr.bottom) / 2.0 },
                    radiusX: (lr.right - lr.left) / 2.0 - if self.press == Some(*b) { 1.0 * s } else { 0.0 },
                    radiusY: (lr.bottom - lr.top) / 2.0 - if self.press == Some(*b) { 1.0 * s } else { 0.0 },
                };
                if self.press == Some(*b) {
                    rt.FillEllipse(&e, &press);
                } else if self.hover == Some(*b) {
                    rt.FillEllipse(&e, &hover);
                }
                let (glyph, brush) = match b {
                    BarButton::Pause => (if self.paused { "\u{E768}" } else { "\u{E769}" }, &fg),
                    BarButton::Stop => ("\u{E71A}", &red),
                    BarButton::Mic => (if self.muted { "\u{EC54}" } else { "\u{E720}" }, if self.muted { &dim } else { &fg }),
                    BarButton::Discard => ("\u{E74D}", &dim),
                };
                d2d::draw_text(rt1, &icon, glyph, lr, brush);
            }
            let _ = rt.EndDraw(None, None);
        }
    }
}

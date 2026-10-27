//! Preview card shown in the corner after a capture (Snipping Tool style):
//! thumbnail, a status line and the action buttons. Never takes focus.

use super::d2d::{self, Gfx, rect_f, rgba};
use super::overlay::ACCENT;
use super::util::lparam_point;
use super::class_name;
use crate::geom::{Point, Rect};
use crate::image::Frame;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;

const HIDE_TIMER: usize = 1;
const BG: u32 = 0x202022;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Button {
    Copy,
    Save,
    SaveTo,
    Folder,
    Edit,
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
            let border = 0x00403C3Au32; // COLORREF 0x00BBGGRR
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border as *const _ as *const _, 4);
        }
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
    pub fn show(&mut self, gfx: &Gfx, frame: &Frame, anchor: Rect, status: String, seconds: u32) {
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
        let pad = px(10.0);
        let inner_w = w - pad * 2;
        let (iw, ih) = (frame.width().max(1) as f32, frame.height().max(1) as f32);
        let max_h = px(190.0) as f32;
        let fit = (inner_w as f32 / iw).min(max_h / ih).min(1.0 * s.max(1.0));
        let th = ((ih * fit).round() as i32).clamp(px(60.0), px(190.0));
        let status_h = px(22.0);
        let btn_h = px(38.0);
        let h = pad + th + px(4.0) + status_h + btn_h + pad;
        self.size = (w, h);
        self.thumb = Rect::new(pad, pad, w - pad, pad + th);
        self.buttons.clear();
        let row_top = pad + th + px(4.0) + status_h;
        let bw = inner_w / 5;
        for (i, b) in [Button::Copy, Button::Save, Button::SaveTo, Button::Folder, Button::Edit].into_iter().enumerate() {
            let x = pad + bw * i as i32;
            self.buttons.push((b, Rect::new(x, row_top, x + bw, row_top + btn_h)));
        }
        let c = px(26.0);
        self.buttons.push((Button::Close, Rect::new(w - pad - c - px(6.0), pad + px(6.0), w - pad - px(6.0), pad + px(6.0) + c)));
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
            if let (Ok(a), Ok(b), Ok(c)) = (gfx.ui_font(12.0 * s, false), gfx.icon_font(16.0 * s), gfx.icon_font(12.0 * s)) {
                self.fonts = Some((s, a, b, c));
            }
        }
        let margin = px(16.0);
        let x = work.right - w - margin;
        let y = work.bottom - h - margin;
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        self.visible = true;
        self.render();
        self.arm_timer();
    }

    pub fn hide(&mut self) {
        if !self.visible {
            return;
        }
        self.visible = false;
        self.bmp = None;
        unsafe {
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
            WM_TIMER => {
                self.hide();
                (Some(LRESULT(0)), None)
            }
            WM_SETCURSOR => {
                let id = if self.hover.is_some() && self.hover != Some(Button::Edit) { IDC_HAND } else { IDC_ARROW };
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
                    (Some(a), Some(b)) if a == b && a != Button::Edit => {
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
        let s = self.scale;
        let rt1: &ID2D1RenderTarget = &rt;
        let (Ok(fg), Ok(sub), Ok(dis), Ok(hover), Ok(press), Ok(well), Ok(accent), Ok(close_bg)) = (
            d2d::brush(rt1, rgba(0xFFFFFF, 0.95)),
            d2d::brush(rt1, rgba(0xFFFFFF, 0.62)),
            d2d::brush(rt1, rgba(0xFFFFFF, 0.30)),
            d2d::brush(rt1, rgba(0xFFFFFF, 0.08)),
            d2d::brush(rt1, rgba(0xFFFFFF, 0.14)),
            d2d::brush(rt1, rgba(0x0F0F10, 1.0)),
            d2d::brush(rt1, rgba(ACCENT, 1.0)),
            d2d::brush(rt1, rgba(0x000000, 0.55)),
        ) else {
            return;
        };
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&rgba(BG, 1.0)));
            // Thumbnail, letterboxed on a dark well.
            let t = Self::f(self.thumb);
            rt.FillRoundedRectangle(&d2d::round_rect(t, 6.0 * s), &well);
            if let Some(bmp) = &self.bmp {
                let (iw, ih) = self.img_size;
                let fit = ((t.right - t.left) / iw).min((t.bottom - t.top) / ih).min(1.0 * s.max(1.0));
                let (dw, dh) = (iw * fit, ih * fit);
                let x = (t.left + t.right - dw) / 2.0;
                let y = (t.top + t.bottom - dh) / 2.0;
                let dst = rect_f(x.round(), y.round(), (x + dw).round(), (y + dh).round());
                match d2d::as_device_context(&rt) {
                    Some(dc) => dc.DrawBitmap(bmp, Some(&dst), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None),
                    None => rt.DrawBitmap(bmp, Some(&dst), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
                }
                if self.hover == Some(Button::Thumb) {
                    rt.DrawRectangle(&rect_f(dst.left + 0.5, dst.top + 0.5, dst.right - 0.5, dst.bottom - 0.5), &accent, 1.0, None);
                }
            }
            // Close button over the thumbnail.
            if let Some((_, r)) = self.buttons.iter().find(|(b, _)| *b == Button::Close) {
                let c = Self::f(*r);
                let e = D2D1_ELLIPSE {
                    point: windows_numerics::Vector2 { X: (c.left + c.right) / 2.0, Y: (c.top + c.bottom) / 2.0 },
                    radiusX: (c.right - c.left) / 2.0,
                    radiusY: (c.bottom - c.top) / 2.0,
                };
                rt.FillEllipse(&e, if self.hover == Some(Button::Close) { &press } else { &close_bg });
                d2d::draw_text(rt1, &small_icon, "\u{E711}", c, &fg);
            }
            // Status line.
            let sy = self.thumb.bottom as f32 + 4.0 * s;
            let status = rect_f(self.thumb.left as f32 + 2.0 * s, sy + 2.0 * s, self.thumb.right as f32, sy + 22.0 * s);
            d2d::draw_text(rt1, &ui, &self.status, status, &sub);
            // Buttons.
            for (b, r) in &self.buttons {
                let (glyph, label) = match b {
                    Button::Copy => ("\u{E8C8}", "Copy"),
                    Button::Save => ("\u{E74E}", "Save"),
                    Button::SaveTo => ("\u{E792}", "Save to"),
                    Button::Folder => ("\u{E838}", "Folder"),
                    Button::Edit => ("\u{E70F}", "Edit"),
                    _ => continue,
                };
                let lr = Self::f(*r);
                let inset = rect_f(lr.left + 2.0 * s, lr.top + 2.0 * s, lr.right - 2.0 * s, lr.bottom - 2.0 * s);
                let disabled = *b == Button::Edit;
                if !disabled && self.press == Some(*b) {
                    rt.FillRoundedRectangle(&d2d::round_rect(inset, 6.0 * s), &press);
                } else if !disabled && self.hover == Some(*b) {
                    rt.FillRoundedRectangle(&d2d::round_rect(inset, 6.0 * s), &hover);
                }
                let color = if disabled { &dis } else { &fg };
                let icon_r = rect_f(inset.left, inset.top + 1.0 * s, inset.right, inset.top + 19.0 * s);
                d2d::draw_text(rt1, &icon, glyph, icon_r, color);
                let lw = 60.0 * s;
                let cx = (inset.left + inset.right) / 2.0;
                let lab = rect_f(cx - lw / 2.0, inset.top + 19.0 * s, cx + lw / 2.0, inset.bottom);
                ui.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER).ok();
                d2d::draw_text(rt1, &ui, label, lab, if disabled { &dis } else { &sub });
                ui.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING).ok();
            }
            let _ = rt.EndDraw(None, None);
        }
    }
}

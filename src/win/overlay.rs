//! Full-desktop overlay: frozen frame, dimming, selection, loupe and the top
//! mini panel. One window spans the whole virtual desktop; client pixels map
//! 1:1 to desktop pixels (offset by the desktop origin).

use super::d2d::{self, Gfx, rect_f, rgba};
use super::util::lparam_point;
use super::{WM_APP_KEY, class_name};
use crate::geom::{Point, Rect};
use crate::image::{Frame, hex_color};
use crate::select::{Key, Outcome, Selection};
use std::rc::Rc;
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
    Folder,
    Close,
}

pub enum Event {
    Confirm(Rect),
    Cancel,
    CopyColor(String),
    /// Open the folder dropdown at this desktop point.
    FolderMenu(Point),
}

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
    frame: Option<Rc<Frame>>,
    vd: Rect,
    sel: Option<Selection>,
    /// Monitor (desktop coords) where the panel sits, and its UI scale.
    monitor: Rect,
    scale: f32,
    fonts: Option<(f32, Fonts)>,
    loupe: bool,
    folder_label: String,
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
            vd: Rect::default(),
            sel: None,
            monitor: Rect::default(),
            scale: 1.0,
            fonts: None,
            loupe: true,
            folder_label: String::new(),
            panel: Vec::new(),
            panel_rect: Rect::default(),
            panel_hover: None,
            panel_press: None,
        })
    }

    pub fn frame(&self) -> Option<Rc<Frame>> {
        self.frame.clone()
    }

    pub fn is_active(&self) -> bool {
        self.sel.is_some()
    }

    pub fn set_folder_label(&mut self, label: String, gfx: &Gfx) {
        self.folder_label = label;
        self.layout_panel(gfx);
        self.invalidate();
    }

    pub fn show(&mut self, gfx: &Gfx, frame: Rc<Frame>, candidates: Vec<Rect>, loupe: bool, folder_label: String) -> bool {
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
        let tol = (6.0 * scale).round() as i32;
        let mut sel = Selection::new(vd, candidates, tol);
        sel.mouse_move(cursor);
        self.sel = Some(sel);
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

    pub fn hide(&mut self) {
        self.sel = None;
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
        let seg = |label: &str| -> i32 { (12.0 * s) as i32 * 2 + icon_w + gap + gfx.measure(&f.ui, label).ceil() as i32 };
        let folder_w = (12.0 * s) as i32 * 2 + icon_w + gap * 2 + gfx.measure(&f.ui, &self.folder_label).ceil() as i32 + (12.0 * s) as i32;
        let close_w = h - pad * 2;
        let sep = (13.0 * s).round() as i32;
        let widths = [
            (PanelItem::Photo, seg("Photo")),
            (PanelItem::Video, seg("Video")),
            (PanelItem::Folder, folder_w),
            (PanelItem::Close, close_w),
        ];
        let total: i32 = pad * 2 + widths.iter().map(|w| w.1).sum::<i32>() + sep * 2;
        let left = self.monitor.left + (self.monitor.width() - total) / 2;
        let top = self.monitor.top + (16.0 * s).round() as i32;
        self.panel_rect = Rect::new(left, top, left + total, top + h);
        let mut x = left + pad;
        for (i, (item, w)) in widths.iter().enumerate() {
            if i == 2 || i == 3 {
                x += sep;
            }
            self.panel.push((*item, Rect::new(x, top + pad, x + w, top + h - pad)));
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

    fn panel_click(&mut self, item: PanelItem) -> Option<Event> {
        match item {
            PanelItem::Photo | PanelItem::Video => None,
            PanelItem::Close => Some(Event::Cancel),
            PanelItem::Folder => {
                let r = self.panel.iter().find(|(i, _)| *i == PanelItem::Folder).map(|(_, r)| *r)?;
                Some(Event::FolderMenu(Point::new(r.left, self.panel_rect.bottom + (4.0 * self.scale) as i32)))
            }
        }
    }

    fn set_cursor(&self) {
        let mut cur = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut cur);
        }
        let p = Point::new(cur.x, cur.y);
        let id = if self.panel_rect.contains(p) {
            if matches!(self.panel_item_at(p), Some(PanelItem::Video) | None) { IDC_ARROW } else { IDC_HAND }
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
        let Some((_, fonts)) = &self.fonts else { return };
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
            self.draw_video_hint(rt1, fonts);
            if self.loupe && !self.panel_rect.contains(pos) {
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
        let (Ok(bg), Ok(border), Ok(fg), Ok(dimfg), Ok(sel_bg), Ok(hover_bg), Ok(sep)) = (
            d2d::brush(rt, rgba(PANEL_BG, 0.97)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.10)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.95)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.35)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.14)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.07)),
            d2d::brush(rt, rgba(0xFFFFFF, 0.12)),
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
        for (i, (item, r)) in self.panel.iter().enumerate() {
            let lr = self.local(*r);
            if i == 2 || i == 3 {
                let x = lr.left - 6.5 * s;
                unsafe {
                    rt.FillRectangle(&rect_f(x, lr.top + 6.0 * s, x + 1.0, lr.bottom - 6.0 * s), &sep);
                }
            }
            let disabled = *item == PanelItem::Video;
            let selected = *item == PanelItem::Photo;
            let pressed = self.panel_press == Some(*item);
            let hovered = self.panel_hover == Some(*item) && !disabled;
            let rr = d2d::round_rect(lr, 7.0 * s);
            unsafe {
                if selected || pressed {
                    rt.FillRoundedRectangle(&rr, &sel_bg);
                } else if hovered {
                    rt.FillRoundedRectangle(&rr, &hover_bg);
                }
            }
            let color = if disabled { &dimfg } else { &fg };
            let (glyph, label) = match item {
                PanelItem::Photo => ("\u{E722}", "Photo"),
                PanelItem::Video => ("\u{E714}", "Video"),
                PanelItem::Folder => ("\u{E8B7}", self.folder_label.as_str()),
                PanelItem::Close => ("\u{E711}", ""),
            };
            if label.is_empty() {
                d2d::draw_text(rt, &fonts.icon, glyph, lr, color);
                continue;
            }
            let x0 = lr.left + 12.0 * s;
            d2d::draw_text(rt, &fonts.icon, glyph, rect_f(x0, lr.top, x0 + icon_w, lr.bottom), color);
            let tx = x0 + icon_w + gap;
            let ty = (lr.top + lr.bottom) / 2.0 - 10.0 * s;
            d2d::draw_text(rt, &fonts.ui, label, rect_f(tx, ty, lr.right, ty + 22.0 * s), color);
            if *item == PanelItem::Folder {
                let cx = lr.right - 18.0 * s;
                d2d::draw_text(rt, &fonts.small_icon(), "\u{E70D}", rect_f(cx, lr.top, cx + 12.0 * s, lr.bottom), color);
            }
        }
    }

    unsafe fn draw_video_hint(&self, rt: &ID2D1RenderTarget, fonts: &Fonts) {
        if self.panel_hover != Some(PanelItem::Video) {
            return;
        }
        let Some((_, r)) = self.panel.iter().find(|(i, _)| *i == PanelItem::Video) else { return };
        let s = self.scale;
        let text = "Video recording comes in the next update";
        let w = 262.0 * s;
        let lr = self.local(*r);
        let pr = self.local(self.panel_rect);
        let x = ((lr.left + lr.right) / 2.0 - w / 2.0).max(pr.left);
        let top = pr.bottom + 6.0 * s;
        let box_ = rect_f(x, top, x + w, top + 28.0 * s);
        if let (Ok(bg), Ok(fg)) = (d2d::brush(rt, rgba(PANEL_BG, 0.97)), d2d::brush(rt, rgba(0xFFFFFF, 0.85))) {
            unsafe {
                rt.FillRoundedRectangle(&d2d::round_rect(box_, 6.0 * s), &bg);
            }
            d2d::draw_text(rt, &fonts.small, text, rect_f(box_.left + 10.0 * s, box_.top + 5.0 * s, box_.right, box_.bottom), &fg);
        }
    }

    unsafe fn draw_loupe(&self, rt: &ID2D1RenderTarget, bmp: &ID2D1Bitmap, fonts: &Fonts, pos: Point, frame: Option<Rect>) {
        let s = self.scale;
        const CELLS: i32 = 17;
        let zoom = (8.0 * s).round();
        let box_px = CELLS as f32 * zoom;
        let info_h = 74.0 * s;
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
            d2d::draw_text(rt, &fonts.small, "Press C to copy", rect_f(tx, ty, dst.right, ty + line), &hint);
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

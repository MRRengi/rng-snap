//! Home: the window that explains the app. Three big actions with their
//! keys, the latest captures, and the few settings that matter. Closing it
//! only hides it; the app keeps running in the tray.

use super::class_name;
use super::d2d::{self, Gfx, rect_f};
use super::theme::{self, Fader, Material};
use super::util::lparam_point;
use crate::geom::{Point, Rect};
use crate::image::Frame;
use crate::palette::Rgba;
use crate::recent::{self, Entry, Kind};
use crate::vedit::{self, Cam};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::DwmDefWindowProc;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::Interface;

/// A thumbnail finished loading.
pub const WM_HOME_THUMB: u32 = WM_APP + 50;
const ANIM_TIMER: usize = 1;
const RECENT_MAX: usize = 8;

pub enum Event {
    Capture,
    Record,
    EditVideo,
    Open(PathBuf),
    Reveal(PathBuf),
    OpenFolder,
    Shortcut(Point),
    Folder(Point),
    Autostart,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    Tile(u8),
    Recent(usize),
    OpenFolder,
    Shortcut,
    Folder,
    Autostart,
}

impl Hit {
    fn key(self) -> u64 {
        match self {
            Hit::Tile(i) => 10 + i as u64,
            Hit::Recent(i) => 100 + i as u64,
            Hit::OpenFolder => 1,
            Hit::Shortcut => 2,
            Hit::Folder => 3,
            Hit::Autostart => 4,
        }
    }

    fn name(self) -> String {
        match self {
            Hit::Tile(0) => "screenshot".into(),
            Hit::Tile(1) => "record".into(),
            Hit::Tile(_) => "editvideo".into(),
            Hit::Recent(i) => format!("recent{i}"),
            Hit::OpenFolder => "openfolder".into(),
            Hit::Shortcut => "shortcut".into(),
            Hit::Folder => "folder".into(),
            Hit::Autostart => "autostart".into(),
        }
    }
}

/// What Home shows about the settings.
#[derive(Clone, Default)]
pub struct Info {
    pub hotkey: String,
    pub folder: PathBuf,
    pub folder_label: String,
    pub autostart: bool,
}

struct Fonts {
    title: IDWriteTextFormat,
    h2: IDWriteTextFormat,
    body: IDWriteTextFormat,
    body_b: IDWriteTextFormat,
    small: IDWriteTextFormat,
    cap: IDWriteTextFormat,
    badge: IDWriteTextFormat,
    glyph: IDWriteTextFormat,
}

/// Layout in client pixels.
#[derive(Default, Clone)]
struct Lay {
    caption: f32,
    header: D2D_RECT_F,
    tiles: [D2D_RECT_F; 3],
    recent_title: D2D_RECT_F,
    open_folder: D2D_RECT_F,
    cells: Vec<D2D_RECT_F>,
    thumb_h: f32,
    empty: D2D_RECT_F,
    rows: [D2D_RECT_F; 3],
    switch: D2D_RECT_F,
    footer: D2D_RECT_F,
}

type Thumbs = Arc<Mutex<HashMap<PathBuf, Option<Frame>>>>;

pub struct Home {
    pub hwnd: HWND,
    rt: Option<ID2D1HwndRenderTarget>,
    rt_size: (u32, u32),
    scale: f32,
    /// Mica behind the window and content drawn into the title bar area.
    unified: bool,
    info: Info,
    recent: Vec<Entry>,
    thumbs: Thumbs,
    thumb_bmps: HashMap<PathBuf, ID2D1Bitmap>,
    icon: Option<(u32, ID2D1Bitmap)>,
    lay: Lay,
    hover: Option<Hit>,
    press: Option<Hit>,
    fader: Fader,
    fonts: Option<(f32, Fonts)>,
    tracking: bool,
    /// Switch knob position 0..1 and when it started moving.
    knob: (f32, Instant),
    visible: bool,
}

fn dpi_scale(h: HWND) -> f32 {
    (unsafe { GetDpiForWindow(h) }.max(96)) as f32 / 96.0
}

fn inside(r: &D2D_RECT_F, p: Point) -> bool {
    (p.x as f32) >= r.left && (p.x as f32) < r.right && (p.y as f32) >= r.top && (p.y as f32) < r.bottom
}

static UNIFIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Title-bar handling that needs no app state, so it also works while the
/// app is busy (frame changes arrive synchronously from inside calls).
pub fn frame_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if !UNIFIED.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    let mut res = LRESULT(0);
    if unsafe { DwmDefWindowProc(hwnd, msg, wp, lp, &mut res) }.as_bool() {
        return Some(res);
    }
    match msg {
        WM_NCCALCSIZE if wp.0 != 0 => {
            // Keep the side and bottom frame, give the title bar to the client.
            let params = unsafe { &mut *(lp.0 as *mut NCCALCSIZE_PARAMS) };
            let top = params.rgrc[0].top;
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
            params.rgrc[0].top = top;
            Some(LRESULT(0))
        }
        WM_NCHITTEST => {
            let r = unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
            if r.0 as u32 != HTCLIENT {
                return Some(r);
            }
            let mut pt = POINT { x: lparam_point(lp.0).x, y: lparam_point(lp.0).y };
            unsafe {
                let _ = ScreenToClient(hwnd, &mut pt);
            }
            let caption = 32.0 * dpi_scale(hwnd);
            Some(LRESULT(if (pt.y as f32) < caption { HTCAPTION } else { HTCLIENT } as isize))
        }
        _ => None,
    }
}

const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0);

impl Home {
    pub fn create(instance: HINSTANCE) -> windows::core::Result<Home> {
        let hwnd = unsafe {
            CreateWindowExW(WS_EX_APPWINDOW, class_name::HOME, windows::core::w!("Snap"), STYLE, CW_USEDEFAULT, CW_USEDEFAULT, 820, 720, None, None, Some(instance), None)?
        };
        theme::apply_window(hwnd);
        let unified = theme::material(hwnd, Material::Mica);
        UNIFIED.store(unified, std::sync::atomic::Ordering::Release);
        if unified {
            // Re-run WM_NCCALCSIZE so the client covers the title bar.
            unsafe {
                let _ = SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        Ok(Home {
            hwnd,
            rt: None,
            rt_size: (0, 0),
            scale: dpi_scale(hwnd),
            unified,
            info: Info::default(),
            recent: Vec::new(),
            thumbs: Arc::new(Mutex::new(HashMap::new())),
            thumb_bmps: HashMap::new(),
            icon: None,
            lay: Lay::default(),
            hover: None,
            press: None,
            fader: Fader::default(),
            fonts: None,
            tracking: false,
            knob: (0.0, Instant::now()),
            visible: false,
        })
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Shows Home centred on the monitor under the cursor.
    pub fn show(&mut self, gfx: &Gfx, info: Info) {
        self.knob = (if info.autostart { 1.0 } else { 0.0 }, Instant::now() - std::time::Duration::from_secs(1));
        self.info = info;
        self.refresh_recent();
        if !self.visible {
            self.scale = dpi_scale(self.hwnd);
            let s = self.scale;
            unsafe {
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                let m = MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST);
                let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
                let _ = GetMonitorInfoW(m, &mut mi);
                let work = super::util::to_rect(mi.rcWork);
                let w = ((820.0 * s) as i32).min(work.width() - (16.0 * s) as i32);
                let h = ((760.0 * s) as i32).min(work.height() - (16.0 * s) as i32);
                let _ = SetWindowPos(self.hwnd, None, work.left + (work.width() - w) / 2, work.top + (work.height() - h) / 2, w, h, SWP_NOZORDER);
            }
        }
        unsafe {
            let _ = ShowWindow(self.hwnd, if IsIconic(self.hwnd).as_bool() { SW_RESTORE } else { SW_SHOW });
        }
        super::overlay::force_foreground(self.hwnd);
        self.visible = true;
        self.on_size(gfx);
    }

    pub fn hide(&mut self) {
        self.visible = false;
        super::uimap::clear("home");
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    pub fn set_info(&mut self, info: Info) {
        if info.autostart != self.info.autostart {
            self.knob = (if info.autostart { 0.0 } else { 1.0 }, Instant::now());
            unsafe {
                SetTimer(Some(self.hwnd), ANIM_TIMER, 15, None);
            }
        }
        let folder_changed = info.folder != self.info.folder;
        self.info = info;
        if folder_changed {
            self.refresh_recent();
        }
        self.invalidate();
    }

    pub fn theme_changed(&mut self) {
        theme::apply_window(self.hwnd);
        self.invalidate();
    }

    /// Re-reads the save folder and loads missing thumbnails in the background.
    pub fn refresh_recent(&mut self) {
        self.recent = recent::list(&self.info.folder, RECENT_MAX);
        let want: Vec<Entry> = {
            let have = self.thumbs.lock().map(|g| g.keys().cloned().collect::<Vec<_>>()).unwrap_or_default();
            self.recent.iter().filter(|e| !have.contains(&e.path)).cloned().collect()
        };
        if !want.is_empty() {
            let thumbs = self.thumbs.clone();
            let hw = self.hwnd.0 as isize;
            let px = (360.0 * self.scale) as u32;
            std::thread::spawn(move || {
                unsafe {
                    let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
                }
                for e in want {
                    let f = load_thumb(&e, px);
                    if let Ok(mut g) = thumbs.lock() {
                        g.insert(e.path.clone(), f);
                    }
                    unsafe {
                        let _ = PostMessageW(Some(HWND(hw as *mut _)), WM_HOME_THUMB, WPARAM(0), LPARAM(0));
                    }
                }
            });
        }
        self.layout();
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
                self.thumb_bmps.clear();
                self.icon = None;
            }
            _ => {}
        }
        let s = self.scale;
        if self.fonts.as_ref().map(|f| f.0) != Some(s) {
            let f = (|| {
                Some(Fonts {
                    title: gfx.display_font(26.0 * s, true).ok()?,
                    h2: gfx.ui_font(15.0 * s, true).ok()?,
                    body: gfx.ui_font(13.0 * s, false).ok()?,
                    body_b: gfx.ui_font(13.0 * s, true).ok()?,
                    small: gfx.ui_font(12.0 * s, false).ok()?,
                    cap: gfx.ui_font(11.5 * s, true).ok()?,
                    badge: gfx.icon_font(20.0 * s).ok()?,
                    glyph: gfx.icon_font(11.0 * s).ok()?,
                })
            })();
            self.fonts = f.map(|f| (s, f));
        }
        self.layout();
        self.invalidate();
    }

    fn caption_h(&self) -> f32 {
        if self.unified { 32.0 * self.scale } else { 0.0 }
    }

    fn layout(&mut self) {
        let s = self.scale;
        let (cw, ch) = self.client();
        let pad = 28.0 * s;
        let mut l = Lay { caption: self.caption_h(), ..Default::default() };
        let top = l.caption.max(14.0 * s);
        l.header = rect_f(pad, top, cw - pad, top + 52.0 * s);
        let ty = l.header.bottom + 18.0 * s;
        let gap = 14.0 * s;
        let tw = (cw - pad * 2.0 - gap * 2.0) / 3.0;
        for i in 0..3 {
            let x = pad + (tw + gap) * i as f32;
            l.tiles[i] = rect_f(x, ty, x + tw, ty + 128.0 * s);
        }
        // Settings and footer anchor to the bottom; Recent takes what's left.
        l.footer = rect_f(pad, ch - 34.0 * s, cw - pad, ch - 12.0 * s);
        let row_h = 46.0 * s;
        let rows_top = l.footer.top - 8.0 * s - row_h * 3.0;
        for i in 0..3 {
            l.rows[i] = rect_f(pad, rows_top + row_h * i as f32, cw - pad, rows_top + row_h * (i + 1) as f32);
        }
        let sw = (42.0 * s, 26.0 * s);
        let r2 = l.rows[2];
        l.switch = rect_f(r2.right - 16.0 * s - sw.0, (r2.top + r2.bottom - sw.1) / 2.0, r2.right - 16.0 * s, (r2.top + r2.bottom + sw.1) / 2.0);
        let rt_top = l.tiles[0].bottom + 26.0 * s;
        l.recent_title = rect_f(pad, rt_top, cw - pad, rt_top + 24.0 * s);
        l.open_folder = rect_f(cw - pad - 110.0 * s, rt_top, cw - pad, rt_top + 24.0 * s);
        let grid_top = l.recent_title.bottom + 10.0 * s;
        let grid_bottom = rows_top - 22.0 * s;
        let cols = 4;
        let cgap = 12.0 * s;
        let cell_w = (cw - pad * 2.0 - cgap * (cols - 1) as f32) / cols as f32;
        let text_h = 36.0 * s;
        let avail = (grid_bottom - grid_top).max(0.0);
        let min_thumb = cell_w * 0.56;
        let mut rows = if avail >= (min_thumb + text_h) * 2.0 + cgap && self.recent.len() > cols { 2 } else { 1 };
        // Thumbnails grow into the room there is (up to 4:3), so the grid
        // never leaves a hole above the settings.
        let mut thumb_h = ((avail - cgap * (rows - 1) as f32) / rows as f32 - text_h).min(cell_w * 0.75);
        if thumb_h < 40.0 * s {
            thumb_h = (avail - text_h).max(40.0 * s);
            rows = 1;
        }
        l.thumb_h = thumb_h;
        let n = (rows * cols).min(self.recent.len());
        for i in 0..n {
            let (c, r) = (i % cols, i / cols);
            let x = pad + (cell_w + cgap) * c as f32;
            let y = grid_top + (thumb_h + text_h + cgap) * r as f32;
            l.cells.push(rect_f(x, y, x + cell_w, y + thumb_h + text_h));
        }
        l.empty = rect_f(pad, grid_top, cw - pad, grid_top + thumb_h.min(avail));
        self.lay = l;
        self.publish();
    }

    fn hits(&self) -> Vec<(Hit, D2D_RECT_F)> {
        let l = &self.lay;
        let mut v: Vec<(Hit, D2D_RECT_F)> = (0..3).map(|i| (Hit::Tile(i as u8), l.tiles[i])).collect();
        for (i, c) in l.cells.iter().enumerate() {
            v.push((Hit::Recent(i), *c));
        }
        v.push((Hit::OpenFolder, l.open_folder));
        v.push((Hit::Shortcut, l.rows[0]));
        v.push((Hit::Folder, l.rows[1]));
        v.push((Hit::Autostart, l.rows[2]));
        v
    }

    fn hit(&self, p: Point) -> Option<Hit> {
        self.hits().into_iter().find(|(_, r)| inside(r, p)).map(|(h, _)| h)
    }

    fn publish(&self) {
        if !self.visible {
            return;
        }
        let mut o = POINT::default();
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut o);
        }
        super::uimap::clear("home");
        for (h, r) in self.hits() {
            super::uimap::put(&format!("home.{}", h.name()), Rect::new(r.left as i32 + o.x, r.top as i32 + o.y, r.right as i32 + o.x, r.bottom as i32 + o.y));
        }
    }

    fn screen_point(&self, x: f32, y: f32) -> Point {
        let mut pt = POINT { x: x as i32, y: y as i32 };
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut pt);
        }
        Point::new(pt.x, pt.y)
    }

    fn click(&self, h: Hit) -> Option<Event> {
        let l = &self.lay;
        match h {
            Hit::Tile(0) => Some(Event::Capture),
            Hit::Tile(1) => Some(Event::Record),
            Hit::Tile(_) => Some(Event::EditVideo),
            Hit::Recent(i) => self.recent.get(i).map(|e| Event::Open(e.path.clone())),
            Hit::OpenFolder => Some(Event::OpenFolder),
            Hit::Shortcut => Some(Event::Shortcut(self.screen_point(l.rows[0].right - 200.0 * self.scale, l.rows[0].bottom))),
            Hit::Folder => Some(Event::Folder(self.screen_point(l.rows[1].right - 260.0 * self.scale, l.rows[1].bottom))),
            Hit::Autostart => Some(Event::Autostart),
        }
    }

    pub fn handle(&mut self, gfx: &Gfx, msg: u32, wp: WPARAM, lp: LPARAM) -> (Option<LRESULT>, Option<Event>) {
        match msg {
            WM_CLOSE => {
                self.hide();
                (Some(LRESULT(0)), None)
            }
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
            WM_ACTIVATE => {
                if (wp.0 & 0xFFFF) != 0 {
                    // Back to Home: show what was captured meanwhile.
                    self.refresh_recent();
                }
                (None, None)
            }
            WM_PAINT => {
                self.render(gfx);
                unsafe {
                    let _ = ValidateRect(Some(self.hwnd), None);
                }
                (Some(LRESULT(0)), None)
            }
            WM_ERASEBKGND => (Some(LRESULT(1)), None),
            m if m == WM_HOME_THUMB => {
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            WM_TIMER if wp.0 == ANIM_TIMER => {
                let moving = self.fader.step();
                let knob = self.knob.1.elapsed().as_secs_f32() * 1000.0 < 220.0;
                if !moving && !knob {
                    unsafe {
                        let _ = KillTimer(Some(self.hwnd), ANIM_TIMER);
                    }
                }
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            WM_SETCURSOR => {
                if (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                    let id = if self.hover.is_some() { IDC_HAND } else { IDC_ARROW };
                    unsafe {
                        if let Ok(c) = LoadCursorW(None, id) {
                            SetCursor(Some(c));
                        }
                    }
                    return (Some(LRESULT(1)), None);
                }
                (None, None)
            }
            WM_MOUSEMOVE => {
                if !self.tracking {
                    self.tracking = true;
                    let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                    unsafe {
                        let _ = TrackMouseEvent(&mut t);
                    }
                }
                let h = self.hit(lparam_point(lp.0));
                if h != self.hover {
                    self.hover = h;
                    self.fader.set(h.map(Hit::key));
                    unsafe {
                        SetTimer(Some(self.hwnd), ANIM_TIMER, 15, None);
                    }
                    self.invalidate();
                }
                (Some(LRESULT(0)), None)
            }
            WM_MOUSELEAVE => {
                self.tracking = false;
                self.hover = None;
                self.press = None;
                self.fader.set(None);
                unsafe {
                    SetTimer(Some(self.hwnd), ANIM_TIMER, 15, None);
                }
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONDOWN => {
                self.press = self.hit(lparam_point(lp.0));
                unsafe {
                    SetCapture(self.hwnd);
                }
                self.invalidate();
                (Some(LRESULT(0)), None)
            }
            WM_LBUTTONUP => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                let p = lparam_point(lp.0);
                let pressed = self.press.take();
                self.invalidate();
                match (pressed, self.hit(p)) {
                    (Some(a), Some(b)) if a == b => (Some(LRESULT(0)), self.click(a)),
                    _ => (Some(LRESULT(0)), None),
                }
            }
            WM_RBUTTONUP => {
                if let Some(Hit::Recent(i)) = self.hit(lparam_point(lp.0)) {
                    if let Some(e) = self.recent.get(i) {
                        return (Some(LRESULT(0)), Some(Event::Reveal(e.path.clone())));
                    }
                }
                (Some(LRESULT(0)), None)
            }
            WM_KEYDOWN => {
                if wp.0 as u16 == windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE.0 {
                    self.hide();
                }
                (Some(LRESULT(0)), None)
            }
            _ => (None, None),
        }
    }

    fn hover_amount(&self, h: Hit) -> f32 {
        self.fader.get(h.key())
    }

    fn render(&mut self, _gfx: &Gfx) {
        let Some(rt) = self.rt.clone() else { return };
        let Some((_, f)) = &self.fonts else { return };
        let f = Fonts {
            title: f.title.clone(),
            h2: f.h2.clone(),
            body: f.body.clone(),
            body_b: f.body_b.clone(),
            small: f.small.clone(),
            cap: f.cap.clone(),
            badge: f.badge.clone(),
            glyph: f.glyph.clone(),
        };
        let s = self.scale;
        let p = theme::pal();
        let l = self.lay.clone();
        let rt1: &ID2D1RenderTarget = &rt;
        let Some(b) = Brushes::new(rt1) else { return };
        unsafe {
            rt.BeginDraw();
            if self.unified {
                rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
                rt.Clear(Some(&d2d::rgba(0, 0.0)));
            } else {
                rt.Clear(Some(&theme::color(p.window)));
            }
            self.draw_header(rt1, &f, &b, &l);
            self.draw_tiles(rt1, &f, &b, &l);
            self.draw_recent(rt1, &f, &b, &l);
            self.draw_settings(rt1, &f, &b, &l);
            align(&f.small, true);
            d2d::draw_text(rt1, &f.small, "Snap keeps running in the tray. Close this window anytime.", l.footer, &b.label3);
            align(&f.small, false);
            let _ = rt.EndDraw(None, None);
        }
        let _ = s;
    }

    unsafe fn draw_header(&mut self, rt: &ID2D1RenderTarget, f: &Fonts, b: &Brushes, l: &Lay) {
        let s = self.scale;
        let n = (44.0 * s).round() as u32;
        if self.icon.as_ref().map(|i| i.0) != Some(n) {
            let px = crate::icon::pixels(n as usize);
            self.icon = d2d::bitmap_premul(rt, n, n, &px).ok().map(|b| (n, b));
        }
        let h = l.header;
        let iy = (h.top + h.bottom - n as f32) / 2.0;
        if let Some((_, bmp)) = &self.icon {
            theme::shadow(rt, rect_f(h.left + 2.0 * s, iy + 2.0 * s, h.left + n as f32 - 2.0 * s, iy + n as f32 - 2.0 * s), 10.0 * s, s * 0.5, 0.6);
            unsafe { rt.DrawBitmap(bmp, Some(&rect_f(h.left, iy, h.left + n as f32, iy + n as f32)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
        }
        let tx = h.left + n as f32 + 14.0 * s;
        let title = "Snap";
        let gfx_w = measure(&f.title, title);
        d2d::draw_text(rt, &f.title, title, rect_f(tx, h.top + 6.0 * s, tx + gfx_w + 4.0, h.bottom), &b.label);
        let v = concat!("v", env!("CARGO_PKG_VERSION"));
        let vw = measure(&f.small, v);
        d2d::draw_text(rt, &f.small, v, rect_f(h.right - vw - 2.0, h.top + 21.0 * s, h.right, h.bottom), &b.label3);
    }

    unsafe fn draw_tiles(&self, rt: &ID2D1RenderTarget, f: &Fonts, b: &Brushes, l: &Lay) {
        let s = self.scale;
        let p = theme::pal();
        let tiles: [(&str, &str, &str, Rgba, &str); 3] = [
            ("\u{E722}", "Screenshot", "Area, window or a whole screen", p.accent, &self.info.hotkey),
            ("\u{E714}", "Record video", "Screen with sound, up to 120 fps", p.red, &self.info.hotkey),
            ("\u{E8B3}", "Edit a video", "Cut, reorder, add text and zooms", p.purple, ""),
        ];
        for (i, (glyph, title, sub, color, key)) in tiles.iter().enumerate() {
            let h = Hit::Tile(i as u8);
            let pressed = self.press == Some(h);
            let r = if pressed { theme::inset(l.tiles[i], 1.5 * s) } else { l.tiles[i] };
            let radius = 16.0 * s;
            card(rt, r, radius, s, self.hover_amount(h), b);
            // Coloured badge with a white symbol.
            let bs = 38.0 * s;
            let br = rect_f(r.left + 16.0 * s, r.top + 16.0 * s, r.left + 16.0 * s + bs, r.top + 16.0 * s + bs);
            if let Some(cb) = theme::brush(rt, *color) {
                unsafe { rt.FillRoundedRectangle(&theme::round(br, 10.0 * s), &cb) };
            }
            if let Some(sheen) = theme::brush(rt, Rgba(0xFFFFFF, 0.16)) {
                unsafe { rt.FillRoundedRectangle(&theme::round(rect_f(br.left, br.top, br.right, (br.top + br.bottom) / 2.0), 10.0 * s), &sheen) };
            }
            d2d::draw_text(rt, &f.badge, glyph, br, &b.white);
            d2d::draw_text(rt, &f.h2, title, rect_f(r.left + 16.0 * s, br.bottom + 12.0 * s, r.right - 8.0 * s, br.bottom + 34.0 * s), &b.label);
            d2d::draw_text(rt, &f.small, sub, rect_f(r.left + 16.0 * s, br.bottom + 33.0 * s, r.right - 8.0 * s, br.bottom + 52.0 * s), &b.label2);
            // The key, top-right.
            if !key.is_empty() {
                let kw = keycap_w(&f.cap, key, s);
                let kx = r.right - 14.0 * s - kw;
                let mut right = kx;
                if i == 1 {
                    // Record: the same key, then Video on the panel.
                    let then = "then Video";
                    let tw = measure(&f.small, then);
                    let y = r.top + 17.0 * s;
                    d2d::draw_text(rt, &f.small, then, rect_f(r.right - 14.0 * s - tw, y + 21.0 * s, r.right, y + 40.0 * s), &b.label3);
                    right = kx;
                }
                keycap(rt, &f.cap, key, right, r.top + 16.0 * s, s, b);
            } else {
                let t = "Pick an MP4";
                let tw = measure(&f.small, t);
                d2d::draw_text(rt, &f.small, t, rect_f(r.right - 14.0 * s - tw, r.top + 19.0 * s, r.right, r.top + 40.0 * s), &b.label3);
            }
        }
    }

    unsafe fn draw_recent(&mut self, rt: &ID2D1RenderTarget, f: &Fonts, b: &Brushes, l: &Lay) {
        let s = self.scale;
        d2d::draw_text(rt, &f.h2, "Recent", l.recent_title, &b.label);
        let of = "Open folder";
        let ow = measure(&f.body, of);
        let hov = self.hover_amount(Hit::OpenFolder);
        let link = if hov > 0.5 { &b.accent_hi } else { &b.accent };
        d2d::draw_text(rt, &f.body, of, rect_f(l.open_folder.right - ow - 16.0 * s, l.open_folder.top + 3.0 * s, l.open_folder.right, l.open_folder.bottom), link);
        d2d::draw_text(rt, &f.glyph, "\u{E76C}", rect_f(l.open_folder.right - 12.0 * s, l.open_folder.top + 1.0 * s, l.open_folder.right, l.open_folder.bottom), link);
        if self.recent.is_empty() {
            let r = l.empty;
            if r.bottom - r.top > 20.0 * s {
                if let Some(dash) = dashed(rt) {
                    unsafe { rt.DrawRoundedRectangle(&theme::round(theme::inset(r, 0.5), 14.0 * s), &b.label3, 1.2 * s, &dash) };
                }
                align(&f.body_b, true);
                align(&f.small, true);
                let cy = (r.top + r.bottom) / 2.0;
                d2d::draw_text(rt, &f.body_b, "Your captures will show up here", rect_f(r.left, cy - 20.0 * s, r.right, cy), &b.label2);
                let hint = format!("Press {} to take the first one", self.info.hotkey);
                d2d::draw_text(rt, &f.small, &hint, rect_f(r.left, cy + 2.0 * s, r.right, cy + 22.0 * s), &b.label3);
                align(&f.body_b, false);
                align(&f.small, false);
            }
            return;
        }
        let thumbs: HashMap<PathBuf, Option<Frame>> = self.thumbs.lock().map(|g| g.clone()).unwrap_or_default();
        let now = SystemTime::now();
        for (i, cell) in l.cells.iter().enumerate() {
            let Some(e) = self.recent.get(i).cloned() else { break };
            let h = Hit::Recent(i);
            let pressed = self.press == Some(h);
            let c = if pressed { theme::inset(*cell, 1.5 * s) } else { *cell };
            let tr = rect_f(c.left, c.top, c.right, c.top + l.thumb_h);
            let radius = 12.0 * s;
            theme::shadow(rt, tr, radius, s * 0.6, 0.55 + 0.45 * self.hover_amount(h));
            unsafe { rt.FillRoundedRectangle(&theme::round(tr, radius), &b.well) };
            if !self.thumb_bmps.contains_key(&e.path) {
                if let Some(Some(fr)) = thumbs.get(&e.path) {
                    if let Ok(bm) = d2d::bitmap(rt, fr.width() as u32, fr.height() as u32, &fr.pixels) {
                        self.thumb_bmps.insert(e.path.clone(), bm);
                    }
                }
            }
            if let Some(bm) = self.thumb_bmps.get(&e.path) {
                let sz = unsafe { bm.GetSize() };
                // Cover: fill the cell, crop the overflow.
                let k = ((tr.right - tr.left) / sz.width).max((tr.bottom - tr.top) / sz.height);
                let (dw, dh) = (sz.width * k, sz.height * k);
                let d = rect_f((tr.left + tr.right - dw) / 2.0, (tr.top + tr.bottom - dh) / 2.0, (tr.left + tr.right + dw) / 2.0, (tr.top + tr.bottom + dh) / 2.0);
                clip_round(rt, tr, radius, || unsafe {
                    match rt.cast::<ID2D1DeviceContext>() {
                        Ok(dc) => dc.DrawBitmap(bm, Some(&d), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None),
                        Err(_) => rt.DrawBitmap(bm, Some(&d), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
                    }
                });
            }
            unsafe { rt.DrawRoundedRectangle(&theme::round(theme::inset(tr, 0.5), radius), &b.hair, 1.0, None) };
            let hv = self.hover_amount(h);
            if hv > 0.0 {
                if let Some(ring) = theme::brush(rt, theme::with_alpha(theme::pal().accent, hv)) {
                    unsafe { rt.DrawRoundedRectangle(&theme::round(theme::inset(tr, 1.0 * s), radius - 1.0 * s), &ring, 2.0 * s, None) };
                }
            }
            // Kind badge bottom-left: play symbol for videos, "GIF" for gifs.
            if e.kind != Kind::Image {
                let bw = if e.kind == Kind::Video { 24.0 * s } else { 34.0 * s };
                let br = rect_f(tr.left + 8.0 * s, tr.bottom - 8.0 * s - 22.0 * s, tr.left + 8.0 * s + bw, tr.bottom - 8.0 * s);
                unsafe { rt.FillRoundedRectangle(&theme::round(br, 11.0 * s), &b.scrim) };
                if e.kind == Kind::Video {
                    d2d::draw_text(rt, &f.glyph, "\u{E768}", rect_f(br.left + 1.0 * s, br.top, br.right + 1.0 * s, br.bottom), &b.white);
                } else {
                    align(&f.cap, true);
                    d2d::draw_text(rt, &f.cap, "GIF", rect_f(br.left, br.top + 3.0 * s, br.right, br.bottom), &b.white);
                    align(&f.cap, false);
                }
            }
            let name = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let max_chars = (((c.right - c.left) / (6.6 * s)) as usize).max(8);
            d2d::draw_text(rt, &f.small, &recent::shorten(&name, max_chars), rect_f(c.left + 2.0 * s, tr.bottom + 6.0 * s, c.right, tr.bottom + 22.0 * s), &b.label);
            let age = now.duration_since(e.modified).map(|d| d.as_secs()).unwrap_or(0);
            d2d::draw_text(rt, &f.small, &recent::ago(age), rect_f(c.left + 2.0 * s, tr.bottom + 21.0 * s, c.right, tr.bottom + 38.0 * s), &b.label2);
        }
    }

    unsafe fn draw_settings(&self, rt: &ID2D1RenderTarget, f: &Fonts, b: &Brushes, l: &Lay) {
        let s = self.scale;
        let p = theme::pal();
        let group = rect_f(l.rows[0].left, l.rows[0].top, l.rows[2].right, l.rows[2].bottom);
        let radius = 14.0 * s;
        card(rt, group, radius, s, 0.0, b);
        let labels = [("\u{E765}", "Shortcut"), ("\u{E8B7}", "Save to"), ("\u{E7E8}", "Start with Windows")];
        for (i, (glyph, text)) in labels.iter().enumerate() {
            let r = l.rows[i];
            let h = [Hit::Shortcut, Hit::Folder, Hit::Autostart][i];
            let hv = self.hover_amount(h);
            if hv > 0.0 {
                if let Some(hb) = theme::brush(rt, theme::with_alpha(p.hover, hv)) {
                    // Round only the outer corners of the first and last row.
                    let q = if i == 0 { rect_f(r.left, r.top, r.right, r.bottom) } else { r };
                    clip_round(rt, group, radius, || unsafe { rt.FillRectangle(&q, &hb) });
                }
            }
            if i > 0 {
                unsafe { rt.FillRectangle(&rect_f(r.left + 48.0 * s, r.top, r.right, r.top + 1.0), &b.sep) };
            }
            let ib = rect_f(r.left + 14.0 * s, (r.top + r.bottom) / 2.0 - 12.0 * s, r.left + 38.0 * s, (r.top + r.bottom) / 2.0 + 12.0 * s);
            if let Some(gb) = theme::brush(rt, [p.accent, p.teal, p.green][i]) {
                unsafe { rt.FillRoundedRectangle(&theme::round(ib, 7.0 * s), &gb) };
            }
            d2d::draw_text(rt, &f.glyph, glyph, ib, &b.white);
            d2d::draw_text(rt, &f.body, text, rect_f(r.left + 50.0 * s, (r.top + r.bottom) / 2.0 - 9.5 * s, r.right, r.bottom), &b.label);
            let cy = (r.top + r.bottom) / 2.0;
            match i {
                0 => {
                    let kw = keycap_w(&f.cap, &self.info.hotkey, s);
                    let x = r.right - 34.0 * s - kw;
                    keycap(rt, &f.cap, &self.info.hotkey, x, cy - 11.0 * s, s, b);
                    d2d::draw_text(rt, &f.glyph, "\u{E70D}", rect_f(r.right - 28.0 * s, r.top, r.right - 12.0 * s, r.bottom), &b.label3);
                }
                1 => {
                    let t = &self.info.folder_label;
                    let tw = measure(&f.body, t);
                    d2d::draw_text(rt, &f.body, t, rect_f(r.right - 34.0 * s - tw, cy - 9.5 * s, r.right - 30.0 * s, r.bottom), &b.label2);
                    d2d::draw_text(rt, &f.glyph, "\u{E70D}", rect_f(r.right - 28.0 * s, r.top, r.right - 12.0 * s, r.bottom), &b.label3);
                }
                _ => {
                    // Apple switch: green track, white knob that slides.
                    let k = (self.knob.1.elapsed().as_secs_f32() * 1000.0 / 200.0).min(1.0);
                    let e = 1.0 - (1.0 - k).powi(3);
                    let target = if self.info.autostart { 1.0 } else { 0.0 };
                    let pos = self.knob.0 + (target - self.knob.0) * e;
                    let sw = l.switch;
                    let on = theme::with_alpha(p.green, pos);
                    if let (Some(off), Some(onb)) = (theme::brush(rt, theme::with_alpha(p.selected, 1.6)), theme::brush(rt, on)) {
                        let rr = theme::round(sw, (sw.bottom - sw.top) / 2.0);
                        unsafe {
                            rt.FillRoundedRectangle(&rr, &off);
                            rt.FillRoundedRectangle(&rr, &onb);
                        }
                    }
                    let kr = (sw.bottom - sw.top) / 2.0 - 2.0 * s;
                    let kx = sw.left + 2.0 * s + kr + (sw.right - sw.left - 4.0 * s - kr * 2.0) * pos;
                    let knob = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: kx, Y: (sw.top + sw.bottom) / 2.0 }, radiusX: kr, radiusY: kr };
                    unsafe {
                        if let Ok(sh) = d2d::brush(rt, d2d::rgba(0x000000, 0.22)) {
                            rt.FillEllipse(&D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: kx, Y: knob.point.Y + 1.0 * s }, radiusX: kr + 0.5 * s, radiusY: kr + 0.5 * s }, &sh);
                        }
                        rt.FillEllipse(&knob, &b.white);
                    }
                }
            }
        }
    }
}

struct Brushes {
    label: ID2D1SolidColorBrush,
    label2: ID2D1SolidColorBrush,
    label3: ID2D1SolidColorBrush,
    accent: ID2D1SolidColorBrush,
    accent_hi: ID2D1SolidColorBrush,
    white: ID2D1SolidColorBrush,
    sep: ID2D1SolidColorBrush,
    hair: ID2D1SolidColorBrush,
    well: ID2D1SolidColorBrush,
    scrim: ID2D1SolidColorBrush,
    card: ID2D1SolidColorBrush,
    keycap: ID2D1SolidColorBrush,
}

impl Brushes {
    fn new(rt: &ID2D1RenderTarget) -> Option<Brushes> {
        let p = theme::pal();
        Some(Brushes {
            label: theme::brush(rt, p.label)?,
            label2: theme::brush(rt, p.label2)?,
            label3: theme::brush(rt, p.label3)?,
            accent: theme::brush(rt, p.accent)?,
            accent_hi: theme::brush(rt, theme::with_alpha(p.accent, 0.75))?,
            white: theme::brush(rt, Rgba(0xFFFFFF, 1.0))?,
            sep: theme::brush(rt, p.separator)?,
            hair: theme::brush(rt, p.hairline)?,
            well: theme::brush(rt, theme::with_alpha(p.hover, 1.3))?,
            scrim: theme::brush(rt, Rgba(0x000000, 0.55))?,
            card: theme::brush(rt, p.card)?,
            keycap: theme::brush(rt, if p.dark { Rgba(0xFFFFFF, 0.10) } else { Rgba(0xFFFFFF, 0.9) })?,
        })
    }
}

fn measure(fmt: &IDWriteTextFormat, text: &str) -> f32 {
    let wide: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        let Ok(dw) = windows::Win32::Graphics::DirectWrite::DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) else { return 0.0 };
        match dw.CreateTextLayout(&wide, fmt, 10000.0, 1000.0) {
            Ok(l) => {
                let mut m = DWRITE_TEXT_METRICS::default();
                if l.GetMetrics(&mut m).is_ok() { m.widthIncludingTrailingWhitespace } else { 0.0 }
            }
            Err(_) => 0.0,
        }
    }
}

/// A grouped surface on the window background: soft shadow, fill, rim.
fn card(rt: &ID2D1RenderTarget, r: D2D_RECT_F, radius: f32, s: f32, hover: f32, b: &Brushes) {
    let p = theme::pal();
    theme::shadow(rt, r, radius, s * 0.6, 0.45 + 0.4 * hover);
    unsafe {
        rt.FillRoundedRectangle(&theme::round(r, radius), &b.card);
        if hover > 0.0 {
            if let Some(h) = theme::brush(rt, theme::with_alpha(p.hover, hover)) {
                rt.FillRoundedRectangle(&theme::round(r, radius), &h);
            }
        }
    }
    theme::rim(rt, r, radius, s);
}

fn align(fmt: &IDWriteTextFormat, center: bool) {
    unsafe {
        let _ = fmt.SetTextAlignment(if center { DWRITE_TEXT_ALIGNMENT_CENTER } else { DWRITE_TEXT_ALIGNMENT_LEADING });
    }
}

fn keycap_w(fmt: &IDWriteTextFormat, text: &str, s: f32) -> f32 {
    measure(fmt, text) + 16.0 * s
}

/// A key drawn like a keyboard key.
fn keycap(rt: &ID2D1RenderTarget, fmt: &IDWriteTextFormat, text: &str, x: f32, y: f32, s: f32, b: &Brushes) {
    let w = keycap_w(fmt, text, s);
    let r = rect_f(x, y, x + w, y + 22.0 * s);
    unsafe {
        rt.FillRoundedRectangle(&theme::round(rect_f(r.left, r.top + 1.5 * s, r.right, r.bottom + 1.5 * s), 6.0 * s), &b.sep);
        rt.FillRoundedRectangle(&theme::round(r, 6.0 * s), &b.keycap);
        rt.DrawRoundedRectangle(&theme::round(theme::inset(r, 0.5), 6.0 * s), &b.sep, 1.0, None);
    }
    d2d::draw_text(rt, fmt, text, rect_f(r.left + 8.0 * s, r.top + 3.0 * s, r.right, r.bottom), &b.label2);
}

fn dashed(rt: &ID2D1RenderTarget) -> Option<ID2D1StrokeStyle> {
    unsafe {
        let factory = rt.GetFactory().ok()?;
        let props = D2D1_STROKE_STYLE_PROPERTIES { dashStyle: D2D1_DASH_STYLE_DASH, dashCap: D2D1_CAP_STYLE_ROUND, ..Default::default() };
        factory.CreateStrokeStyle(&props, None).ok()
    }
}

/// Runs `f` clipped to a rounded rect.
fn clip_round(rt: &ID2D1RenderTarget, r: D2D_RECT_F, radius: f32, f: impl FnOnce()) {
    unsafe {
        let dc = rt.cast::<ID2D1DeviceContext>().ok();
        let geo = rt.GetFactory().ok().and_then(|fa| fa.CreateRoundedRectangleGeometry(&theme::round(r, radius)).ok());
        match (dc, geo) {
            (Some(dc), Some(geo)) => {
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
            }
            _ => {
                rt.PushAxisAlignedClip(&r, D2D1_ANTIALIAS_MODE_ALIASED);
                f();
                rt.PopAxisAlignedClip();
            }
        }
    }
}

/// A small picture of a capture, at most `px` wide.
fn load_thumb(e: &Entry, px: u32) -> Option<Frame> {
    let full = match e.kind {
        Kind::Image => crate::image::decode_png(&std::fs::read(&e.path).ok()?)?,
        Kind::Gif => crate::image::decode_gif_first(&std::fs::read(&e.path).ok()?)?,
        Kind::Video => {
            let mut r = super::mp4read::Reader::open(&e.path).ok()?;
            // A frame a little in, so it isn't a black fade-in.
            if r.duration > 10_000_000 {
                r.seek(5_000_000);
            }
            r.next()?.1
        }
    };
    let (w, h) = (full.width() as u32, full.height() as u32);
    if w == 0 || h == 0 {
        return None;
    }
    let k = (px as f32 / w as f32).min(1.0);
    let (ow, oh) = (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1));
    Some(vedit::resample(&full, Cam::full(w as f32, h as f32), ow, oh))
}

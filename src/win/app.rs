//! App wiring: the hidden message window, tray, hotkey, overlay and preview
//! card. Everything runs on one UI thread.
//!
//! State lives in a thread-local `App`. Window procedures borrow it with
//! `try_borrow_mut`; modal UI (menus, dialogs) always runs with the borrow
//! released, so nested messages never hit a busy borrow when it matters.

use super::card::{self, Button, Card};
use super::d2d::Gfx;
use super::overlay::{self, Overlay};
use super::util::log;
use super::{WM_APP_CAPTURE, WM_APP_TRAY, class_name, clipboard, files, grab, hook, paths, tray, wins};
use crate::config::Config;
use crate::geom::{Point, Rect};
use crate::image::Frame;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::UI::Shell::NIN_SELECT;
const NIN_KEYSELECT: u32 = NIN_SELECT + 1;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

struct Last {
    frame: Rc<Frame>,
    rect: Rect,
    saved: Option<PathBuf>,
}

struct App {
    msg: HWND,
    gfx: Gfx,
    cfg: Config,
    cfg_path: Option<PathBuf>,
    overlay: Overlay,
    card: Card,
    last: Option<Last>,
    icon: HICON,
    taskbar_created: u32,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// Frame promised to the clipboard as PNG (delayed rendering). Kept apart
    /// from `APP` because clipboard requests can arrive at any moment.
    static CLIP: RefCell<Option<Rc<Frame>>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

pub fn run() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
    let want_capture = std::env::args().any(|a| a == "--capture");

    // Single instance: a second launch asks the running one to capture.
    let mutex = unsafe { CreateMutexW(None, true, w!("Local\\RengiSnap.Instance")) };
    if mutex.is_ok() && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if let Ok(other) = unsafe { FindWindowW(class_name::MSG, None) } {
            unsafe {
                let _ = PostMessageW(Some(other), WM_APP_CAPTURE, WPARAM(0), LPARAM(0));
            }
        }
        return;
    }

    dark_menus();
    let instance: HINSTANCE = unsafe { GetModuleHandleW(None) }.map(|m| m.into()).unwrap_or_default();
    register_classes(instance);

    let (cfg, cfg_path, first_run) = load_config();
    if first_run && std::env::var_os("RENGI_SNAP_NO_AUTOSTART").is_none() {
        files::set_autostart(true);
    }

    let msg = unsafe {
        CreateWindowExW(WS_EX_TOOLWINDOW, class_name::MSG, w!("Rengi Snap"), WS_POPUP, 0, 0, 0, 0, None, None, Some(instance), None)
    };
    let Ok(msg) = msg else {
        log("cannot create message window");
        return;
    };
    let (Ok(gfx), Ok(overlay), Ok(card)) = (Gfx::new(), Overlay::create(instance), Card::create(instance)) else {
        log("cannot create graphics or windows");
        return;
    };
    let icon = make_icon();
    let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    tray::add(msg, icon);
    if !hook::install(msg, cfg.hotkey.clone()) {
        log("keyboard hook failed");
    }
    APP.with(|a| {
        *a.borrow_mut() = Some(App { msg, gfx, cfg, cfg_path, overlay, card, last: None, icon, taskbar_created });
    });
    if want_capture {
        unsafe {
            let _ = PostMessageW(Some(msg), WM_APP_CAPTURE, WPARAM(0), LPARAM(0));
        }
    }

    unsafe {
        let mut m = MSG::default();
        while GetMessageW(&mut m, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&m);
            DispatchMessageW(&m);
        }
    }
    hook::uninstall();
    tray::remove(msg);
    let _ = mutex;
}

fn register_classes(instance: HINSTANCE) {
    unsafe {
        let arrow = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        for (name, proc_, style) in [
            (class_name::MSG, Some(msg_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT), WNDCLASS_STYLES(0)),
            (class_name::OVERLAY, Some(overlay_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT), CS_DBLCLKS),
            (class_name::CARD, Some(card_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT), CS_DROPSHADOW),
        ] {
            let wc = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style,
                lpfnWndProc: proc_,
                hInstance: instance,
                hCursor: arrow,
                lpszClassName: name,
                ..Default::default()
            };
            RegisterClassExW(&wc);
        }
    }
}

fn load_config() -> (Config, Option<PathBuf>, bool) {
    let Some(dir) = paths::data_dir() else { return (Config::default(), None, false) };
    let path = dir.join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(text) => (Config::parse(&text), Some(path), false),
        Err(_) => {
            let cfg = Config::default();
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&path, cfg.to_text());
            (cfg, Some(path), true)
        }
    }
}

impl App {
    fn save_config(&self) {
        if let Some(p) = &self.cfg_path {
            let _ = std::fs::write(p, self.cfg.to_text());
        }
    }

    fn current_folder(&self) -> PathBuf {
        self.cfg.save_folder(&paths::pictures_dir())
    }

    fn folder_label(&self) -> String {
        files::folder_name(&self.current_folder())
    }

    fn start_capture(&mut self) {
        if self.overlay.is_active() {
            return;
        }
        if self.card.is_visible() {
            self.card.hide();
            // Let DWM compose a frame without the card before grabbing.
            unsafe {
                let _ = DwmFlush();
            }
        }
        let Some(frame) = grab::grab_desktop() else {
            log("desktop grab failed");
            return;
        };
        let (windows, monitors) = wins::snapshot();
        let label = self.folder_label();
        if self.overlay.show(&self.gfx, Arc::new(frame), windows, monitors, self.cfg.loupe, label) {
            hook::set_overlay(Some(self.overlay.hwnd));
        }
    }

    fn end_overlay(&mut self) -> Option<Arc<Frame>> {
        hook::set_overlay(None);
        let frame = self.overlay_frame();
        self.overlay.hide();
        frame
    }

    fn overlay_frame(&self) -> Option<Arc<Frame>> {
        self.overlay.frame()
    }

    fn finish(&mut self, rect: Rect) {
        let Some(full) = self.end_overlay() else { return };
        let frame = Rc::new(full.crop(rect));
        if frame.width() == 0 || frame.height() == 0 {
            return;
        }
        let copied = clipboard::put_image(self.msg, &frame);
        CLIP.with(|c| *c.borrow_mut() = copied.then(|| frame.clone()));
        let mut status = if copied { "Copied to clipboard".to_string() } else { "Could not open the clipboard".to_string() };
        let mut saved = None;
        if self.cfg.auto_save {
            match files::save_png(&frame, &self.current_folder(), paths::local_stamp()) {
                Ok(p) => {
                    status = format!("Copied and saved as {}", file_name(&p));
                    saved = Some(p);
                }
                Err(e) => status = format!("Copied, but saving failed: {e}"),
            }
        }
        self.card.show(&self.gfx, &frame, rect, status, self.cfg.card_seconds);
        self.last = Some(Last { frame, rect, saved });
    }

    fn save_last_to(&mut self, folder: PathBuf) {
        let Some(last) = &mut self.last else { return };
        match files::save_png(&last.frame, &folder, paths::local_stamp()) {
            Ok(p) => {
                self.card.set_status(format!("Saved as {}", file_name(&p)));
                last.saved = Some(p);
            }
            Err(e) => self.card.set_status(format!("Saving failed: {e}")),
        }
    }

    fn card_click(&mut self, b: Button) {
        match b {
            Button::Copy => {
                if let Some(last) = &self.last {
                    let ok = clipboard::put_image(self.msg, &last.frame);
                    let f = last.frame.clone();
                    CLIP.with(|c| *c.borrow_mut() = ok.then_some(f));
                    self.card.set_status(if ok { "Copied to clipboard".into() } else { "Could not open the clipboard".into() });
                }
            }
            Button::Save => {
                let folder = self.current_folder();
                self.save_last_to(folder);
            }
            Button::Folder => {
                let saved = self.last.as_ref().and_then(|l| l.saved.clone());
                let folder = saved.as_ref().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| self.current_folder());
                files::open_folder(&folder, saved.as_deref());
                self.card.hide();
            }
            Button::Thumb => {
                let Some(last) = &mut self.last else { return };
                let path = match &last.saved {
                    Some(p) if p.exists() => Some(p.clone()),
                    _ => {
                        let tmp = std::env::temp_dir().join("Rengi Snap");
                        files::save_png(&last.frame, &tmp, paths::local_stamp()).ok()
                    }
                };
                if let Some(p) = path {
                    files::open_path(&p);
                }
                self.card.hide();
            }
            Button::Close => self.card.hide(),
            Button::SaveTo | Button::Edit => {}
        }
    }
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Folder dropdown shared by the panel and the card. Runs with no borrow held.
/// Returns the chosen folder (already made current and saved to config).
fn choose_folder(owner: HWND, at: Point) -> Option<PathBuf> {
    let (folders, current) = with_app(|a| {
        let current = a.current_folder().to_string_lossy().into_owned();
        let mut list = a.cfg.recent_folders.clone();
        if !list.iter().any(|f| f.eq_ignore_ascii_case(&current)) {
            list.insert(0, current.clone());
        }
        (list, current)
    })?;
    hook::set_overlay(None);
    let pick = files::folder_menu(owner, (at.x, at.y), &folders, &current);
    let chosen = match pick {
        Some(files::MENU_CHOOSE) => files::pick_folder(owner, std::path::Path::new(&current)),
        Some(i) => folders.get(i).map(PathBuf::from),
        None => None,
    };
    with_app(|a| {
        if a.overlay.is_active() {
            hook::set_overlay(Some(a.overlay.hwnd));
        }
        if let Some(c) = &chosen {
            a.cfg.use_folder(&c.to_string_lossy());
            a.save_config();
            let label = a.folder_label();
            a.overlay.set_folder_label(label, &a.gfx);
        }
    });
    chosen
}

fn on_overlay_event(ev: overlay::Event) {
    match ev {
        overlay::Event::Confirm(r) => {
            with_app(|a| a.finish(r));
        }
        overlay::Event::Cancel => {
            with_app(|a| {
                a.end_overlay();
            });
        }
        overlay::Event::CopyColor(hex) => {
            with_app(|a| {
                a.end_overlay();
                clipboard::put_text(a.msg, &hex);
                CLIP.with(|c| *c.borrow_mut() = None);
            });
        }
        overlay::Event::FolderMenu(at) => {
            if let Some(owner) = with_app(|a| a.overlay.hwnd) {
                choose_folder(owner, at);
            }
        }
    }
}

fn on_card_event(ev: card::Event) {
    let card::Event::Clicked(b, at) = ev;
    if b == Button::SaveTo {
        let Some(owner) = with_app(|a| a.card.hwnd) else { return };
        if let Some(folder) = choose_folder(owner, at) {
            with_app(|a| a.save_last_to(folder));
        }
        return;
    }
    with_app(|a| a.card_click(b));
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some((res, ev)) = with_app(|a| a.overlay.handle(msg, wp, lp)) {
        if let Some(ev) = ev {
            on_overlay_event(ev);
        }
        if let Some(res) = res {
            return res;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn card_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some((res, ev)) = with_app(|a| a.card.handle(msg, wp, lp)) {
        if let Some(ev) = ev {
            on_card_event(ev);
        }
        if let Some(res) = res {
            return res;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn msg_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_APP_CAPTURE => {
            with_app(|a| a.start_capture());
            return LRESULT(0);
        }
        WM_APP_TRAY => {
            let event = (lp.0 & 0xFFFF) as u32;
            if event == NIN_SELECT || event == NIN_KEYSELECT || event == WM_LBUTTONUP {
                with_app(|a| a.start_capture());
            } else if event == WM_CONTEXTMENU || event == WM_RBUTTONUP {
                let auto = files::autostart_enabled();
                match tray::menu(hwnd, auto) {
                    tray::CMD_CAPTURE => {
                        // Let the menu fade out before freezing the screen.
                        unsafe {
                            let _ = PostMessageW(Some(hwnd), WM_APP_CAPTURE, WPARAM(0), LPARAM(0));
                        }
                    }
                    tray::CMD_OPEN_FOLDER => {
                        if let Some(f) = with_app(|a| a.current_folder()) {
                            files::open_folder(&f, None);
                        }
                    }
                    tray::CMD_AUTOSTART => files::set_autostart(!auto),
                    tray::CMD_EXIT => unsafe {
                        let _ = DestroyWindow(hwnd);
                    },
                    _ => {}
                }
            }
            return LRESULT(0);
        }
        WM_RENDERFORMAT => {
            CLIP.with(|c| {
                if let Some(f) = c.borrow().as_ref() {
                    clipboard::render_png(f);
                }
            });
            return LRESULT(0);
        }
        WM_RENDERALLFORMATS => {
            let f = CLIP.with(|c| c.borrow().clone());
            if let Some(f) = f {
                clipboard::render_all(hwnd, &f);
            }
            return LRESULT(0);
        }
        WM_DESTROYCLIPBOARD => {
            CLIP.with(|c| *c.borrow_mut() = None);
            return LRESULT(0);
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {}
    }
    let tb = with_app(|a| (a.taskbar_created, a.icon));
    if let Some((tb_msg, icon)) = tb {
        if msg == tb_msg {
            tray::add(hwnd, icon);
            return LRESULT(0);
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn make_icon() -> HICON {
    let n = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16) as usize;
    let px = crate::icon::pixels(n);
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: n as i32,
                biHeight: -(n as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = GetDC(None);
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0);
        ReleaseDC(None, dc);
        let Ok(color) = color else { return HICON::default() };
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mask = CreateBitmap(n as i32, n as i32, 1, 1, None);
        let ii = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&ii).unwrap_or_default();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}


/// Dark popup menus to match the panel. Uses the uxtheme entry points every
/// dark-mode Win32 app relies on (ordinals 135 SetPreferredAppMode and
/// 136 FlushMenuThemes); on systems without them nothing happens.
fn dark_menus() {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::PCSTR;
    unsafe {
        let Ok(m) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(f) = GetProcAddress(m, PCSTR(135usize as *const u8)) {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set(2); // ForceDark
        }
        if let Some(f) = GetProcAddress(m, PCSTR(136usize as *const u8)) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

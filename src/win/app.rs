//! App wiring: the hidden message window, tray, hotkey, overlay and preview
//! card. Everything runs on one UI thread.
//!
//! State lives in a thread-local `App`. Window procedures borrow it with
//! `try_borrow_mut`; modal UI (menus, dialogs) always runs with the borrow
//! released, so nested messages never hit a busy borrow when it matters.

use super::card::{self, Button, Card};
use super::d2d::Gfx;
use super::editor::{self, Editor};
use super::veditor::{self, VEditor};
use crate::annot::{Color, Item, Tool};
use super::overlay::{self, Overlay, PanelState};
use super::recbar::{BarButton, RecBar};
use super::record::{self, Recorder};
use super::util::log;
use super::{AUTOCOPY_TIMER, WM_APP_CAPTURE, WM_APP_EDIT, WM_APP_GIF, WM_APP_TRAY, class_name, clipboard, files, grab, hook, mp4read, paths, tray, wins};
use crate::videoplan::{self, Res};
use crate::config::Config;
use crate::geom::{Point, Rect};
use crate::image::Frame;
use std::cell::RefCell;
use std::os::windows::ffi::OsStrExt;
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
    /// What the user got (with marks).
    frame: Rc<Frame>,
    /// The bare capture and its marks, so the editor reopens them editable.
    base: Rc<Frame>,
    items: Vec<Item>,
    rect: Rect,
    saved: Option<PathBuf>,
}

struct Recording {
    recorder: Recorder,
    region: Rect,
}

struct LastVideo {
    path: PathBuf,
    /// Expected decoded frames, for GIF progress.
    frames: u64,
    gif_busy: bool,
}

struct App {
    msg: HWND,
    gfx: Gfx,
    cfg: Config,
    cfg_path: Option<PathBuf>,
    overlay: Overlay,
    card: Card,
    editor: Option<Editor>,
    veditor: Option<VEditor>,
    instance: HINSTANCE,
    recbar: RecBar,
    rec: Option<Recording>,
    last: Option<Last>,
    last_video: Option<LastVideo>,
    /// The card currently shows a video.
    card_video: bool,
    icon: HICON,
    taskbar_created: u32,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// Video handed over by a second instance (`--edit`).
    static PENDING_EDIT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
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
    let args: Vec<String> = std::env::args().collect();
    let want_capture = args.iter().any(|a| a == "--capture");
    // `--edit <video>` opens the video editor (also used by "Open with").
    let want_edit = args.iter().position(|a| a == "--edit").and_then(|i| args.get(i + 1)).map(PathBuf::from);

    // Single instance: a second launch hands its request to the running one.
    let mutex = unsafe { CreateMutexW(None, true, w!("Local\\RengiSnap.Instance")) };
    if mutex.is_ok() && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if let Ok(other) = unsafe { FindWindowW(class_name::MSG, None) } {
            unsafe {
                match &want_edit {
                    Some(p) => {
                        let w: Vec<u16> = p.as_os_str().encode_wide().collect();
                        let cds = windows::Win32::System::DataExchange::COPYDATASTRUCT { dwData: COPY_EDIT, cbData: (w.len() * 2) as u32, lpData: w.as_ptr() as *mut _ };
                        let _ = SendMessageW(other, WM_COPYDATA, None, Some(LPARAM(&cds as *const _ as isize)));
                    }
                    None => {
                        let _ = PostMessageW(Some(other), WM_APP_CAPTURE, WPARAM(0), LPARAM(0));
                    }
                }
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
    let (Ok(gfx), Ok(overlay), Ok(card), Ok(recbar)) = (Gfx::new(), Overlay::create(instance), Card::create(instance), RecBar::create(instance)) else {
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
        *a.borrow_mut() = Some(App {
            msg,
            gfx,
            cfg,
            cfg_path,
            overlay,
            card,
            editor: None,
            veditor: None,
            instance,
            recbar,
            rec: None,
            last: None,
            last_video: None,
            card_video: false,
            icon,
            taskbar_created,
        });
    });
    if want_capture {
        unsafe {
            let _ = PostMessageW(Some(msg), WM_APP_CAPTURE, WPARAM(0), LPARAM(0));
        }
    }
    if let Some(p) = want_edit {
        with_app(|a| a.open_video_editor(p));
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
            (class_name::BAR, Some(bar_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT), CS_DROPSHADOW),
            (class_name::EDITOR, Some(editor_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT), CS_DBLCLKS),
            (class_name::VEDITOR, Some(veditor_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT), CS_DBLCLKS),
            (class_name::OUTLINE, Some(outline_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT), WNDCLASS_STYLES(0)),
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

    fn panel_state(&self) -> PanelState {
        PanelState {
            video: self.cfg.mode == "video",
            fps_label: match self.cfg.video_fps {
                0 => "Max fps".into(),
                n => format!("{n} fps"),
            },
            res_label: match Res::parse(&self.cfg.video_res) {
                Res::Original => "Original".into(),
                Res::P1080 => "1080p".into(),
                Res::P720 => "720p".into(),
            },
            sound: self.cfg.system_audio,
            mic: self.cfg.mic,
        }
    }

    fn refresh_panel(&mut self) {
        self.save_config();
        let st = self.panel_state();
        self.overlay.set_state(st, &self.gfx);
    }

    fn start_capture(&mut self) {
        if self.rec.is_some() {
            self.stop_recording(false);
            return;
        }
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
        let st = self.panel_state();
        self.overlay.style = (Color::from_hex(self.cfg.ed_color), self.cfg.ed_width as usize);
        // Route keys before the overlay appears, so an early Esc is not lost.
        hook::set_overlay(Some(self.overlay.hwnd));
        if !self.overlay.show(&self.gfx, Arc::new(frame), windows, monitors, self.cfg.loupe, label, st) {
            hook::set_overlay(None);
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
        if self.overlay.is_video() {
            self.start_recording(rect);
            return;
        }
        let items = self.overlay.take_items(rect);
        let Some(full) = self.end_overlay() else { return };
        let base = Rc::new(full.crop(rect));
        if base.width() == 0 || base.height() == 0 {
            return;
        }
        let frame = if items.is_empty() { base.clone() } else { Rc::new(self.render_marks(&base, &items)) };
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
        self.card.show(&self.gfx, &frame, rect, status, self.cfg.card_seconds, false);
        self.card_video = false;
        self.last = Some(Last { frame, base, items, rect, saved });
    }

    fn render_marks(&self, base: &Frame, items: &[Item]) -> Frame {
        let pm = crate::annot::render::base_pixmap(base);
        let fonts = crate::annot::font::shared();
        let out = crate::annot::render::render(&pm, items, Rect::from_xywh(0, 0, base.width() as i32, base.height() as i32), fonts.as_deref(), self.overlay_scale());
        crate::annot::render::to_frame(&out)
    }

    fn overlay_scale(&self) -> f32 {
        let dpi = unsafe { windows::Win32::UI::HiDpi::GetDpiForSystem() }.max(96);
        dpi as f32 / 96.0
    }

    fn open_editor(&mut self) {
        let Some(last) = &self.last else { return };
        let (base, items, near) = (last.base.clone(), last.items.clone(), last.rect);
        self.card.hide();
        if let Some(old) = self.editor.take() {
            unsafe {
                let _ = DestroyWindow(old.hwnd);
            }
        }
        let style = (Tool::parse(&self.cfg.ed_tool), Color::from_hex(self.cfg.ed_color), self.cfg.ed_width as usize);
        match Editor::open(self.instance, &self.gfx, &base, items, crate::annot::font::shared(), style, near) {
            Ok(mut e) => {
                if let Some(p) = &last_saved(&self.last) {
                    e.saved_path = Some(p.clone());
                    e.set_title(&format!("{} - Rengi Snap", file_name(p)));
                }
                self.editor = Some(e);
            }
            Err(err) => log(&format!("cannot open editor: {err}")),
        }
    }

    /// Copies the editor's current image and keeps `last` in step.
    fn editor_copy(&mut self, status: bool) {
        let Some(e) = &mut self.editor else { return };
        let frame = Rc::new(e.final_frame());
        let ok = clipboard::put_image(self.msg, &frame);
        CLIP.with(|c| *c.borrow_mut() = ok.then(|| frame.clone()));
        if status {
            e.set_status(if ok { "Copied to clipboard".into() } else { "Could not open the clipboard".into() });
        }
        let items = e.items();
        if let Some(last) = &mut self.last {
            last.frame = frame;
            last.items = items;
        }
    }

    fn editor_save(&mut self, folder: Option<PathBuf>) {
        let Some(e) = &mut self.editor else { return };
        let frame = e.final_frame();
        let target = match (&folder, &e.saved_path) {
            (None, Some(p)) => Some(p.clone()),
            _ => None,
        };
        let result = match target {
            Some(p) => std::fs::write(&p, frame.encode_png()).map(|_| p),
            None => files::save_png(&frame, &folder.unwrap_or_else(|| self.cfg.save_folder(&paths::pictures_dir())), paths::local_stamp()),
        };
        match result {
            Ok(p) => {
                e.set_status(format!("Saved {}", file_name(&p)));
                e.set_title(&format!("{} - Rengi Snap", file_name(&p)));
                e.saved_path = Some(p.clone());
                if let Some(last) = &mut self.last {
                    last.saved = Some(p);
                }
            }
            Err(err) => e.set_status(format!("Saving failed: {err}")),
        }
    }

    fn open_video_editor(&mut self, path: PathBuf) {
        self.card.hide();
        if let Some(old) = self.veditor.take() {
            unsafe {
                let _ = DestroyWindow(old.hwnd);
            }
        }
        let style = (Tool::parse(&self.cfg.ed_tool), Color::from_hex(self.cfg.ed_color), self.cfg.ed_width as usize);
        match VEditor::open(self.instance, &self.gfx, path, crate::annot::font::shared(), style) {
            Ok(e) => self.veditor = Some(e),
            Err(err) => {
                log(&format!("cannot open the video editor: {err}"));
                let text = super::util::wide(&format!("Can't open this video: {err}"));
                unsafe {
                    MessageBoxW(None, windows::core::PCWSTR(text.as_ptr()), w!("Rengi Snap"), MB_OK | MB_ICONWARNING);
                }
            }
        }
    }

    fn remember_style(&mut self, tool: Option<Tool>, color: Color, width: usize) {
        if let Some(t) = tool {
            if t != Tool::Crop && t != Tool::Zoom {
                self.cfg.ed_tool = t.name().into();
            }
        }
        self.cfg.ed_color = color.hex();
        self.cfg.ed_width = width as u32;
        self.save_config();
    }

    fn start_recording(&mut self, rect: Rect) {
        let full = self.end_overlay();
        let (monitor, mon_rect) = unsafe {
            use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect};
            let m = MonitorFromRect(&super::util::to_win_rect(rect), MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(m, &mut info);
            (m.0 as isize, super::util::to_rect(info.rcMonitor))
        };
        let region = videoplan::even_rect(rect.intersect(&mon_rect));
        if region.width() < 16 || region.height() < 16 {
            return;
        }
        let fps = match self.cfg.video_fps {
            0 => record::refresh_rate(monitor),
            n => n,
        };
        let plan = videoplan::plan(region.width() as u32, region.height() as u32, fps, Res::parse(&self.cfg.video_res), record::hevc_available());
        let folder = self.current_folder();
        let name = crate::config::stamped_name("Video", "mp4", paths::local_stamp(), |n| folder.join(n).exists());
        let params = record::Params {
            monitor,
            monitor_rect: mon_rect,
            region,
            plan,
            path: folder.join(name),
            system_audio: self.cfg.system_audio,
            mic: self.cfg.mic,
        };
        // Make sure the overlay is gone from the screen before frames flow.
        unsafe {
            let _ = DwmFlush();
        }
        match Recorder::start(params) {
            Ok(r) => {
                self.recbar.show(&self.gfx, region, self.cfg.mic);
                self.rec = Some(Recording { recorder: r, region });
            }
            Err(e) => {
                log(&format!("cannot start recording: {e}"));
                if let Some(full) = full {
                    let frame = full.crop(region);
                    self.card.show(&self.gfx, &frame, region, format!("Recording failed: {e}"), 0, true);
                    self.card_video = true;
                    self.last_video = None;
                }
            }
        }
    }

    fn stop_recording(&mut self, discard: bool) {
        let Some(rec) = self.rec.take() else { return };
        self.recbar.hide();
        match rec.recorder.finish(discard) {
            Ok(sum) if !discard => {
                let secs = sum.seconds.round() as u64;
                let status = format!(
                    "{}:{:02} \u{00B7} {}x{} \u{00B7} {} fps{} \u{00B7} {}",
                    secs / 60,
                    secs % 60,
                    sum.plan.out_w,
                    sum.plan.out_h,
                    sum.plan.fps,
                    if sum.plan.codec == videoplan::Codec::Hevc { " HEVC" } else { "" },
                    file_name(&sum.path)
                );
                let thumb = sum.thumb.unwrap_or_else(|| Frame {
                    rect: Rect::from_xywh(0, 0, 16, 9),
                    pixels: vec![0; 16 * 9 * 4],
                });
                self.card.show(&self.gfx, &thumb, rec.region, status, self.cfg.card_seconds, true);
                self.card_video = true;
                self.last_video = Some(LastVideo { path: sum.path, frames: sum.frames, gif_busy: false });
            }
            Ok(_) => {}
            Err(e) => {
                log(&format!("recording failed: {e}"));
            }
        }
    }

    fn bar_click(&mut self, b: BarButton) {
        match b {
            BarButton::Stop => self.stop_recording(false),
            BarButton::Discard => self.stop_recording(true),
            BarButton::Pause => {
                if let Some(r) = &mut self.rec {
                    let on = !r.recorder.is_paused();
                    r.recorder.set_paused(on);
                    self.recbar.paused = on;
                    self.recbar.invalidate();
                }
            }
            BarButton::Mic => {
                if let Some(r) = &self.rec {
                    let on = !r.recorder.mic_muted();
                    r.recorder.set_mic_muted(on);
                    self.recbar.muted = on;
                    self.recbar.invalidate();
                }
            }
        }
    }

    fn bar_tick(&mut self) {
        if let Some(r) = &self.rec {
            self.recbar.seconds = r.recorder.elapsed();
            self.recbar.invalidate();
        }
    }

    fn video_card_click(&mut self, b: Button) {
        let Some(v) = &mut self.last_video else {
            if b == Button::Close {
                self.card.hide();
            }
            return;
        };
        match b {
            Button::Copy => {
                let ok = clipboard::put_file(self.msg, &v.path);
                CLIP.with(|c| *c.borrow_mut() = None);
                self.card.set_status(if ok { "Video file copied - paste it anywhere".into() } else { "Could not open the clipboard".into() });
            }
            Button::Folder => {
                let folder = v.path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                files::open_folder(&folder, Some(&v.path));
                self.card.hide();
            }
            Button::Thumb => {
                files::open_path(&v.path);
                self.card.hide();
            }
            Button::Gif => {
                if v.gif_busy {
                    return;
                }
                v.gif_busy = true;
                let (src, frames, notify) = (v.path.clone(), v.frames, self.msg.0 as isize);
                self.card.set_status("Making GIF...".into());
                std::thread::spawn(move || export_gif(src, frames, notify));
            }
            Button::Edit => {
                let p = v.path.clone();
                self.open_video_editor(p);
            }
            Button::Close => self.card.hide(),
            _ => {}
        }
    }

    fn move_video_to(&mut self, folder: PathBuf) {
        let Some(v) = &mut self.last_video else { return };
        match files::move_into(&v.path, &folder) {
            Ok(p) => {
                self.card.set_status(format!("Moved to {}", folder.display()));
                v.path = p;
            }
            Err(e) => self.card.set_status(format!("Moving failed: {e}")),
        }
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
            Button::Edit => self.open_editor(),
            Button::SaveTo | Button::Gif => {}
        }
    }
}

/// Decodes the video and writes a GIF next to it; posts progress to `notify`.
fn export_gif(src: PathBuf, frames: u64, notify: isize) {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
    }
    let post = |w: usize| unsafe {
        let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_APP_GIF, WPARAM(w), LPARAM(0));
    };
    let result = (|| -> Result<(), String> {
        let mut r = mp4read::Reader::open(&src)?;
        let out = src.with_extension("gif");
        let file = std::io::BufWriter::new(std::fs::File::create(&out).map_err(|e| e.to_string())?);
        let mut g = crate::gifenc::GifWriter::new(file, r.w, r.h)?;
        let mut n = 0u64;
        let mut last_t = 0i64;
        let mut last_pct = 0usize;
        while let Some((t, f)) = r.next() {
            g.push(t, &f)?;
            last_t = t;
            n += 1;
            let pct = ((n * 100) / frames.max(1)).min(99) as usize;
            if pct >= last_pct + 5 {
                last_pct = pct;
                post(pct);
            }
        }
        let _ = last_t;
        if n == 0 {
            return Err("no frames decoded".into());
        }
        g.finish(333_333)?;
        Ok(())
    })();
    if let Err(e) = &result {
        log(&format!("gif export failed: {e}"));
        let _ = std::fs::remove_file(src.with_extension("gif"));
    }
    post(if result.is_ok() { 1000 } else { 1001 });
}

fn last_saved(last: &Option<Last>) -> Option<PathBuf> {
    last.as_ref().and_then(|l| l.saved.clone())
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
        overlay::Event::ConfirmSave(r) => {
            with_app(|a| {
                a.finish(r);
                let folder = a.current_folder();
                a.save_last_to(folder);
            });
        }
        overlay::Event::OpenEditor(r) => {
            with_app(|a| {
                a.finish(r);
                a.open_editor();
            });
        }
        overlay::Event::Style(c, w) => {
            with_app(|a| a.remember_style(None, c, w));
        }
        overlay::Event::Mode(video) => {
            with_app(|a| {
                a.cfg.mode = if video { "video" } else { "photo" }.into();
                a.refresh_panel();
            });
        }
        overlay::Event::ToggleSound => {
            with_app(|a| {
                a.cfg.system_audio = !a.cfg.system_audio;
                a.refresh_panel();
            });
        }
        overlay::Event::ToggleMic => {
            with_app(|a| {
                a.cfg.mic = !a.cfg.mic;
                a.refresh_panel();
            });
        }
        overlay::Event::FpsMenu(at) => {
            const VALUES: [u32; 4] = [30, 60, 120, 0];
            let Some((owner, cur)) = with_app(|a| (a.overlay.hwnd, a.cfg.video_fps)) else { return };
            let checked = VALUES.iter().position(|v| *v == cur).unwrap_or(1);
            hook::set_overlay(None);
            let pick = files::choice_menu(owner, (at.x, at.y), &["30 fps", "60 fps", "120 fps", "Max (monitor refresh rate)"], checked);
            with_app(|a| {
                hook::set_overlay(Some(a.overlay.hwnd));
                if let Some(i) = pick {
                    a.cfg.video_fps = VALUES[i];
                    a.refresh_panel();
                }
            });
        }
        overlay::Event::ResMenu(at) => {
            const VALUES: [Res; 3] = [Res::Original, Res::P1080, Res::P720];
            let Some((owner, cur)) = with_app(|a| (a.overlay.hwnd, Res::parse(&a.cfg.video_res))) else { return };
            let checked = VALUES.iter().position(|v| *v == cur).unwrap_or(0);
            hook::set_overlay(None);
            let pick = files::choice_menu(owner, (at.x, at.y), &["Original size", "1080p", "720p"], checked);
            with_app(|a| {
                hook::set_overlay(Some(a.overlay.hwnd));
                if let Some(i) = pick {
                    a.cfg.video_res = VALUES[i].as_str().into();
                    a.refresh_panel();
                }
            });
        }
    }
}

fn on_card_event(ev: card::Event) {
    let card::Event::Clicked(b, at) = ev;
    let Some(video) = with_app(|a| a.card_video) else { return };
    if b == Button::SaveTo {
        let Some(owner) = with_app(|a| a.card.hwnd) else { return };
        if let Some(folder) = choose_folder(owner, at) {
            with_app(|a| if video { a.move_video_to(folder) } else { a.save_last_to(folder) });
        }
        return;
    }
    with_app(|a| if video { a.video_card_click(b) } else { a.card_click(b) });
}

fn on_editor_event(ev: editor::Event) {
    match ev {
        editor::Event::Changed => {
            with_app(|a| unsafe {
                SetTimer(Some(a.msg), AUTOCOPY_TIMER, 150, None);
            });
        }
        editor::Event::Copy => {
            with_app(|a| a.editor_copy(true));
        }
        editor::Event::Save => {
            with_app(|a| a.editor_save(None));
        }
        editor::Event::SaveTo(at) => {
            let Some(owner) = with_app(|a| a.editor.as_ref().map(|e| e.hwnd)).flatten() else { return };
            if let Some(folder) = choose_folder(owner, at) {
                with_app(|a| a.editor_save(Some(folder)));
            }
        }
        editor::Event::CustomColor => {
            let Some((owner, start)) = with_app(|a| a.editor.as_ref().map(|e| (e.hwnd, e.custom))).flatten() else { return };
            if let Some(c) = pick_color(owner, start) {
                let ev = with_app(|a| {
                    a.editor.as_mut().and_then(|e| {
                        e.custom = c;
                        e.set_color(c)
                    })
                })
                .flatten();
                if let Some(ev) = ev {
                    on_editor_event(ev);
                }
                with_app(|a| {
                    if let Some(st) = a.editor.as_ref().map(|e| e.style()) {
                        a.remember_style(Some(st.0), st.1, st.2);
                    }
                });
            }
        }
        editor::Event::Style => {
            with_app(|a| {
                if let Some(st) = a.editor.as_ref().map(|e| e.style()) {
                    a.remember_style(Some(st.0), st.1, st.2);
                }
            });
        }
    }
}

/// Windows colour dialog. Runs with no borrow held.
fn pick_color(owner: HWND, start: Color) -> Option<Color> {
    use windows::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
    static mut CUSTOM: [COLORREF; 16] = [COLORREF(0x00FFFFFF); 16];
    let rgb = |c: Color| COLORREF(c.0 as u32 | (c.1 as u32) << 8 | (c.2 as u32) << 16);
    unsafe {
        let mut cc = CHOOSECOLORW {
            lStructSize: size_of::<CHOOSECOLORW>() as u32,
            hwndOwner: owner,
            rgbResult: rgb(start),
            lpCustColors: std::ptr::addr_of_mut!(CUSTOM) as *mut COLORREF,
            Flags: CC_RGBINIT | CC_FULLOPEN,
            ..Default::default()
        };
        if ChooseColorW(&mut cc).as_bool() {
            let v = cc.rgbResult.0;
            Some(Color(v as u8, (v >> 8) as u8, (v >> 16) as u8))
        } else {
            None
        }
    }
}

unsafe extern "system" fn editor_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        with_app(|a| {
            if a.editor.as_ref().is_some_and(|e| e.hwnd == hwnd) {
                a.editor = None;
            }
        });
        return LRESULT(0);
    }
    let r = with_app(|a| {
        let App { editor, gfx, .. } = a;
        editor.as_mut().filter(|e| e.hwnd == hwnd).map(|e| e.handle(gfx, msg, wp, lp))
    })
    .flatten();
    if let Some((res, ev)) = r {
        if let Some(ev) = ev {
            on_editor_event(ev);
        }
        if let Some(res) = res {
            return res;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn on_veditor_event(ev: veditor::Event) {
    match ev {
        veditor::Event::CustomColor => {
            let Some((owner, start)) = with_app(|a| a.veditor.as_ref().map(|e| (e.hwnd, e.custom))).flatten() else { return };
            if let Some(c) = pick_color(owner, start) {
                with_app(|a| {
                    if let Some(e) = a.veditor.as_mut() {
                        e.custom = c;
                        e.set_color(c);
                    }
                });
            }
        }
        veditor::Event::Exported(p) => log(&format!("video exported: {}", p.display())),
    }
}

/// Windows file dialog for a video to edit. Runs with no borrow held.
fn choose_video(owner: HWND) -> Option<PathBuf> {
    use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW};
    let filter: Vec<u16> = "Videos (*.mp4;*.mov;*.m4v;*.wmv)\0*.mp4;*.mov;*.m4v;*.wmv\0All files\0*.*\0\0".encode_utf16().collect();
    let mut buf = vec![0u16; 4096];
    let dir = super::util::wide(&with_app(|a| a.current_folder()).unwrap_or_default().to_string_lossy());
    unsafe {
        let mut ofn = OPENFILENAMEW {
            lStructSize: size_of::<OPENFILENAMEW>() as u32,
            hwndOwner: owner,
            lpstrFilter: windows::core::PCWSTR(filter.as_ptr()),
            lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
            nMaxFile: buf.len() as u32,
            lpstrInitialDir: windows::core::PCWSTR(dir.as_ptr()),
            Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST,
            ..Default::default()
        };
        if GetOpenFileNameW(&mut ofn).as_bool() {
            let n = buf.iter().position(|c| *c == 0).unwrap_or(0);
            return Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])));
        }
    }
    None
}

unsafe extern "system" fn veditor_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        with_app(|a| {
            if a.veditor.as_ref().is_some_and(|e| e.hwnd == hwnd) {
                if let Some(st) = a.veditor.as_ref().map(|e| e.style()) {
                    a.remember_style(Some(st.0), st.1, st.2);
                }
                a.veditor = None;
            }
        });
        return LRESULT(0);
    }
    let r = with_app(|a| {
        let App { veditor, gfx, .. } = a;
        veditor.as_mut().filter(|e| e.hwnd == hwnd).map(|e| e.handle(gfx, msg, wp, lp))
    })
    .flatten();
    if let Some((res, ev)) = r {
        if let Some(ev) = ev {
            on_veditor_event(ev);
        }
        if let Some(res) = res {
            return res;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn bar_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some((res, click, tick)) = with_app(|a| a.recbar.handle(msg, wp, lp)) {
        if tick {
            with_app(|a| a.bar_tick());
        }
        if let Some(b) = click {
            with_app(|a| a.bar_click(b));
        }
        if let Some(res) = res {
            return res;
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn outline_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(r) = RecBar::outline_proc(hwnd, msg) {
        return r;
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
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

/// WM_COPYDATA tag: the payload is a UTF-16 path to open in the video editor.
const COPY_EDIT: usize = 0x5245_4544;

unsafe extern "system" fn msg_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_COPYDATA => {
            let cds = unsafe { &*(lp.0 as *const windows::Win32::System::DataExchange::COPYDATASTRUCT) };
            if cds.dwData == COPY_EDIT && !cds.lpData.is_null() {
                let w = unsafe { std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2) };
                let p = PathBuf::from(String::from_utf16_lossy(w));
                // Open after returning, so the sender isn't kept waiting.
                PENDING_EDIT.with(|e| *e.borrow_mut() = Some(p));
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_APP_EDIT, WPARAM(0), LPARAM(0));
                }
                return LRESULT(1);
            }
        }
        m if m == WM_APP_EDIT => {
            if let Some(p) = PENDING_EDIT.with(|e| e.borrow_mut().take()) {
                with_app(|a| a.open_video_editor(p));
            }
            return LRESULT(0);
        }
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
                    tray::CMD_EDIT_VIDEO => {
                        if let Some(p) = choose_video(hwnd) {
                            with_app(|a| a.open_video_editor(p));
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
        WM_TIMER if wp.0 == AUTOCOPY_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), AUTOCOPY_TIMER);
            }
            with_app(|a| a.editor_copy(false));
            return LRESULT(0);
        }
        WM_APP_GIF => {
            let code = wp.0;
            with_app(|a| {
                let gif = a.last_video.as_ref().map(|v| v.path.with_extension("gif"));
                match code {
                    1000 => {
                        if let Some(v) = &mut a.last_video {
                            v.gif_busy = false;
                        }
                        let name = gif.as_deref().map(file_name).unwrap_or_default();
                        a.card.set_status(format!("GIF saved: {name}"));
                    }
                    1001 => {
                        if let Some(v) = &mut a.last_video {
                            v.gif_busy = false;
                        }
                        a.card.set_status("GIF export failed".into());
                    }
                    pct => a.card.set_status(format!("Making GIF... {pct}%")),
                }
            });
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

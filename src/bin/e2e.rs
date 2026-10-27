//! End-to-end test on a real Windows desktop (CI runner). Drives the app with
//! synthetic input exactly like a user and checks what lands on the clipboard.
//!
//! Usage: e2e <path-to-RengiSnap.exe> <results-dir>

#[cfg(not(windows))]
fn main() {
    eprintln!("e2e runs on Windows only");
}

#[cfg(windows)]
#[path = "../geom.rs"]
mod geom;
#[cfg(windows)]
#[path = "../image.rs"]
mod image;

#[cfg(windows)]
fn main() {
    std::process::exit(e2e::run());
}

#[cfg(windows)]
mod e2e {
    use super::geom::Rect;
    use super::image::Frame;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command};
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    const PAT: Rect = Rect::new(120, 100, 520, 400);
    /// Win32 window with a bordered child panel and a button inside it.
    const NEST: Rect = Rect::new(560, 60, 960, 360);
    /// Window with no children that only paints a card and a button.
    const PAINT: Rect = Rect::new(560, 390, 960, 690);
    const PAINT_CARD: Rect = Rect::new(30, 30, 330, 230);
    const PAINT_BUTTON: Rect = Rect::new(60, 60, 180, 100);
    /// BGR of the painted button.
    const BUTTON_BGR: [u8; 3] = [0xD0, 0x78, 0x20];

    struct Report {
        file: std::fs::File,
        failures: u32,
        dir: PathBuf,
    }

    impl Report {
        fn line(&mut self, s: &str) {
            println!("{s}");
            let _ = writeln!(self.file, "{s}");
        }
        fn check(&mut self, name: &str, ok: bool, detail: &str) {
            if !ok {
                self.failures += 1;
            }
            self.line(&format!("{} {name}{}", if ok { "PASS" } else { "FAIL" }, if detail.is_empty() { String::new() } else { format!(" - {detail}") }));
        }
    }

    fn pattern(x: i32, y: i32) -> [u8; 3] {
        // BGR, deterministic and varied so any offset or flip shows up.
        [((x * 7 + y * 3) & 255) as u8, ((x * 2 + y * 5) & 255) as u8, ((x ^ y) & 255) as u8]
    }

    fn sleep(ms: u64) {
        std::thread::sleep(Duration::from_millis(ms));
    }

    fn wait(timeout_ms: u64, mut f: impl FnMut() -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(timeout_ms) {
            if f() {
                return true;
            }
            sleep(10);
        }
        f()
    }

    fn find(class: windows::core::PCWSTR) -> Option<HWND> {
        unsafe { FindWindowW(class, None).ok() }
    }

    fn visible(class: windows::core::PCWSTR) -> bool {
        find(class).map(|h| unsafe { IsWindowVisible(h).as_bool() }).unwrap_or(false)
    }

    fn overlay_visible() -> bool {
        visible(w!("RengiSnapOverlay"))
    }

    fn key(vk: VIRTUAL_KEY, up: bool) {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: vk, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, ..Default::default() },
            },
        };
        unsafe {
            SendInput(&[input], size_of::<INPUT>() as i32);
        }
    }

    fn tap(vk: VIRTUAL_KEY) {
        key(vk, false);
        sleep(15);
        key(vk, true);
    }

    fn mouse(flags: MOUSE_EVENT_FLAGS) {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } },
        };
        unsafe {
            SendInput(&[input], size_of::<INPUT>() as i32);
        }
    }

    fn move_to(x: i32, y: i32) {
        unsafe {
            let _ = SetCursorPos(x, y);
        }
        sleep(8);
    }

    fn drag(from: (i32, i32), to: (i32, i32), steps: i32) {
        move_to(from.0, from.1);
        sleep(60);
        mouse(MOUSEEVENTF_LEFTDOWN);
        sleep(30);
        for i in 1..=steps {
            move_to(from.0 + (to.0 - from.0) * i / steps, from.1 + (to.1 - from.1) * i / steps);
        }
        sleep(40);
        mouse(MOUSEEVENTF_LEFTUP);
    }

    fn seq() -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    fn open_clip() -> bool {
        wait(2000, || unsafe { OpenClipboard(None).is_ok() })
    }

    fn global_bytes(h: HANDLE) -> Vec<u8> {
        unsafe {
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u8;
            if p.is_null() {
                return Vec::new();
            }
            let v = std::slice::from_raw_parts(p, GlobalSize(g)).to_vec();
            let _ = GlobalUnlock(g);
            v
        }
    }

    /// Reads CF_DIB (synthesized by Windows from our DIBV5) into a top-down frame.
    fn clip_image() -> Option<Frame> {
        if !open_clip() {
            return None;
        }
        let bytes = unsafe { GetClipboardData(CF_DIB.0 as u32).ok().map(global_bytes) };
        unsafe {
            let _ = CloseClipboard();
        }
        let b = bytes?;
        let rd = |o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let hsize = rd(0) as usize;
        let w = rd(4);
        let h = rd(8);
        let bpp = u16::from_le_bytes([b[14], b[15]]);
        let comp = rd(16);
        if bpp != 32 {
            return None;
        }
        let mut off = hsize;
        if comp == 3 && hsize == 40 {
            off += 12;
        }
        let (hh, bottom_up) = (h.abs(), h > 0);
        let stride = w as usize * 4;
        let mut pixels = Vec::with_capacity(stride * hh as usize);
        for row in 0..hh as usize {
            let src = if bottom_up { hh as usize - 1 - row } else { row };
            let s = off + src * stride;
            pixels.extend_from_slice(b.get(s..s + stride)?);
        }
        Some(Frame { rect: Rect::from_xywh(0, 0, w, hh), pixels })
    }

    fn clip_png_size() -> Option<(u32, u32)> {
        if !open_clip() {
            return None;
        }
        let bytes = unsafe {
            let fmt = RegisterClipboardFormatW(w!("PNG"));
            GetClipboardData(fmt).ok().map(global_bytes)
        };
        unsafe {
            let _ = CloseClipboard();
        }
        let dec = png::Decoder::new(std::io::Cursor::new(bytes?));
        let r = dec.read_info().ok()?;
        Some((r.info().width, r.info().height))
    }

    fn clip_text() -> Option<String> {
        if !open_clip() {
            return None;
        }
        let bytes = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32).ok().map(global_bytes) };
        unsafe {
            let _ = CloseClipboard();
        }
        let b = bytes?;
        let u: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
        Some(String::from_utf16_lossy(&u))
    }

    /// Compares a clipboard image against the pattern window area `area` (desktop coords).
    fn matches_pattern(img: &Frame, area: Rect) -> Result<(), String> {
        if img.width() as i32 != area.width() || img.height() as i32 != area.height() {
            return Err(format!("size {}x{}, expected {}x{}", img.width(), img.height(), area.width(), area.height()));
        }
        let mut bad = 0;
        let mut first = String::new();
        for y in 0..area.height() {
            for x in 0..area.width() {
                let got = img.pixel(x, y).unwrap();
                let want = pattern(area.left + x - PAT.left, area.top + y - PAT.top);
                if got != want {
                    if bad == 0 {
                        first = format!("first mismatch at ({x},{y}): got {got:?} want {want:?}");
                    }
                    bad += 1;
                }
            }
        }
        if bad == 0 { Ok(()) } else { Err(format!("{bad} pixels differ; {first}")) }
    }

    fn screenshot(dir: &Path, name: &str) {
        let r = unsafe {
            Rect::from_xywh(
                GetSystemMetrics(SM_XVIRTUALSCREEN),
                GetSystemMetrics(SM_YVIRTUALSCREEN),
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            )
        };
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, r.width(), r.height());
            let old = SelectObject(mem, bmp.into());
            let _ = BitBlt(mem, 0, 0, r.width(), r.height(), Some(screen), r.left, r.top, SRCCOPY | CAPTUREBLT);
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: r.width(),
                    biHeight: -r.height(),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; (r.width() * r.height() * 4) as usize];
            GetDIBits(mem, bmp, 0, r.height() as u32, Some(pixels.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            let f = Frame { rect: Rect::from_xywh(0, 0, r.width(), r.height()), pixels };
            let _ = std::fs::write(dir.join(format!("{name}.png")), f.encode_png());
        }
    }

    unsafe extern "system" fn pattern_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_PAINT {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let (w, h) = (PAT.width(), PAT.height());
                let mut px = Vec::with_capacity((w * h * 4) as usize);
                for y in 0..h {
                    for x in 0..w {
                        let p = pattern(x, y);
                        px.extend_from_slice(&[p[0], p[1], p[2], 0]);
                    }
                }
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h,
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                SetDIBitsToDevice(dc, 0, 0, w as u32, h as u32, 0, 0, 0, h as u32, px.as_ptr() as *const _, &info, DIB_RGB_COLORS);
                let _ = EndPaint(hwnd, &ps);
            }
            return LRESULT(0);
        }
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    /// Opens the pattern window on its own thread (with its own message loop).
    fn spawn_pattern_window() {
        std::thread::spawn(|| unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let inst: HINSTANCE = GetModuleHandleW(None).map(|m| m.into()).unwrap_or_default();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(pattern_proc),
                hInstance: inst,
                lpszClassName: w!("E2EPattern"),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST,
                w!("E2EPattern"),
                w!("pattern"),
                WS_POPUP | WS_VISIBLE,
                PAT.left,
                PAT.top,
                PAT.width(),
                PAT.height(),
                None,
                None,
                Some(inst),
                None,
            );
            if hwnd.is_err() {
                return;
            }
            let mut m = MSG::default();
            while GetMessageW(&mut m, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        });
    }

    fn launch(exe: &Path, home: &Path) -> std::io::Result<Child> {
        Command::new(exe)
            .env("RENGI_SNAP_HOME", home)
            .env("RENGI_SNAP_NO_AUTOSTART", "1")
            .env("RENGI_SNAP_DEBUG", "1")
            .spawn()
    }

    fn wheel(up: bool) {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { mouseData: if up { 120 } else { (-120i32) as u32 }, dwFlags: MOUSEEVENTF_WHEEL, ..Default::default() },
            },
        };
        unsafe {
            SendInput(&[input], size_of::<INPUT>() as i32);
        }
        sleep(60);
    }

    fn click() {
        mouse(MOUSEEVENTF_LEFTDOWN);
        sleep(40);
        mouse(MOUSEEVENTF_LEFTUP);
    }

    /// Opens the overlay, hovers `p`, lets elements arrive, applies the wheel
    /// steps (+1 up, -1 down), clicks, and returns the clipboard image.
    fn capture_at(p: (i32, i32), steps: &[i32], shot: Option<(&Path, &str)>) -> Option<Frame> {
        let s0 = seq();
        open_overlay()?;
        move_to(p.0 - 3, p.1 - 3);
        sleep(80);
        move_to(p.0, p.1);
        sleep(600);
        for &st in steps {
            wheel(st > 0);
        }
        sleep(150);
        if let Some((dir, name)) = shot {
            screenshot(dir, name);
        }
        click();
        if !wait(5000, || seq() != s0) {
            close_overlay_if_open();
            return None;
        }
        clip_image()
    }

    fn size_check(rep: &mut Report, name: &str, img: Option<&Frame>, want: Rect) {
        match img {
            Some(i) => {
                let ok = i.width() as i32 == want.width() && i.height() as i32 == want.height();
                rep.check(name, ok, &format!("got {}x{}, want {}x{}", i.width(), i.height(), want.width(), want.height()));
            }
            None => rep.check(name, false, "no capture"),
        }
    }

    unsafe extern "system" fn nest_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    unsafe extern "system" fn paint_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if msg == WM_PAINT {
            unsafe {
                let mut ps = PAINTSTRUCT::default();
                let dc = BeginPaint(hwnd, &mut ps);
                let fill = |r: Rect, bgr: [u8; 3]| {
                    let b = CreateSolidBrush(COLORREF(bgr[2] as u32 | (bgr[1] as u32) << 8 | (bgr[0] as u32) << 16));
                    let rr = RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
                    FillRect(dc, &rr, b);
                    let _ = DeleteObject(b.into());
                };
                fill(Rect::new(0, 0, PAINT.width(), PAINT.height()), [240, 240, 240]);
                let c = PAINT_CARD;
                fill(c, [60, 60, 60]);
                fill(Rect::new(c.left + 1, c.top + 1, c.right - 1, c.bottom - 1), [255, 255, 255]);
                fill(PAINT_BUTTON, BUTTON_BGR);
                SetBkMode(dc, TRANSPARENT);
                let mut text: Vec<u16> = "Some text inside the card, like a real app".encode_utf16().collect();
                let mut tr = RECT { left: 50, top: 130, right: 320, bottom: 160 };
                DrawTextW(dc, &mut text, &mut tr, DT_LEFT | DT_SINGLELINE);
                let _ = EndPaint(hwnd, &ps);
            }
            return LRESULT(0);
        }
        unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
    }

    /// Opens a topmost test window on its own thread; `setup` may create
    /// children and returns raw handles to report back.
    fn spawn_window(
        class: windows::core::PCWSTR,
        at: Rect,
        proc_: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
        setup: fn(HWND, HINSTANCE) -> Vec<isize>,
    ) -> Vec<isize> {
        let class = class.0 as usize;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let class = windows::core::PCWSTR(class as *const u16);
            let inst: HINSTANCE = GetModuleHandleW(None).map(|m| m.into()).unwrap_or_default();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(proc_),
                hInstance: inst,
                lpszClassName: class,
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as isize as *mut _),
                ..Default::default()
            };
            RegisterClassW(&wc);
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_TOPMOST,
                class,
                w!("test"),
                WS_POPUP | WS_VISIBLE | WS_CLIPCHILDREN,
                at.left,
                at.top,
                at.width(),
                at.height(),
                None,
                None,
                Some(inst),
                None,
            ) else {
                let _ = tx.send(Vec::new());
                return;
            };
            let mut handles = vec![hwnd.0 as isize];
            handles.extend(setup(hwnd, inst));
            let _ = tx.send(handles);
            let mut m = MSG::default();
            while GetMessageW(&mut m, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        });
        rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default()
    }

    fn nest_children(parent: HWND, inst: HINSTANCE) -> Vec<isize> {
        unsafe {
            let panel = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WS_BORDER | WS_CLIPCHILDREN,
                20,
                20,
                360,
                260,
                Some(parent),
                None,
                Some(inst),
                None,
            );
            let Ok(panel) = panel else { return Vec::new() };
            let button = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                w!("OK"),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(0), // BS_PUSHBUTTON = 0
                40,
                40,
                120,
                40,
                Some(panel),
                None,
                Some(inst),
                None,
            );
            match button {
                Ok(b) => vec![panel.0 as isize, b.0 as isize],
                Err(_) => vec![panel.0 as isize],
            }
        }
    }

    fn no_children(_: HWND, _: HINSTANCE) -> Vec<isize> {
        Vec::new()
    }

    fn window_rect(h: isize) -> Rect {
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(HWND(h as *mut _), &mut r);
        }
        Rect::new(r.left, r.top, r.right, r.bottom)
    }

    /// Presses the hotkey and waits for the overlay. Returns ms until it showed.
    fn open_overlay() -> Option<u128> {
        let t = Instant::now();
        tap(VK_SCROLL);
        wait(5000, overlay_visible).then(|| t.elapsed().as_millis())
    }

    fn close_overlay_if_open() {
        if overlay_visible() {
            tap(VK_ESCAPE);
            wait(2000, || !overlay_visible());
        }
    }

    pub fn run() -> i32 {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        let args: Vec<String> = std::env::args().collect();
        let exe = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "RengiSnap.exe".into()));
        let dir = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| "e2e-results".into()));
        let _ = std::fs::create_dir_all(&dir);
        let home = dir.join("home");
        let _ = std::fs::create_dir_all(&home);
        let mut rep = Report { file: std::fs::File::create(dir.join("e2e.txt")).unwrap(), failures: 0, dir: dir.clone() };
        let desk = unsafe { (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
        rep.line(&format!("desktop {}x{}", desk.0, desk.1));

        spawn_pattern_window();
        let nest = spawn_window(w!("E2ENested"), NEST, nest_proc, nest_children);
        let paint = spawn_window(w!("E2EPainted"), PAINT, paint_proc, no_children);
        rep.check("test windows created", nest.len() == 3 && paint.len() == 1, &format!("nest {} paint {}", nest.len(), paint.len()));
        sleep(500);
        let mut app = match launch(&exe, &home) {
            Ok(c) => c,
            Err(e) => {
                rep.check("launch", false, &e.to_string());
                return 1;
            }
        };
        let started = wait(10000, || find(w!("RengiSnapMsg")).is_some());
        rep.check("app starts and creates its window", started, "");
        if !started {
            let _ = app.kill();
            return 1;
        }
        sleep(500);
        let scroll_before = unsafe { GetKeyState(VK_SCROLL.0 as i32) } & 1;

        // A: drag a frame over part of the pattern.
        let s0 = seq();
        let t = open_overlay();
        rep.check("A hotkey opens overlay", t.is_some(), &t.map(|ms| format!("{ms} ms")).unwrap_or_default());
        if t.is_some() {
            move_to(PAT.left + 30, PAT.top + 30);
            sleep(150);
            screenshot(&rep.dir, "a1-overlay-hover");
            let from = (PAT.left + 50, PAT.top + 50);
            let to = (PAT.left + 250, PAT.top + 200);
            move_to(from.0, from.1);
            sleep(60);
            mouse(MOUSEEVENTF_LEFTDOWN);
            for i in 1..=10 {
                move_to(from.0 + (to.0 - from.0) * i / 12, from.1 + (to.1 - from.1) * i / 12);
            }
            sleep(120);
            screenshot(&rep.dir, "a2-overlay-dragging");
            move_to(to.0, to.1);
            sleep(40);
            mouse(MOUSEEVENTF_LEFTUP);
            let changed = wait(5000, || seq() != s0);
            rep.check("A clipboard updated", changed, "");
            sleep(150);
            rep.check("A overlay closed", wait(2000, || !overlay_visible()), "");
            match clip_image() {
                Some(img) => {
                    let r = matches_pattern(&img, Rect::new(from.0, from.1, to.0, to.1));
                    rep.check("A clipboard image is exactly the dragged area", r.is_ok(), &r.err().unwrap_or_default());
                }
                None => rep.check("A clipboard has an image", false, ""),
            }
            let png = clip_png_size();
            rep.check("A PNG format renders on demand", png == Some((200, 150)), &format!("{png:?}"));
            let card = wait(2000, || visible(w!("RengiSnapCard")));
            rep.check("A preview card shows", card, "");
            sleep(300);
            screenshot(&rep.dir, "a3-card");
        }
        close_overlay_if_open();

        // B: wheel to the window level (one below the monitor) and click.
        let img = capture_at((PAT.left + 180, PAT.top + 150), &[1, 1, 1, 1, 1, 1, 1, 1, 1, 1, -1], None);
        match img {
            Some(img) => {
                let r = matches_pattern(&img, PAT);
                rep.check("B window level captures the whole window", r.is_ok(), &r.err().unwrap_or_default());
            }
            None => rep.check("B capture", false, ""),
        }
        close_overlay_if_open();

        // I: child controls (HWND + UI Automation path).
        if nest.len() == 3 {
            let (panel, button) = (window_rect(nest[1]), window_rect(nest[2]));
            rep.line(&format!("nested: window {NEST:?} panel {panel:?} button {button:?}"));
            let c = (button.left + button.width() / 2, button.top + button.height() / 2);
            let img = capture_at(c, &[], Some((&rep.dir, "i1-button")));
            size_check(&mut rep, "I hover + click captures the button", img.as_ref(), button);
            close_overlay_if_open();
            let img = capture_at(c, &[1], Some((&rep.dir, "i2-panel")));
            size_check(&mut rep, "I wheel up once captures the panel", img.as_ref(), panel);
            close_overlay_if_open();
            let img = capture_at(c, &[1, 1], None);
            size_check(&mut rep, "I wheel up twice captures the window", img.as_ref(), NEST);
            close_overlay_if_open();
        }

        // J: pixels only (no child windows, nothing for UI Automation).
        let pb = PAINT_BUTTON.offset(PAINT.left, PAINT.top);
        let pc = PAINT_CARD.offset(PAINT.left, PAINT.top);
        let c = (pb.left + 20, pb.top + 10);
        let img = capture_at(c, &[], Some((&rep.dir, "j1-painted-button")));
        size_check(&mut rep, "J pixel box: button", img.as_ref(), pb);
        if let Some(i) = &img {
            let (w, h) = (i.width() as i32 - 1, i.height() as i32 - 1);
            let corners = [(0, 0), (w, 0), (0, h), (w, h)].iter().all(|&(x, y)| i.pixel(x, y) == Some(BUTTON_BGR));
            rep.check("J button capture is pixel exact (all corners are button color)", corners, "");
        }
        close_overlay_if_open();
        let img = capture_at(c, &[1], Some((&rep.dir, "j2-painted-card")));
        size_check(&mut rep, "J pixel box: card", img.as_ref(), pc);
        close_overlay_if_open();
        let img = capture_at(c, &[1, 1], None);
        size_check(&mut rep, "J pixel box: window", img.as_ref(), PAINT);
        close_overlay_if_open();

        // C: Esc cancels and leaves the clipboard alone.
        sleep(200);
        let s0 = seq();
        if open_overlay().is_some() {
            tap(VK_ESCAPE);
            rep.check("C Esc closes overlay", wait(2000, || !overlay_visible()), "");
            sleep(200);
            rep.check("C Esc leaves clipboard untouched", seq() == s0, "");
        } else {
            rep.check("C hotkey opens overlay", false, "");
        }

        // D: C copies the hex color under the cursor.
        let s0 = seq();
        if open_overlay().is_some() {
            move_to(PAT.left + 80, PAT.top + 80);
            sleep(150);
            tap(VK_C);
            rep.check("D clipboard updated", wait(3000, || seq() != s0), "");
            let p = pattern(80, 80);
            let want = format!("#{:02X}{:02X}{:02X}", p[2], p[1], p[0]);
            let got = clip_text();
            rep.check("D hex color copied", got.as_deref() == Some(want.as_str()), &format!("got {got:?} want {want}"));
        } else {
            rep.check("D hotkey opens overlay", false, "");
        }
        close_overlay_if_open();

        // E: Ctrl on release keeps the frame open; Enter confirms.
        let s0 = seq();
        if open_overlay().is_some() {
            key(VK_CONTROL, false);
            drag((PAT.left + 20, PAT.top + 20), (PAT.left + 120, PAT.top + 90), 8);
            sleep(60);
            key(VK_CONTROL, true);
            sleep(200);
            rep.check("E Ctrl-release keeps overlay open", overlay_visible() && seq() == s0, "");
            screenshot(&rep.dir, "e1-editing");
            tap(VK_RIGHT);
            tap(VK_RETURN);
            rep.check("E Enter confirms", wait(3000, || seq() != s0), "");
            match clip_image() {
                Some(img) => {
                    let r = matches_pattern(&img, Rect::new(PAT.left + 21, PAT.top + 20, PAT.left + 121, PAT.top + 90));
                    rep.check("E arrow nudged the frame by 1 px", r.is_ok(), &r.err().unwrap_or_default());
                }
                None => rep.check("E clipboard has an image", false, ""),
            }
        } else {
            rep.check("E hotkey opens overlay", false, "");
        }
        close_overlay_if_open();

        // F: a second launch triggers a capture in the running instance and exits.
        match launch(&exe, &home) {
            Ok(mut second) => {
                let exited = wait(5000, || matches!(second.try_wait(), Ok(Some(_))));
                rep.check("F second instance exits", exited, "");
                rep.check("F second instance opens the overlay", wait(3000, overlay_visible), "");
                close_overlay_if_open();
            }
            Err(e) => rep.check("F second launch", false, &e.to_string()),
        }

        // G: the card's Save button writes a PNG; the card then hides by itself.
        let pics = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Pictures").join("Rengi Snap");
        let count_png = || std::fs::read_dir(&pics).map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "png")).count()).unwrap_or(0);
        let before = count_png();
        let s0 = seq();
        if open_overlay().is_some() {
            move_to(PAT.left + 180, PAT.top + 150);
            sleep(150);
            mouse(MOUSEEVENTF_LEFTDOWN);
            sleep(40);
            mouse(MOUSEEVENTF_LEFTUP);
            wait(5000, || seq() != s0);
            if wait(2000, || visible(w!("RengiSnapCard"))) {
                let mut r = RECT::default();
                unsafe {
                    let _ = GetWindowRect(find(w!("RengiSnapCard")).unwrap(), &mut r);
                }
                move_to(r.left + 106, r.bottom - 29);
                sleep(150);
                mouse(MOUSEEVENTF_LEFTDOWN);
                sleep(40);
                mouse(MOUSEEVENTF_LEFTUP);
                let saved = wait(4000, || count_png() == before + 1);
                rep.check("G card Save writes a PNG", saved, &format!("{} -> {} in {}", before, count_png(), pics.display()));
                sleep(200);
                screenshot(&rep.dir, "g1-card-saved");
                move_to(10, 10);
                let hid = wait(9000, || !visible(w!("RengiSnapCard")));
                rep.check("G card hides itself after the mouse leaves", hid, "");
            } else {
                rep.check("G card shows", false, "");
            }
        } else {
            rep.check("G hotkey opens overlay", false, "");
        }

        // H: panel visuals (video hint, folder dropdown) for review.
        if open_overlay().is_some() {
            move_to(470, 36);
            sleep(200);
            screenshot(&rep.dir, "h1-video-hint");
            move_to(585, 36);
            sleep(100);
            mouse(MOUSEEVENTF_LEFTDOWN);
            sleep(40);
            mouse(MOUSEEVENTF_LEFTUP);
            sleep(500);
            screenshot(&rep.dir, "h2-folder-menu");
            tap(VK_ESCAPE);
            sleep(300);
            tap(VK_ESCAPE);
            rep.check("H Esc closes the folder menu and then the overlay", wait(2000, || !overlay_visible()), "");
        }
        close_overlay_if_open();

        // K: a real app (Explorer), screenshots only.
        for h in nest.iter().take(1).chain(paint.iter()) {
            unsafe {
                let _ = ShowWindow(HWND(*h as *mut _), SW_HIDE);
            }
        }
        if let Some(pw) = find(w!("E2EPattern")) {
            unsafe {
                let _ = ShowWindow(pw, SW_HIDE);
            }
        }
        let _ = Command::new("explorer.exe").arg("C:\\Windows").spawn();
        if wait(10000, || find(w!("CabinetWClass")).is_some()) {
            sleep(2500);
            let ex = find(w!("CabinetWClass")).unwrap();
            unsafe {
                let _ = SetWindowPos(ex, Some(HWND_TOP), 40, 70, 940, 600, SWP_SHOWWINDOW);
                let _ = SetForegroundWindow(ex);
            }
            sleep(1500);
            for (name, p) in [("k1-explorer-list", (560, 330)), ("k2-explorer-nav", (130, 330)), ("k3-explorer-toolbar", (300, 160))] {
                if open_overlay().is_some() {
                    move_to(p.0 - 3, p.1);
                    sleep(60);
                    move_to(p.0, p.1);
                    sleep(700);
                    screenshot(&rep.dir, &format!("{name}-0"));
                    wheel(true);
                    sleep(150);
                    screenshot(&rep.dir, &format!("{name}-1"));
                    wheel(true);
                    sleep(150);
                    screenshot(&rep.dir, &format!("{name}-2"));
                }
                close_overlay_if_open();
            }
        } else {
            rep.line("explorer window not found");
        }

        let scroll_after = unsafe { GetKeyState(VK_SCROLL.0 as i32) } & 1;
        rep.check("Scroll Lock state never toggled", scroll_before == scroll_after, "");
        rep.check("app still running", matches!(app.try_wait(), Ok(None)), "");

        // Timing sample.
        let mut times = Vec::new();
        for _ in 0..5 {
            sleep(150);
            if let Some(ms) = open_overlay() {
                times.push(ms);
            }
            close_overlay_if_open();
        }
        rep.line(&format!("hotkey-to-overlay ms: {times:?}"));
        let _ = app.kill();
        rep.line(&format!("failures: {}", rep.failures));
        if rep.failures == 0 { 0 } else { 1 }
    }
}

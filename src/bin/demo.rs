//! Films a product demo on a real Windows desktop (CI runner): sets the stage,
//! drives Rengi Snap with smooth human-like input and records the screen with
//! ffmpeg. Writes `raw.mp4`, `beats.txt` (what happened when) and `cursor.txt`
//! (cursor path) so the video can be edited elsewhere.
//!
//! Usage: demo <RengiSnap.exe> <page.html> <wallpaper.jpg> <out-dir>

#[cfg(not(windows))]
fn main() {
    eprintln!("demo runs on Windows only");
}

#[cfg(windows)]
fn main() {
    std::process::exit(demo::run());
}

#[cfg(windows)]
mod demo {
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Dwm::*;
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::Media::timeBeginPeriod;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{BOOL, PCWSTR, w};

    #[derive(Clone, Copy, Debug)]
    struct R {
        l: i32,
        t: i32,
        r: i32,
        b: i32,
    }

    impl R {
        fn w(&self) -> i32 {
            self.r - self.l
        }
        fn h(&self) -> i32 {
            self.b - self.t
        }
    }

    static T0: OnceLock<Instant> = OnceLock::new();
    static BEATS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    fn now_ms() -> i64 {
        T0.get().map(|t| t.elapsed().as_millis() as i64).unwrap_or(-1)
    }

    /// `kind` is one of: key, caption, mark, zoom.
    fn beat(kind: &str, text: &str) {
        let line = format!("{} {kind} {text}", now_ms());
        println!("{line}");
        BEATS.lock().unwrap().push(line);
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
            sleep(15);
        }
        f()
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn find(class: PCWSTR) -> Option<HWND> {
        unsafe { FindWindowW(class, None).ok() }
    }

    fn visible(class: PCWSTR) -> bool {
        find(class).map(|h| unsafe { IsWindowVisible(h).as_bool() }).unwrap_or(false)
    }

    fn window_rect(h: HWND) -> R {
        let mut r = RECT::default();
        unsafe {
            if DwmGetWindowAttribute(h, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut _ as *mut _, size_of::<RECT>() as u32).is_err() {
                let _ = GetWindowRect(h, &mut r);
            }
        }
        R { l: r.left, t: r.top, r: r.right, b: r.bottom }
    }

    fn client_screen(h: HWND) -> R {
        let mut r = RECT::default();
        let mut o = POINT::default();
        unsafe {
            let _ = GetClientRect(h, &mut r);
            let _ = ClientToScreen(h, &mut o);
        }
        R { l: o.x, t: o.y, r: o.x + r.right, b: o.y + r.bottom }
    }

    fn cursor() -> (i32, i32) {
        let mut p = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut p);
        }
        (p.x, p.y)
    }

    fn set_cursor(x: i32, y: i32) {
        unsafe {
            let _ = SetCursorPos(x, y);
        }
    }

    fn send_key(vk: VIRTUAL_KEY, up: bool) {
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

    fn tap(vk: VIRTUAL_KEY, label: &str) {
        if !label.is_empty() {
            beat("key", label);
        }
        send_key(vk, false);
        sleep(40);
        send_key(vk, true);
    }

    fn type_text(s: &str) {
        for ch in s.encode_utf16() {
            for up in [false, true] {
                let input = INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wScan: ch,
                            dwFlags: if up { KEYEVENTF_UNICODE | KEYEVENTF_KEYUP } else { KEYEVENTF_UNICODE },
                            ..Default::default()
                        },
                    },
                };
                unsafe {
                    SendInput(&[input], size_of::<INPUT>() as i32);
                }
                sleep(if up { 45 } else { 25 });
            }
        }
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

    fn wheel(up: bool) {
        beat("key", if up { "wheel-up" } else { "wheel-down" });
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT { mouseData: if up { 120 } else { (-120i32) as u32 }, dwFlags: MOUSEEVENTF_WHEEL, ..Default::default() },
            },
        };
        unsafe {
            SendInput(&[input], size_of::<INPUT>() as i32);
        }
    }

    /// Moves the cursor like a hand: eased, slightly curved, ~120 updates/s.
    fn glide(to: (i32, i32), ms: u64) {
        let from = cursor();
        let (dx, dy) = ((to.0 - from.0) as f64, (to.1 - from.1) as f64);
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 1.0 {
            return;
        }
        // Perpendicular bow, up to 6% of the distance.
        let bow = (dist * 0.06).min(40.0);
        let (nx, ny) = (-dy / dist * bow, dx / dist * bow);
        let start = Instant::now();
        let total = ms.max(1) as f64;
        loop {
            let t = (start.elapsed().as_millis() as f64 / total).min(1.0);
            let e = if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 };
            let arc = (std::f64::consts::PI * e).sin();
            let x = from.0 as f64 + dx * e + nx * arc;
            let y = from.1 as f64 + dy * e + ny * arc;
            set_cursor(x.round() as i32, y.round() as i32);
            if t >= 1.0 {
                break;
            }
            sleep(8);
        }
        set_cursor(to.0, to.1);
    }

    fn click() {
        mouse(MOUSEEVENTF_LEFTDOWN);
        sleep(70);
        mouse(MOUSEEVENTF_LEFTUP);
    }

    fn glide_click(to: (i32, i32), ms: u64) {
        glide(to, ms);
        sleep(140);
        click();
    }

    fn drag(from: (i32, i32), to: (i32, i32), ms: u64) {
        glide(from, 450);
        sleep(120);
        mouse(MOUSEEVENTF_LEFTDOWN);
        sleep(60);
        glide(to, ms);
        sleep(80);
        mouse(MOUSEEVENTF_LEFTUP);
    }

    fn files_with(dir: &Path, ext: &str) -> usize {
        std::fs::read_dir(dir)
            .map(|d| d.filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case(ext))).count())
            .unwrap_or(0)
    }

    fn screenshot(dir: &Path, name: &str) {
        unsafe {
            let (w, h) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, w, h);
            let old = SelectObject(mem, bmp.into());
            let _ = BitBlt(mem, 0, 0, w, h, Some(screen), 0, 0, SRCCOPY | CAPTUREBLT);
            let mut info = BITMAPINFO {
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
            let mut px = vec![0u8; (w * h * 4) as usize];
            GetDIBits(mem, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
            SelectObject(mem, old);
            let _ = DeleteObject(bmp.into());
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            for p in px.chunks_exact_mut(4) {
                p.swap(0, 2);
                p[3] = 255;
            }
            let Ok(f) = std::fs::File::create(dir.join(format!("{name}.png"))) else { return };
            let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w as u32, h as u32);
            enc.set_color(png::ColorType::Rgba);
            if let Ok(mut wr) = enc.write_header() {
                let _ = wr.write_image_data(&px);
            }
        }
    }

    // ---- stage ----

    fn autohide_taskbar() {
        unsafe {
            let mut d = APPBARDATA { cbSize: size_of::<APPBARDATA>() as u32, ..Default::default() };
            if let Some(h) = find(w!("Shell_TrayWnd")) {
                d.hWnd = h;
            }
            d.lParam = LPARAM(ABS_AUTOHIDE as isize);
            SHAppBarMessage(ABM_SETSTATE, &mut d);
        }
    }

    fn set_wallpaper(path: &Path) {
        let p = wide(&path.to_string_lossy());
        unsafe {
            let _ = SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, Some(p.as_ptr() as *mut _), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE);
        }
    }

    unsafe extern "system" fn minimize_one(h: HWND, _: LPARAM) -> BOOL {
        unsafe {
            if !IsWindowVisible(h).as_bool() {
                return true.into();
            }
            let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 || GetWindowTextLengthW(h) == 0 {
                return true.into();
            }
            let mut cls = [0u16; 64];
            let n = GetClassNameW(h, &mut cls);
            let cls = String::from_utf16_lossy(&cls[..n as usize]);
            if ["Shell_TrayWnd", "Progman", "WorkerW", "Shell_SecondaryTrayWnd"].contains(&cls.as_str()) {
                return true.into();
            }
            let _ = ShowWindow(h, SW_MINIMIZE);
        }
        true.into()
    }

    fn minimize_all() {
        unsafe {
            let _ = EnumWindows(Some(minimize_one), LPARAM(0));
        }
    }

    unsafe extern "system" fn find_render(h: HWND, out: LPARAM) -> BOOL {
        let mut cls = [0u16; 64];
        let n = unsafe { GetClassNameW(h, &mut cls) };
        if String::from_utf16_lossy(&cls[..n as usize]) == "Chrome_RenderWidgetHostHWND" {
            unsafe { *(out.0 as *mut isize) = h.0 as isize };
            return false.into();
        }
        true.into()
    }

    /// The page viewport on screen.
    fn viewport(top: HWND) -> R {
        let mut found: isize = 0;
        unsafe {
            let _ = EnumChildWindows(Some(top), Some(find_render), LPARAM(&mut found as *mut isize as isize));
        }
        if found != 0 {
            let h = HWND(found as *mut _);
            let mut r = RECT::default();
            unsafe {
                let _ = GetWindowRect(h, &mut r);
            }
            return R { l: r.left, t: r.top, r: r.right, b: r.bottom };
        }
        client_screen(top)
    }

    fn browser() -> Option<PathBuf> {
        [
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
        ]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.exists())
    }

    fn launch_page(page: &Path, profile: &Path) -> Option<Child> {
        let exe = browser()?;
        let url = format!("file:///{}", page.to_string_lossy().replace('\\', "/"));
        Command::new(exe)
            .arg(format!("--app={url}"))
            .arg(format!("--user-data-dir={}", profile.display()))
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--disable-features=Translate,OptimizationGuideModelDownloading,AutofillServerCommunication",
                "--window-position=160,64",
                "--window-size=1600,920",
                "--force-device-scale-factor=1",
                "--force-renderer-accessibility",
                "--hide-crash-restore-bubble",
                "--disable-session-crashed-bubble",
                "--disable-background-networking",
                "--no-service-autorun",
            ])
            .spawn()
            .ok()
    }

    fn page_window() -> Option<HWND> {
        unsafe { FindWindowW(None, w!("Fernwave - Overview")).ok() }
    }

    // ---- sync flash ----

    unsafe extern "system" fn flash_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        unsafe { DefWindowProcW(h, m, wp, lp) }
    }

    fn pump(ms: u64) {
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            sleep(5);
        }
    }

    /// Full-screen black for a moment; T0 = the moment it is on screen.
    fn flash() {
        unsafe {
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(flash_proc),
                hInstance: inst.into(),
                lpszClassName: w!("RengiDemoFlash"),
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                ..Default::default()
            };
            RegisterClassW(&wc);
            let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            let Ok(h) = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                w!("RengiDemoFlash"),
                w!(""),
                WS_POPUP | WS_VISIBLE,
                0,
                0,
                sw,
                sh,
                None,
                None,
                Some(inst.into()),
                None,
            ) else {
                let _ = T0.set(Instant::now());
                return;
            };
            let _ = UpdateWindow(h);
            pump(60);
            let _ = DwmFlush();
            let _ = T0.set(Instant::now());
            beat("mark", "flash");
            pump(400);
            let _ = DestroyWindow(h);
            pump(150);
        }
    }

    pub fn run() -> i32 {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            timeBeginPeriod(1);
        }
        let a: Vec<String> = std::env::args().collect();
        if a.len() < 5 {
            eprintln!("usage: demo <RengiSnap.exe> <page.html> <wallpaper.jpg> <out-dir>");
            return 2;
        }
        let exe = std::fs::canonicalize(&a[1]).unwrap_or(PathBuf::from(&a[1]));
        let page = std::fs::canonicalize(&a[2]).unwrap_or(PathBuf::from(&a[2]));
        let wall = std::fs::canonicalize(&a[3]).unwrap_or(PathBuf::from(&a[3]));
        let out = PathBuf::from(&a[4]);
        let _ = std::fs::create_dir_all(&out);
        let out = std::fs::canonicalize(&out).unwrap_or(out);
        let mut log = std::fs::File::create(out.join("demo-log.txt")).unwrap();
        let mut note = |s: String| {
            println!("{s}");
            let _ = writeln!(log, "{s}");
        };
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        note(format!("screen {sw}x{sh}"));

        // Stage.
        autohide_taskbar();
        set_wallpaper(&wall);
        minimize_all();
        sleep(800);
        let profile = out.join("chrome-profile");
        let Some(mut chrome) = launch_page(&page, &profile) else {
            note("no browser".into());
            return 1;
        };
        if !wait(20000, || page_window().is_some()) {
            note("page window did not appear".into());
            screenshot(&out, "fail-page");
            let _ = chrome.kill();
            return 1;
        }
        sleep(2500);
        let top = page_window().unwrap();
        unsafe {
            let _ = SetForegroundWindow(top);
        }
        let vp = viewport(top);
        note(format!("window {:?} viewport {vp:?}", window_rect(top)));
        let page_at = |x: i32, y: i32| (vp.l + x, vp.t + y);

        let home = out.join("home");
        let _ = std::fs::create_dir_all(&home);
        let _ = std::fs::write(home.join("config.toml"), "video_fps = 120\ncard_seconds = 12\nloupe = true\n");
        let pics = std::env::var("USERPROFILE").map(|p| PathBuf::from(p).join("Pictures").join("Rengi Snap")).unwrap_or_default();
        let mut app = match Command::new(&exe)
            .env("RENGI_SNAP_HOME", &home)
            .env("RENGI_SNAP_NO_AUTOSTART", "1")
            .env("RENGI_SNAP_BAR_VISIBLE", "1")
            .env("RENGI_SNAP_DEBUG", "1")
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                note(format!("app: {e}"));
                return 1;
            }
        };
        if !wait(10000, || find(w!("RengiSnapMsg")).is_some()) {
            note("app did not start".into());
            return 1;
        }
        sleep(1500);
        set_cursor(sw / 2 + 120, sh - 260);
        screenshot(&out, "s0-stage");

        // Camera on.
        let raw = out.join("raw.mp4");
        let mut ff = match Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "gdigrab", "-framerate", "60", "-draw_mouse", "1", "-i", "desktop"])
            .args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "12", "-pix_fmt", "yuv420p"])
            .arg(&raw)
            .stdin(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                note(format!("ffmpeg: {e}"));
                return 1;
            }
        };
        wait(10000, || raw.metadata().map(|m| m.len() > 0).unwrap_or(false));
        sleep(1500);
        flash();

        // Cursor path sampler.
        let stop = Arc::new(AtomicBool::new(false));
        let path = Arc::new(Mutex::new(Vec::<(i64, i32, i32)>::new()));
        let sampler = {
            let (stop, path) = (stop.clone(), path.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    let (x, y) = cursor();
                    path.lock().unwrap().push((now_ms(), x, y));
                    sleep(10);
                }
            })
        };

        scenario(&out, &pics, page_at, vp, &mut note);

        // Camera off.
        sleep(800);
        stop.store(true, Ordering::Relaxed);
        let _ = sampler.join();
        if let Some(mut si) = ff.stdin.take() {
            let _ = si.write_all(b"q\n");
        }
        let ok = wait(30000, || matches!(ff.try_wait(), Ok(Some(_))));
        note(format!("ffmpeg stopped: {ok}"));
        if !ok {
            let _ = ff.kill();
        }
        let _ = std::fs::write(out.join("beats.txt"), BEATS.lock().unwrap().join("\n"));
        let cur: Vec<String> = path.lock().unwrap().iter().map(|(t, x, y)| format!("{t} {x} {y}")).collect();
        let _ = std::fs::write(out.join("cursor.txt"), cur.join("\n"));
        let _ = std::fs::write(out.join("layout.txt"), format!("screen {sw} {sh}\nviewport {} {} {} {}\n", vp.l, vp.t, vp.r, vp.b));
        let _ = app.kill();
        let _ = chrome.kill();
        0
    }

    fn scenario(out: &Path, pics: &Path, page_at: impl Fn(i32, i32) -> (i32, i32), vp: R, note: &mut dyn FnMut(String)) {
        let (sw, _) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        sleep(1200);

        // 1. Freeze.
        beat("caption", "Press Scroll Lock. The screen freezes.");
        glide(page_at(520, 470), 900);
        sleep(300);
        tap(VK_SCROLL, "Scroll Lock");
        if !wait(4000, || visible(w!("RengiSnapOverlay"))) {
            note("overlay did not open".into());
            return;
        }
        sleep(900);

        // 2. Smart regions.
        beat("caption", "It finds every element under the cursor");
        glide(page_at(380, 250), 700);
        sleep(900);
        glide(page_at(720, 260), 650);
        sleep(800);
        glide(page_at(1060, 250), 650);
        sleep(800);
        glide(page_at(1400, 300), 700);
        sleep(900);
        glide(page_at(700, 620), 800);
        sleep(900);
        screenshot(out, "s1-hover-traffic");
        beat("caption", "Mouse wheel picks the level");
        wheel(true);
        sleep(800);
        wheel(true);
        sleep(800);
        screenshot(out, "s2-wheel-up");
        wheel(false);
        sleep(700);
        wheel(false);
        sleep(900);

        // 3. Quick marks: hold Ctrl and drag a frame over the page.
        beat("caption", "Hold Ctrl and mark it up right there");
        glide(page_at(4, 4), 900);
        sleep(300);
        beat("key", "Ctrl");
        send_key(VK_CONTROL, false);
        sleep(150);
        drag(page_at(4, 4), page_at(vp.w() - 4, vp.h() - 4), 1100);
        sleep(120);
        send_key(VK_CONTROL, true);
        sleep(700);
        screenshot(out, "s3-quick");
        tap(VK_R, "R");
        sleep(300);
        drag(page_at(1240, 390), page_at(1556, 434), 650);
        sleep(500);
        tap(VK_A, "A");
        sleep(300);
        drag(page_at(1300, 660), page_at(1385, 446), 600);
        sleep(500);
        tap(VK_N, "N");
        sleep(300);
        glide_click(page_at(452, 208), 700);
        sleep(450);
        glide_click(page_at(334, 512), 700);
        sleep(700);
        screenshot(out, "s4-marked");
        tap(VK_RETURN, "Enter");
        beat("caption", "Copied to the clipboard");
        sleep(1400);

        // 4. Editor from the card.
        if !wait(3000, || visible(w!("RengiSnapCard"))) {
            note("card did not show".into());
            return;
        }
        let card = window_rect(find(w!("RengiSnapCard")).unwrap());
        note(format!("card {card:?}"));
        beat("caption", "Need more? Open the editor");
        glide_click((card.l + 10 + 64 * 4 + 32, card.b - 29), 1000);
        if !wait(4000, || visible(w!("RengiSnapEditor"))) {
            note("editor did not open".into());
            return;
        }
        sleep(1000);
        let ed = find(w!("RengiSnapEditor")).unwrap();
        let cl = client_screen(ed);
        let (iw, ih) = ((vp.w() - 8) as f64, (vp.h() - 8) as f64);
        let bar = 52.0;
        let pad = 24.0;
        let zoom = ((cl.w() as f64 - pad * 2.0) / iw).min((cl.h() as f64 - bar - pad * 2.0) / ih).min(1.0);
        let ox = ((cl.w() as f64 - iw * zoom) / 2.0).round();
        let oy = (bar + (cl.h() as f64 - bar - ih * zoom) / 2.0).round();
        // Page point -> editor screen point (the image starts at page (4,4)).
        let img = |x: i32, y: i32| (cl.l + (ox + (x - 4) as f64 * zoom).round() as i32, cl.t + (oy + (y - 4) as f64 * zoom).round() as i32);
        note(format!("editor client {cl:?} zoom {zoom:.3} origin ({ox},{oy})"));
        screenshot(out, "s5-editor");

        beat("caption", "Blur anything private");
        tap(VK_B, "B");
        sleep(300);
        drag(img(1398, 28), img(1584, 56), 700);
        sleep(700);
        beat("caption", "Add a note");
        tap(VK_T, "T");
        sleep(300);
        glide_click(img(1252, 560), 800);
        sleep(300);
        type_text("Failing for 2 h, on it");
        sleep(400);
        tap(VK_ESCAPE, "");
        sleep(800);
        beat("caption", "Every change is already in your clipboard");
        glide(img(900, 760), 900);
        sleep(1600);
        screenshot(out, "s6-edited");
        unsafe {
            let _ = PostMessageW(Some(ed), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        wait(3000, || find(w!("RengiSnapEditor")).is_none());
        sleep(900);

        // 5. Video.
        beat("caption", "Video too: up to 120+ fps");
        tap(VK_SCROLL, "Scroll Lock");
        if !wait(4000, || visible(w!("RengiSnapOverlay"))) {
            note("overlay 2 did not open".into());
            return;
        }
        sleep(600);
        glide_click((sw / 2 - 42, 36), 900);
        sleep(900);
        screenshot(out, "s7-video-mode");
        glide(page_at(560, 610), 900);
        sleep(1000);
        click();
        beat("mark", "recording");
        if !wait(4000, || visible(w!("RengiSnapBar"))) {
            note("recording bar did not show".into());
        }
        sleep(1200);
        screenshot(out, "s8-recording");
        glide(page_at(820, 700), 1600);
        sleep(1600);
        let mp4s = files_with(pics, "mp4");
        tap(VK_SCROLL, "Scroll Lock");
        beat("caption", "Saved as MP4");
        wait(8000, || files_with(pics, "mp4") > mp4s && !visible(w!("RengiSnapBar")));
        if !wait(3000, || visible(w!("RengiSnapCard"))) {
            note("video card did not show".into());
            return;
        }
        sleep(900);
        let card = window_rect(find(w!("RengiSnapCard")).unwrap());
        note(format!("video card {card:?}"));
        screenshot(out, "s9-video-card");
        beat("caption", "One click to GIF");
        let gifs = files_with(pics, "gif");
        glide_click((card.l + 290, card.b - 29), 1000);
        let made = wait(30000, || files_with(pics, "gif") > gifs);
        beat("mark", "gif-done");
        note(format!("gif made: {made}"));
        sleep(2200);
        screenshot(out, "s10-gif-done");
        beat("mark", "end");
    }
}

//! The editor's Cut out tool: downloads the models once, keeps a worker
//! thread that owns the AI engine, and draws the hover glow, the dimmed
//! "cut" look and the result bar.

use super::d2d::{self, Gfx, rect_f};
use super::download;
use super::theme::{self, Backdrop};
use crate::cutout::{self, Rgba};
use crate::palette::Rgba as Col;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::IDWriteTextFormat;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// The worker has news (download progress, the picture is ready, a mask).
pub const WM_CUT: u32 = WM_APP + 60;

const BASE: &str = "https://github.com/MRRengi/rng-snap/releases/download/cutout-models-v1/";

struct Asset {
    name: &'static str,
    sha256: &'static str,
    size: u64,
}

const ASSETS: [Asset; 3] = [
    Asset { name: cutout::ENCODER, sha256: "af81abe3b6ed882efef7700a1d3187a15e4104be62c89b8b33baa19965184461", size: 28_016_465 },
    Asset { name: cutout::DECODER, sha256: "43c7655a81b62c0f2b0e31d2bc91d3811b48d7f736ecf93b3a4a1254318241cf", size: 16_496_933 },
    Asset { name: "onnxruntime.dll", sha256: "579b636403983254346a5c1d80bd28f1519cd1e284cd204f8d4ff41f8d711559", size: 12_418_080 },
];

pub fn download_mb() -> u64 {
    (ASSETS.iter().map(|a| a.size).sum::<u64>() + 500_000) / 1_000_000
}

/// `%LOCALAPPDATA%\Snap\models` (or `RENGI_SNAP_MODELS`).
pub fn models_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("RENGI_SNAP_MODELS") {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Snap").join("models")
}

fn have_models() -> bool {
    let dir = models_dir();
    ASSETS.iter().all(|a| std::fs::metadata(dir.join(a.name)).is_ok_and(|m| m.len() == a.size))
}

#[derive(Clone, PartialEq, Debug)]
pub enum Phase {
    /// The models are not on this PC yet.
    NeedModel,
    Downloading,
    /// Starting the engine / looking at the picture.
    Working,
    Ready,
    Failed(String),
}

/// A mask for the picture at a given output size.
pub struct Mask {
    pub hover: bool,
    pub w: usize,
    pub h: usize,
    pub alpha: Vec<u8>,
    /// Full-resolution alpha (clicks only), for the actual cut.
    pub full: Option<Vec<u8>>,
}

struct Shared {
    phase: Mutex<Phase>,
    done: AtomicU64,
    cancel: AtomicBool,
    results: Mutex<Vec<Mask>>,
}

/// The picture being cut, top-down BGRA.
#[derive(Clone)]
pub struct Picture {
    pub bgra: Arc<Vec<u8>>,
    pub w: usize,
    pub h: usize,
}

enum Cmd {
    Start { pic: Picture, download: bool },
    Predict { points: Vec<(f32, f32, bool)>, hover: bool, out: (usize, usize) },
}

pub struct Worker {
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
}

impl Worker {
    pub fn start(notify: HWND) -> Worker {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared { phase: Mutex::new(if have_models() { Phase::Working } else { Phase::NeedModel }), done: AtomicU64::new(0), cancel: AtomicBool::new(false), results: Mutex::new(Vec::new()) });
        let sh = shared.clone();
        let hw = notify.0 as isize;
        std::thread::spawn(move || run(rx, sh, hw));
        Worker { tx, shared }
    }

    pub fn phase(&self) -> Phase {
        self.shared.phase.lock().map(|p| p.clone()).unwrap_or(Phase::Failed("busy".into()))
    }

    /// Downloaded bytes and the total.
    pub fn progress(&self) -> (u64, u64) {
        (self.shared.done.load(Ordering::Acquire), ASSETS.iter().map(|a| a.size).sum())
    }

    /// Look at a (new) picture; `download` allows fetching the models.
    pub fn picture(&self, pic: Picture, download: bool) {
        let _ = self.tx.send(Cmd::Start { pic, download });
    }

    pub fn predict(&self, id: u64, points: Vec<(f32, f32, bool)>, hover: bool, out: (usize, usize)) {
        let _ = id;
        let _ = self.tx.send(Cmd::Predict { points, hover, out });
    }

    pub fn take(&self) -> Option<Mask> {
        self.shared.results.lock().ok().and_then(|mut r| (!r.is_empty()).then(|| r.remove(0)))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.cancel.store(true, Ordering::Release);
    }
}

fn set(sh: &Shared, p: Phase, hw: isize) {
    if let Phase::Failed(e) = &p {
        super::util::log(&format!("cut out: {e}"));
    }
    if let Ok(mut g) = sh.phase.lock() {
        *g = p;
    }
    post(hw);
}

fn post(hw: isize) {
    unsafe {
        let _ = PostMessageW(Some(HWND(hw as *mut _)), WM_CUT, WPARAM(0), LPARAM(0));
    }
}

fn run(rx: Receiver<Cmd>, sh: Arc<Shared>, hw: isize) {
    let mut emb: Option<(Vec<f32>, f32, Picture)> = None;
    let dir = models_dir();
    while let Ok(first) = rx.recv() {
        // Only the newest hover matters; starts and clicks always run.
        let mut cmds = vec![first];
        while let Ok(c) = rx.try_recv() {
            cmds.push(c);
        }
        let last_hover = cmds.iter().rposition(|c| matches!(c, Cmd::Predict { hover: true, .. }));
        for (i, cmd) in cmds.into_iter().enumerate() {
            if sh.cancel.load(Ordering::Acquire) {
                return;
            }
            match cmd {
                Cmd::Start { pic, download } => {
                    emb = None;
                    if !have_models() {
                        if !download {
                            set(&sh, Phase::NeedModel, hw);
                            continue;
                        }
                        set(&sh, Phase::Downloading, hw);
                        let _ = std::fs::create_dir_all(&dir);
                        sh.done.store(0, Ordering::Release);
                        let mut failed = None;
                        for a in &ASSETS {
                            let dest = dir.join(a.name);
                            if std::fs::metadata(&dest).is_ok_and(|m| m.len() == a.size) {
                                sh.done.fetch_add(a.size, Ordering::AcqRel);
                                continue;
                            }
                            // Progress posts while bytes arrive.
                            let done = &sh.done;
                            let stop = Arc::new(AtomicBool::new(false));
                            let ticker = {
                                let stop = stop.clone();
                                std::thread::spawn(move || {
                                    while !stop.load(Ordering::Acquire) {
                                        post(hw);
                                        std::thread::sleep(std::time::Duration::from_millis(150));
                                    }
                                })
                            };
                            let r = download::fetch(&format!("{BASE}{}", a.name), &dest, a.sha256, done, &sh.cancel);
                            stop.store(true, Ordering::Release);
                            let _ = ticker.join();
                            if let Err(e) = r {
                                failed = Some(format!("Download failed: {e}"));
                                break;
                            }
                        }
                        if let Some(f) = failed {
                            set(&sh, Phase::Failed(f), hw);
                            continue;
                        }
                    }
                    set(&sh, Phase::Working, hw);
                    let eng = match cutout::engine(&dir, &dir.join("onnxruntime.dll")) {
                        Ok(e) => e,
                        Err(e) => {
                            set(&sh, Phase::Failed(format!("Cut out can't start: {e}")), hw);
                            continue;
                        }
                    };
                    let p = cutout::prepare(&pic.bgra, pic.w, pic.h);
                    let r = eng.lock().map_err(|_| "busy".to_string()).and_then(|mut g| g.embed(&p));
                    match r {
                        Ok(v) => {
                            emb = Some((v, p.scale, pic));
                            set(&sh, Phase::Ready, hw);
                        }
                        Err(e) => set(&sh, Phase::Failed(format!("Cut out failed: {e}")), hw),
                    }
                }
                Cmd::Predict { points, hover, out } => {
                    if hover && Some(i) != last_hover {
                        continue;
                    }
                    let Some((v, scale, pic)) = &emb else { continue };
                    let Ok(eng) = cutout::engine(&dir, &dir.join("onnxruntime.dll")) else { continue };
                    let Ok(m) = eng.lock().map_err(|_| ()).and_then(|mut g| g.predict(v, *scale, &points).map_err(|_| ())) else { continue };
                    let k = cutout::choose(&m);
                    let (ow, oh) = out;
                    let fx = ow as f32 / pic.w as f32;
                    let fy = oh as f32 / pic.h as f32;
                    let keep: Vec<(usize, usize)> = points.iter().filter(|p| p.2).map(|p| ((p.0 * fx) as usize, (p.1 * fy) as usize)).collect();
                    let mut a = cutout::alpha(&cutout::upscale(m.mask(k), *scale * pic.w as f32 / ow as f32, ow, oh));
                    cutout::tidy(&mut a, ow, oh, &keep);
                    let full = (!hover).then(|| {
                        let seeds: Vec<(usize, usize)> = points.iter().filter(|p| p.2).map(|p| (p.0 as usize, p.1 as usize)).collect();
                        let mut f = cutout::alpha(&cutout::upscale(m.mask(k), *scale, pic.w, pic.h));
                        cutout::tidy(&mut f, pic.w, pic.h, &seeds);
                        f
                    });
                    if let Ok(mut r) = sh.results.lock() {
                        if hover {
                            r.retain(|m| !m.hover);
                        }
                        r.push(Mask { hover, w: ow, h: oh, alpha: a, full });
                    }
                    post(hw);
                }
            }
        }
    }
}

/// What the Cut out controls on screen do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CutBtn {
    Download,
    NotNow,
    Retry,
    Copy,
    Sticker,
    Save,
    Clear,
}

/// A mask on screen with its bitmaps (made on first draw).
pub struct Shown {
    pub mask: Mask,
    pub glow: Option<ID2D1Bitmap>,
    pub edge: Option<ID2D1Bitmap>,
    /// When the shimmer started (a new object).
    pub since: Instant,
}

impl Shown {
    pub fn new(mask: Mask, fresh: bool) -> Shown {
        let since = if fresh { Instant::now() } else { Instant::now() - std::time::Duration::from_secs(5) };
        Shown { mask, glow: None, edge: None, since }
    }

    /// Bounding box in 0..1 picture units.
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let m = &self.mask;
        cutout::bbox(&m.alpha, m.w, m.h).map(|(a, b, c, d)| (a as f32 / m.w as f32, b as f32 / m.h as f32, c as f32 / m.w as f32, d as f32 / m.h as f32))
    }

    pub fn shimmer(&self) -> f32 {
        self.since.elapsed().as_secs_f32() / 0.9
    }
}

/// Editor-side state of the Cut out tool.
pub struct CutState {
    pub worker: Worker,
    /// Canvas version the picture was last checked against.
    pub checked: Option<u64>,
    pub pic: Option<Picture>,
    pub next_id: u64,
    pub hover_at: Option<(i32, i32)>,
    pub hover: Option<Shown>,
    /// Clicked points (picture coords, keep).
    pub points: Vec<(f32, f32, bool)>,
    pub cut: Option<Shown>,
    pub sticker: bool,
    pub buttons: Vec<(CutBtn, D2D_RECT_F)>,
    pub hot: Option<CutBtn>,
    pub started: Instant,
    pub note: Option<(String, Instant)>,
}

impl CutState {
    pub fn new(notify: HWND) -> CutState {
        CutState {
            worker: Worker::start(notify),
            checked: None,
            pic: None,
            next_id: 1,
            hover_at: None,
            hover: None,
            points: Vec::new(),
            cut: None,
            sticker: false,
            buttons: Vec::new(),
            hot: None,
            started: Instant::now(),
            note: None,
        }
    }

    /// The finished cutout (full resolution), sticker-styled if on.
    pub fn result(&self) -> Option<Rgba> {
        let m = &self.cut.as_ref()?.mask;
        let pic = self.pic.as_ref()?;
        let full = m.full.as_ref()?;
        let c = cutout::cut(&pic.bgra, pic.w, pic.h, full, 2)?;
        Some(if self.sticker { cutout::sticker(&c) } else { c })
    }

    pub fn clear(&mut self) {
        self.points.clear();
        self.cut = None;
        self.hover = None;
        self.hover_at = None;
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<CutBtn> {
        self.buttons.iter().find(|(_, r)| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map(|(b, _)| *b)
    }

    pub fn animating(&self) -> bool {
        let shimmering = |s: &Option<Shown>| s.as_ref().is_some_and(|s| s.shimmer() < 1.0);
        shimmering(&self.hover) || shimmering(&self.cut) || self.worker.phase() == Phase::Working
    }
}

/// Glow bitmap for a mask: a soft white band on the outline and a faint
/// fill; with `dim_outside` everything else is darkened (the "lifted" look).
/// `thick` widens the band (the sticker's white outline).
pub fn glow_bitmap(rt: &ID2D1RenderTarget, m: &Mask, dim_outside: bool, thick: bool) -> Option<ID2D1Bitmap> {
    let base = (m.w.max(m.h) as f32 / 300.0).clamp(2.0, 5.0);
    let band = cutout::edge(&m.alpha, m.w, m.h, if thick { base * 2.2 } else { base * 1.3 });
    // A wider, softer halo around the band so the outline reads as light.
    let halo = cutout::edge(&m.alpha, m.w, m.h, base * 4.0);
    let mut px = Vec::with_capacity(m.w * m.h * 4);
    for i in 0..m.w * m.h {
        let a = m.alpha[i] as f32 / 255.0;
        let e = band[i] as f32 / 255.0;
        let g = (halo[i] as f32 / 255.0).powi(2) * 0.35;
        let lift = if dim_outside { 0.0 } else { 0.12 * a };
        let white = (e.max(g) + lift * (1.0 - e)).min(1.0);
        let dim = if dim_outside { 0.55 * (1.0 - a) * (1.0 - e.max(g)) } else { 0.0 };
        let out_a = white + dim * (1.0 - white);
        let v = (white * 255.0).round() as u8; // premultiplied: white over black
        px.extend_from_slice(&[v, v, v, (out_a * 255.0).round() as u8]);
    }
    d2d::bitmap_premul(rt, m.w as u32, m.h as u32, &px).ok()
}

/// Just the outline band, used as the mask for the light sweep.
pub fn edge_bitmap(rt: &ID2D1RenderTarget, m: &Mask) -> Option<ID2D1Bitmap> {
    let r = (m.w.max(m.h) as f32 / 300.0).clamp(2.0, 5.0) * 1.6;
    let edge = cutout::edge(&m.alpha, m.w, m.h, r);
    let px: Vec<u8> = edge.iter().flat_map(|&e| [e, e, e, e]).collect();
    d2d::bitmap_premul(rt, m.w as u32, m.h as u32, &px).ok()
}

/// Apple's shimmer: one sweep of light along the outline, `k` from 0 to 1.
pub fn draw_sweep(rt: &ID2D1RenderTarget, edge: &ID2D1Bitmap, dst: D2D_RECT_F, k: f32) {
    if !(0.0..1.0).contains(&k) {
        return;
    }
    let ease = 1.0 - (1.0 - k).powi(3);
    unsafe {
        let w = dst.right - dst.left;
        let h = dst.bottom - dst.top;
        let x = dst.left - w * 0.3 + (w * 1.6) * ease;
        let fade = (1.0 - k).min(k * 6.0).min(1.0);
        let stops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: d2d::rgba(0xFFFFFF, 0.0) },
            D2D1_GRADIENT_STOP { position: 0.5, color: d2d::rgba(0xFFFFFF, fade) },
            D2D1_GRADIENT_STOP { position: 1.0, color: d2d::rgba(0xFFFFFF, 0.0) },
        ];
        let Ok(coll) = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { return };
        let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
            startPoint: windows_numerics::Vector2 { X: x - w * 0.18, Y: dst.top },
            endPoint: windows_numerics::Vector2 { X: x + w * 0.18, Y: dst.top + h * 0.3 },
        };
        if let Ok(g) = rt.CreateLinearGradientBrush(&props, None, &coll) {
            let mode = rt.GetAntialiasMode();
            rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
            rt.FillOpacityMask(edge, &g, D2D1_OPACITY_MASK_CONTENT_GRAPHICS, Some(&dst), None);
            rt.SetAntialiasMode(mode);
        }
    }
}

pub struct Fonts<'a> {
    pub title: &'a IDWriteTextFormat,
    pub body: &'a IDWriteTextFormat,
    pub label: &'a IDWriteTextFormat,
}

fn button(rt: &ID2D1RenderTarget, f: &IDWriteTextFormat, gfx: &Gfx, r: D2D_RECT_F, text: &str, prominent: bool, hot: bool, s: f32) {
    let p = theme::pal();
    let rr = theme::round(r, (r.bottom - r.top) / 2.0);
    unsafe {
        let fill = if prominent { p.accent } else { theme::with_alpha(p.hover, if hot { 2.0 } else { 1.3 }) };
        if let Some(b) = theme::brush(rt, fill) {
            rt.FillRoundedRectangle(&rr, &b);
        }
        if prominent && hot {
            if let Some(b) = theme::brush(rt, Col(0xFFFFFF, 0.14)) {
                rt.FillRoundedRectangle(&rr, &b);
            }
        }
    }
    let ink = if prominent { p.on_accent } else { p.label };
    if let Some(b) = theme::brush(rt, ink) {
        let tw = gfx.measure(f, text);
        let x = (r.left + r.right - tw) / 2.0;
        d2d::draw_text(rt, f, text, rect_f(x, (r.top + r.bottom) / 2.0 - 9.5 * s, r.right, r.bottom), &b);
    }
}

fn button_w(gfx: &Gfx, f: &IDWriteTextFormat, text: &str, s: f32) -> f32 {
    gfx.measure(f, text) + 32.0 * s
}

/// The Cut out overlay for the current phase: the first-use card, progress,
/// the hint pill or the result bar. `area` is the canvas (below the toolbar).
pub fn draw_ui(st: &mut CutState, rt: &ID2D1RenderTarget, gfx: &Gfx, f: &Fonts, area: D2D_RECT_F, s: f32, back: Option<&Backdrop>) {
    let p = theme::pal();
    st.buttons.clear();
    let phase = st.worker.phase();
    let (Some(fg), Some(fg2)) = (theme::brush(rt, p.label), theme::brush(rt, p.label2)) else { return };
    let cx = (area.left + area.right) / 2.0;
    let card = |rt: &ID2D1RenderTarget, h: f32| {
        let w = (420.0 * s).min(area.right - area.left - 32.0 * s);
        let top = (area.top + area.bottom) / 2.0 - h / 2.0;
        let r = rect_f(cx - w / 2.0, top, cx + w / 2.0, top + h);
        theme::glass(rt, gfx, r, 22.0 * s, s, back);
        r
    };
    match &phase {
        Phase::NeedModel | Phase::Downloading | Phase::Failed(_) => {
            let r = card(rt, 196.0 * s);
            let x = r.left + 24.0 * s;
            d2d::draw_text(rt, f.title, "Cut out", rect_f(x, r.top + 20.0 * s, r.right, r.top + 52.0 * s), &fg);
            let body: String = match &phase {
                Phase::Failed(e) => e.clone(),
                _ => format!("Lifts an object off the picture by its shape.\nRuns on your PC. It needs a one-time {} MB download.", download_mb()),
            };
            let mut y = r.top + 58.0 * s;
            for line in body.lines() {
                d2d::draw_text(rt, f.body, line, rect_f(x, y, r.right - 20.0 * s, y + 20.0 * s), &fg2);
                y += 20.0 * s;
            }
            let by = r.bottom - 24.0 * s - 36.0 * s;
            match &phase {
                Phase::Downloading => {
                    let (done, total) = st.worker.progress();
                    let k = (done as f32 / total.max(1) as f32).clamp(0.0, 1.0);
                    let bar = rect_f(x, by + 12.0 * s, r.right - 24.0 * s, by + 18.0 * s);
                    if let (Some(track), Some(acc)) = (theme::brush(rt, theme::with_alpha(p.hover, 1.6)), theme::brush(rt, p.accent)) {
                        unsafe {
                            rt.FillRoundedRectangle(&theme::round(bar, 3.0 * s), &track);
                            let fill = rect_f(bar.left, bar.top, bar.left + (bar.right - bar.left) * k, bar.bottom);
                            rt.FillRoundedRectangle(&theme::round(fill, 3.0 * s), &acc);
                        }
                    }
                    let t = format!("Downloading... {} of {} MB", done / 1_000_000, total / 1_000_000);
                    d2d::draw_text(rt, f.label, &t, rect_f(x, by + 24.0 * s, r.right, by + 44.0 * s), &fg2);
                }
                Phase::Failed(_) => {
                    let w = button_w(gfx, f.label, "Try again", s);
                    let b = rect_f(r.right - 24.0 * s - w, by, r.right - 24.0 * s, by + 36.0 * s);
                    button(rt, f.label, gfx, b, "Try again", true, st.hot == Some(CutBtn::Retry), s);
                    st.buttons.push((CutBtn::Retry, b));
                }
                _ => {
                    let w1 = button_w(gfx, f.label, "Download", s);
                    let b1 = rect_f(r.right - 24.0 * s - w1, by, r.right - 24.0 * s, by + 36.0 * s);
                    let w2 = button_w(gfx, f.label, "Not now", s);
                    let b2 = rect_f(b1.left - 10.0 * s - w2, by, b1.left - 10.0 * s, by + 36.0 * s);
                    button(rt, f.label, gfx, b1, "Download", true, st.hot == Some(CutBtn::Download), s);
                    button(rt, f.label, gfx, b2, "Not now", false, st.hot == Some(CutBtn::NotNow), s);
                    st.buttons.push((CutBtn::Download, b1));
                    st.buttons.push((CutBtn::NotNow, b2));
                }
            }
        }
        Phase::Working | Phase::Ready => {
            let ph = 40.0 * s;
            let bottom = area.bottom - 16.0 * s;
            if let Some(_cut) = &st.cut {
                // Result bar: Copy, Sticker, Save, Clear.
                let labels = [(CutBtn::Copy, "Copy"), (CutBtn::Sticker, "Sticker"), (CutBtn::Save, "Save PNG"), (CutBtn::Clear, "Clear")];
                let widths: Vec<f32> = labels.iter().map(|(b, t)| button_w(gfx, f.label, t, s) + if *b == CutBtn::Sticker { 34.0 * s } else { 0.0 }).collect();
                let pad = 6.0 * s;
                let total: f32 = widths.iter().sum::<f32>() + pad * (labels.len() as f32 + 1.0);
                let bar = rect_f(cx - total / 2.0, bottom - ph - pad * 2.0, cx + total / 2.0, bottom);
                theme::glass(rt, gfx, bar, (bar.bottom - bar.top) / 2.0, s, back);
                let mut x = bar.left + pad;
                for ((b, t), w) in labels.iter().zip(widths) {
                    let r = rect_f(x, bar.top + pad, x + w, bar.bottom - pad);
                    let hot = st.hot == Some(*b);
                    if *b == CutBtn::Sticker {
                        // Label plus a small Apple switch.
                        unsafe {
                            if hot {
                                if let Some(h) = theme::brush(rt, p.hover) {
                                    rt.FillRoundedRectangle(&theme::round(r, (r.bottom - r.top) / 2.0), &h);
                                }
                            }
                        }
                        d2d::draw_text(rt, f.label, t, rect_f(r.left + 14.0 * s, (r.top + r.bottom) / 2.0 - 9.5 * s, r.right, r.bottom), &fg);
                        let sw = rect_f(r.right - 14.0 * s - 30.0 * s, (r.top + r.bottom) / 2.0 - 9.0 * s, r.right - 14.0 * s, (r.top + r.bottom) / 2.0 + 9.0 * s);
                        let on = st.sticker;
                        if let (Some(track), Some(knob)) = (theme::brush(rt, if on { p.green } else { theme::with_alpha(p.selected, 1.8) }), d2d::brush(rt, d2d::rgba(0xFFFFFF, 1.0)).ok()) {
                            unsafe {
                                rt.FillRoundedRectangle(&theme::round(sw, 9.0 * s), &track);
                                let kx = if on { sw.right - 9.0 * s } else { sw.left + 9.0 * s };
                                rt.FillEllipse(&D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: kx, Y: (sw.top + sw.bottom) / 2.0 }, radiusX: 7.0 * s, radiusY: 7.0 * s }, &knob);
                            }
                        }
                    } else {
                        let prominent = *b == CutBtn::Copy;
                        if prominent || hot {
                            button(rt, f.label, gfx, r, t, prominent, hot, s);
                        } else if let Some(b2) = theme::brush(rt, p.label) {
                            let tw = gfx.measure(f.label, t);
                            d2d::draw_text(rt, f.label, t, rect_f((r.left + r.right - tw) / 2.0, (r.top + r.bottom) / 2.0 - 9.5 * s, r.right, r.bottom), &b2);
                        }
                    }
                    st.buttons.push((*b, r));
                    x += w + pad;
                }
            }
            // Hint / status pill above the bar (or alone).
            let msg: String = match (&phase, &st.note) {
                (Phase::Working, _) => "Looking at the picture...".into(),
                (_, Some((n, t))) if t.elapsed().as_secs_f32() < 2.5 => n.clone(),
                _ if st.cut.is_some() => String::new(),
                _ => "Click an object to cut it out \u{00B7} Shift adds another".into(),
            };
            if !msg.is_empty() {
                let tw = gfx.measure(f.label, &msg) + 36.0 * s;
                let y1 = if st.cut.is_some() { bottom - ph - 12.0 * s - 12.0 * s } else { bottom };
                let r = rect_f(cx - tw / 2.0, y1 - 34.0 * s, cx + tw / 2.0, y1);
                theme::glass(rt, gfx, r, 17.0 * s, s, back);
                if phase == Phase::Working {
                    // Three dots breathing in turn.
                    let t = st.started.elapsed().as_secs_f32();
                    for k in 0..3 {
                        let a = 0.3 + 0.7 * (((t * 3.0 - k as f32 * 0.6).sin() + 1.0) / 2.0);
                        if let Some(b) = theme::brush(rt, theme::with_alpha(p.accent, a)) {
                            let e = D2D1_ELLIPSE { point: windows_numerics::Vector2 { X: r.left + 14.0 * s + k as f32 * 7.0 * s, Y: (r.top + r.bottom) / 2.0 }, radiusX: 2.4 * s, radiusY: 2.4 * s };
                            unsafe { rt.FillEllipse(&e, &b) };
                        }
                    }
                    d2d::draw_text(rt, f.label, &msg, rect_f(r.left + 36.0 * s, r.top + 7.5 * s, r.right, r.bottom), &fg);
                } else {
                    d2d::draw_text(rt, f.label, &msg, rect_f(r.left + 18.0 * s, r.top + 7.5 * s, r.right, r.bottom), &fg);
                }
            }
        }
    }
}

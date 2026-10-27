//! Background worker that finds UI elements under a point: child windows,
//! then UI Automation, then pixel boxes in the frozen frame. The overlay posts
//! queries and never waits; answers come back via WM_APP_ELEMENTS.

use super::WM_APP_ELEMENTS;
use super::util::to_rect;
use crate::geom::{Point, Rect};
use crate::image::Frame;
use crate::regions::{self, EdgeMap};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::Interface;

const BUDGET: Duration = Duration::from_millis(400);
const MAX_DEPTH: usize = 48;
const MAX_CHILDREN: i32 = 4000;
/// How many siblings to look inside when none of them contains the point.
const LOOKAHEAD: i32 = 64;

pub struct Query {
    pub capture: u64,
    pub point: Point,
    /// Top-level window under the point (raw HWND value).
    pub top: isize,
    pub window: Rect,
    pub frame: Arc<Frame>,
}

pub struct Answer {
    pub capture: u64,
    pub window: Rect,
    pub rects: Vec<Rect>,
    pub ms: u32,
    pub hwnd_n: usize,
    pub uia_n: usize,
    pub pixel_n: usize,
    pub trace: String,
}

pub struct Elements {
    tx: mpsc::Sender<Query>,
    slot: Arc<Mutex<Option<Answer>>>,
}

impl Elements {
    pub fn start(notify: HWND) -> Elements {
        let (tx, rx) = mpsc::channel::<Query>();
        let slot = Arc::new(Mutex::new(None));
        let worker_slot = slot.clone();
        let notify = notify.0 as isize;
        std::thread::spawn(move || worker(rx, worker_slot, notify));
        Elements { tx, slot }
    }

    pub fn ask(&self, q: Query) {
        let _ = self.tx.send(q);
    }

    pub fn take(&self) -> Option<Answer> {
        self.slot.lock().ok()?.take()
    }
}

struct Uia {
    auto: IUIAutomation,
    cache: IUIAutomationCacheRequest,
    all: IUIAutomationCondition,
}

fn init_uia() -> Option<Uia> {
    unsafe {
        let auto: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER)
            .or_else(|_| CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER))
            .ok()?;
        if let Ok(a2) = auto.cast::<IUIAutomation2>() {
            let _ = a2.SetConnectionTimeout(300);
            let _ = a2.SetTransactionTimeout(250);
        }
        let cache = auto.CreateCacheRequest().ok()?;
        cache.AddProperty(UIA_BoundingRectanglePropertyId).ok()?;
        cache.AddProperty(UIA_IsOffscreenPropertyId).ok()?;
        let all = auto.CreateTrueCondition().ok()?;
        cache.SetTreeFilter(&all).ok()?;
        Some(Uia { auto, cache, all })
    }
}

fn worker(rx: mpsc::Receiver<Query>, slot: Arc<Mutex<Option<Answer>>>, notify: isize) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let uia = init_uia();
    let debug = std::env::var_os("RENGI_SNAP_DEBUG").is_some();
    let mut edges: Option<(u64, Rect, EdgeMap)> = None;
    while let Ok(mut q) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            q = newer;
        }
        let t0 = Instant::now();
        let top = HWND(q.top as *mut _);
        let (deepest, mut rects) = child_chain(top, q.point);
        let hwnd_n = rects.len();
        let mut uia_n = 0;
        let mut trace = String::new();
        if debug {
            trace = format!("uia {} deepest {}; ", uia.is_some(), class_of(deepest));
        }
        if let Some(u) = &uia {
            let found = uia_chain(u, deepest, q.point, t0, debug.then_some(&mut trace));
            uia_n = found.len();
            rects.extend(found);
        }
        let smallest = rects
            .iter()
            .map(|r| r.intersect(&q.window))
            .filter(|r| r.contains(q.point) && r.width() >= 12 && r.height() >= 12)
            .min_by_key(|r| r.area())
            .unwrap_or(q.window);
        let mut pixel_n = 0;
        let big = smallest.width() >= 200 && smallest.height() >= 150;
        if smallest == q.window || big {
            let within = smallest.intersect(&q.frame.rect);
            let fresh = matches!(&edges, Some((c, r, _)) if *c == q.capture && *r == within);
            if !fresh {
                edges = Some((q.capture, within, EdgeMap::build(&q.frame, within)));
            }
            if let Some((_, _, map)) = &edges {
                let found = regions::find(map, q.point, 12);
                pixel_n = found.len();
                rects.extend(found);
            }
        }
        let answer = Answer {
            capture: q.capture,
            window: q.window,
            rects,
            ms: t0.elapsed().as_millis() as u32,
            hwnd_n,
            uia_n,
            pixel_n,
            trace,
        };
        drop(q);
        if let Ok(mut s) = slot.lock() {
            *s = Some(answer);
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(notify as *mut _)), WM_APP_ELEMENTS, WPARAM(0), LPARAM(0));
        }
    }
}

/// Child HWNDs under the point, outermost first. Returns the deepest one.
fn child_chain(top: HWND, p: Point) -> (HWND, Vec<Rect>) {
    let mut out = Vec::new();
    let mut cur = top;
    for _ in 0..32 {
        let mut pt = POINT { x: p.x, y: p.y };
        unsafe {
            if !ScreenToClient(cur, &mut pt).as_bool() {
                break;
            }
        }
        let mut child = unsafe { ChildWindowFromPointEx(cur, pt, CWP_SKIPINVISIBLE | CWP_SKIPTRANSPARENT) };
        if child.is_invalid() || child == cur {
            // Chromium (Chrome, Edge, Electron apps) hosts the page's UIA tree in a
            // transparent child window; without it the page is one flat pane.
            let c = unsafe { ChildWindowFromPointEx(cur, pt, CWP_SKIPINVISIBLE) };
            if c.is_invalid() || c == cur || class_of(c) != "Chrome_RenderWidgetHostHWND" {
                break;
            }
            child = c;
        }
        let mut r = RECT::default();
        if unsafe { GetWindowRect(child, &mut r) }.is_ok() {
            out.push(to_rect(r));
        }
        cur = child;
    }
    (cur, out)
}

/// Descends the UIA tree from `start`, always into the smallest child that
/// contains the point. Returns every rect on the way down.
fn uia_chain(u: &Uia, start: HWND, p: Point, t0: Instant, mut trace: Option<&mut String>) -> Vec<Rect> {
    let mut out = Vec::new();
    let mut el = match unsafe { u.auto.ElementFromHandleBuildCache(start, &u.cache) } {
        Ok(e) => e,
        Err(e) => {
            if let Some(t) = trace.as_deref_mut() {
                t.push_str(&format!("from handle failed {e}; "));
            }
            return out;
        }
    };
    for _ in 0..MAX_DEPTH {
        if t0.elapsed() > BUDGET {
            break;
        }
        let kids = match unsafe { el.FindAllBuildCache(TreeScope_Children, &u.all, &u.cache) } {
            Ok(k) => k,
            Err(e) => {
                if let Some(t) = trace.as_deref_mut() {
                    t.push_str(&format!("findall failed {e}; "));
                }
                break;
            }
        };
        let n = unsafe { kids.Length() }.unwrap_or(0).min(MAX_CHILDREN);
        let (mut off, mut norect, mut outside) = (0, 0, 0);
        let mut best: Option<(i64, IUIAutomationElement, Rect)> = None;
        for i in 0..n {
            let Ok(k) = (unsafe { kids.GetElement(i) }) else { continue };
            if unsafe { k.CachedIsOffscreen() }.map(|b| b.as_bool()).unwrap_or(false) {
                off += 1;
                continue;
            }
            let Ok(r) = (unsafe { k.CachedBoundingRectangle() }) else {
                norect += 1;
                continue;
            };
            let r = to_rect(r);
            if r.is_empty() || !r.contains(p) {
                outside += 1;
                continue;
            }
            if best.as_ref().is_none_or(|b| r.area() < b.0) {
                best = Some((r.area(), k, r));
            }
        }
        if let Some(t) = trace.as_deref_mut() {
            t.push_str(&format!("[{n} kids off {off} norect {norect} outside {outside}] "));
        }
        // Tree views: a parent item's rect covers only its own row, not the
        // expanded children under it, so look one level deeper.
        if best.is_none() {
            best = lookahead(u, &kids, n.min(LOOKAHEAD), p, t0);
            if let (Some(t), Some(b)) = (trace.as_deref_mut(), &best) {
                t.push_str(&format!("(lookahead {:?}) ", b.2));
            }
        }
        let Some((_, k, r)) = best else { break };
        out.push(r);
        el = k;
    }
    out
}

fn class_of(h: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// Smallest grandchild containing the point, searched under the first `n` children.
fn lookahead(u: &Uia, kids: &IUIAutomationElementArray, n: i32, p: Point, t0: Instant) -> Option<(i64, IUIAutomationElement, Rect)> {
    let mut best: Option<(i64, IUIAutomationElement, Rect)> = None;
    for i in 0..n {
        if t0.elapsed() > BUDGET {
            break;
        }
        let Ok(k) = (unsafe { kids.GetElement(i) }) else { continue };
        let Ok(grand) = (unsafe { k.FindAllBuildCache(TreeScope_Children, &u.all, &u.cache) }) else { continue };
        let m = unsafe { grand.Length() }.unwrap_or(0).min(MAX_CHILDREN);
        for j in 0..m {
            let Ok(g) = (unsafe { grand.GetElement(j) }) else { continue };
            if unsafe { g.CachedIsOffscreen() }.map(|b| b.as_bool()).unwrap_or(false) {
                continue;
            }
            let Ok(r) = (unsafe { g.CachedBoundingRectangle() }) else { continue };
            let r = to_rect(r);
            if r.is_empty() || !r.contains(p) {
                continue;
            }
            if best.as_ref().is_none_or(|b| r.area() < b.0) {
                best = Some((r.area(), g, r));
            }
        }
    }
    best
}

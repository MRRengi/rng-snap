//! Light/dark theme that follows Windows, and the Liquid Glass drawing shared
//! by every window: soft shadow, blurred backdrop, tint, gradient rim.

use super::d2d::{Gfx, rect_f};
use crate::palette::{self, Palette, Rgba};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Controls::MARGINS;
use windows::core::{Interface, w};

static DARK: AtomicBool = AtomicBool::new(true);
static READ: AtomicBool = AtomicBool::new(false);

fn reg_dword(key: windows::core::PCWSTR, value: windows::core::PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = 4u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, key, value, RRF_RT_REG_DWORD, None, Some(&mut data as *mut u32 as *mut _), Some(&mut size)).ok().ok()? };
    Some(data)
}

const PERSONALIZE: windows::core::PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");

fn read_dark() -> bool {
    match std::env::var("RENGI_SNAP_THEME").ok().as_deref() {
        Some("light") => return false,
        Some("dark") => return true,
        _ => {}
    }
    reg_dword(PERSONALIZE, w!("AppsUseLightTheme")).map(|v| v == 0).unwrap_or(true)
}

/// True when the app should draw dark.
pub fn dark() -> bool {
    if !READ.swap(true, Ordering::AcqRel) {
        DARK.store(read_dark(), Ordering::Release);
    }
    DARK.load(Ordering::Acquire)
}

/// Re-reads the Windows setting; true if it changed.
pub fn refresh() -> bool {
    let d = read_dark();
    READ.store(true, Ordering::Release);
    DARK.swap(d, Ordering::AcqRel) != d
}

pub fn pal() -> Palette {
    palette::get(dark())
}

/// Windows "Transparency effects" switch.
pub fn transparency() -> bool {
    std::env::var_os("RENGI_SNAP_NO_GLASS").is_none() && reg_dword(PERSONALIZE, w!("EnableTransparency")).map(|v| v != 0).unwrap_or(true)
}

pub fn color(c: Rgba) -> D2D1_COLOR_F {
    super::d2d::rgba(c.0, c.1)
}

pub fn with_alpha(c: Rgba, a: f32) -> Rgba {
    Rgba(c.0, c.1 * a)
}

pub fn brush(rt: &ID2D1RenderTarget, c: Rgba) -> Option<ID2D1SolidColorBrush> {
    unsafe { rt.CreateSolidColorBrush(&color(c), None).ok() }
}

/// Title bar and frame in the current theme.
pub fn apply_window(hwnd: HWND) {
    let d = dark() as u32;
    unsafe {
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &d as *const _ as *const _, 4);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Material {
    /// Long-lived windows (Home).
    Mica,
    /// Popups (card, recording bar).
    Acrylic,
}

/// Asks DWM to draw a system material behind the whole client area. True
/// when Windows supports it (Windows 11 22H2+) and transparency is on; the
/// window then has to clear to transparent.
pub fn material(hwnd: HWND, m: Material) -> bool {
    if !transparency() {
        return false;
    }
    let kind: u32 = match m {
        Material::Mica => 2,    // DWMSBT_MAINWINDOW
        Material::Acrylic => 3, // DWMSBT_TRANSIENTWINDOW
    };
    unsafe {
        if DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(38), &kind as *const _ as *const _, 4).is_err() {
            return false;
        }
        let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        DwmExtendFrameIntoClientArea(hwnd, &margins).is_ok()
    }
}

/// Popup menus follow the app theme (uxtheme ordinals every dark Win32 app
/// uses: 135 SetPreferredAppMode, 104 RefreshImmersiveColorPolicyState,
/// 136 FlushMenuThemes). No-ops where they don't exist.
pub fn menus_follow_theme() {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows::core::PCSTR;
    unsafe {
        let Ok(m) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        if let Some(f) = GetProcAddress(m, PCSTR(135usize as *const u8)) {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set(if dark() { 2 } else { 3 }); // ForceDark / ForceLight
        }
        if let Some(f) = GetProcAddress(m, PCSTR(104usize as *const u8)) {
            let refresh: unsafe extern "system" fn() = std::mem::transmute(f);
            refresh();
        }
        if let Some(f) = GetProcAddress(m, PCSTR(136usize as *const u8)) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

pub fn round(r: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }
}

pub fn inset(r: D2D_RECT_F, d: f32) -> D2D_RECT_F {
    rect_f(r.left + d, r.top + d, r.right - d, r.bottom - d)
}

/// Soft drop shadow: stacked rounded rects that fade outwards, offset down.
pub fn shadow(rt: &ID2D1RenderTarget, r: D2D_RECT_F, radius: f32, s: f32, strength: f32) {
    let p = pal();
    let spread = 14.0 * s;
    let steps = 9;
    let dy = 6.0 * s;
    for i in (0..steps).rev() {
        let k = (i + 1) as f32 / steps as f32;
        let grow = spread * k;
        let a = p.shadow.1 * strength * (1.0 - k).powf(1.6) * 0.42;
        if let Some(b) = brush(rt, Rgba(p.shadow.0, a)) {
            let q = rect_f(r.left - grow, r.top - grow + dy, r.right + grow, r.bottom + grow + dy);
            unsafe { rt.FillRoundedRectangle(&round(q, radius + grow), &b) };
        }
    }
    // Contact shadow right under the edge.
    if let Some(b) = brush(rt, Rgba(p.shadow.0, p.shadow.1 * strength * 0.5)) {
        let q = rect_f(r.left, r.top + 1.0 * s, r.right, r.bottom + 1.5 * s);
        unsafe { rt.FillRoundedRectangle(&round(q, radius), &b) };
    }
}

/// Blurs a source bitmap for glass backdrops. Effects belong to the render
/// target that made them, so each window keeps its own.
pub struct Blur {
    rt_key: usize,
    blur: Option<ID2D1Effect>,
    sat: Option<ID2D1Effect>,
    src_key: usize,
    sigma: f32,
}

impl Default for Blur {
    fn default() -> Self {
        Blur { rt_key: 0, blur: None, sat: None, src_key: 0, sigma: 0.0 }
    }
}

impl Blur {
    /// The blurred, slightly saturated `src`, in `src`'s pixel space.
    pub fn image(&mut self, rt: &ID2D1RenderTarget, src: &ID2D1Bitmap, sigma: f32) -> Option<ID2D1Image> {
        let dc: ID2D1DeviceContext = rt.cast().ok()?;
        let key = rt.as_raw() as usize;
        if key != self.rt_key {
            self.rt_key = key;
            self.src_key = 0;
            unsafe {
                let blur = dc.CreateEffect(&CLSID_D2D1GaussianBlur).ok()?;
                let hard = 1u32; // D2D1_BORDER_MODE_HARD: no transparent fringe at the edges
                let _ = blur.SetValue(D2D1_GAUSSIANBLUR_PROP_BORDER_MODE.0 as u32, D2D1_PROPERTY_TYPE_ENUM, &hard.to_le_bytes());
                let sat = dc.CreateEffect(&CLSID_D2D1Saturation).ok()?;
                let _ = sat.SetValue(D2D1_SATURATION_PROP_SATURATION.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &1.5f32.to_le_bytes());
                let out = blur.GetOutput().ok()?;
                sat.SetInput(0, &out, true);
                self.blur = Some(blur);
                self.sat = Some(sat);
            }
        }
        let (blur, sat) = (self.blur.as_ref()?, self.sat.as_ref()?);
        let sk = src.as_raw() as usize;
        unsafe {
            if sk != self.src_key {
                let img: ID2D1Image = src.cast().ok()?;
                blur.SetInput(0, &img, true);
                self.src_key = sk;
            }
            if sigma != self.sigma {
                let _ = blur.SetValue(D2D1_GAUSSIANBLUR_PROP_STANDARD_DEVIATION.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &sigma.to_le_bytes());
                self.sigma = sigma;
            }
            sat.GetOutput().ok()
        }
    }
}

/// What a glass shape shows through: a blurred image and where its pixel
/// (0, 0) sits on the render target.
pub struct Backdrop<'a> {
    pub image: &'a ID2D1Image,
    pub origin: (f32, f32),
    /// Scale from image pixels to target pixels (1 when drawn 1:1).
    pub zoom: f32,
}

/// One Liquid Glass shape: shadow, blurred backdrop (or a solid tint when
/// there is nothing to blur), tint, gradient rim and hairline.
pub fn glass(rt: &ID2D1RenderTarget, gfx: &Gfx, r: D2D_RECT_F, radius: f32, s: f32, back: Option<&Backdrop>) {
    let p = pal();
    shadow(rt, r, radius, s, 1.0);
    let rr = round(r, radius);
    let mut blurred = false;
    if let Some(b) = back {
        if let Ok(dc) = rt.cast::<ID2D1DeviceContext>() {
            unsafe {
                if let Ok(geo) = gfx.d2d.CreateRoundedRectangleGeometry(&rr) {
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
                    let z = b.zoom.max(0.01);
                    let src = rect_f((r.left - b.origin.0) / z - 2.0, (r.top - b.origin.1) / z - 2.0, (r.right - b.origin.0) / z + 2.0, (r.bottom - b.origin.1) / z + 2.0);
                    if (z - 1.0).abs() < 0.001 {
                        let off = windows_numerics::Vector2 { X: b.origin.0 + src.left, Y: b.origin.1 + src.top };
                        dc.DrawImage(b.image, Some(&off), Some(&src), D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
                    } else {
                        let mut old = windows_numerics::Matrix3x2::identity();
                        dc.GetTransform(&mut old);
                        let m = windows_numerics::Matrix3x2 { M11: z, M12: 0.0, M21: 0.0, M22: z, M31: b.origin.0, M32: b.origin.1 };
                        dc.SetTransform(&m);
                        let off = windows_numerics::Vector2 { X: src.left, Y: src.top };
                        dc.DrawImage(b.image, Some(&off), Some(&src), D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
                        dc.SetTransform(&old);
                    }
                    dc.PopLayer();
                    let _ = std::mem::ManuallyDrop::into_inner(params.geometricMask);
                    blurred = true;
                }
            }
        }
    }
    unsafe {
        if let Some(t) = brush(rt, if blurred { p.glass } else { p.glass_solid }) {
            rt.FillRoundedRectangle(&rr, &t);
        }
    }
    rim(rt, r, radius, s);
}

/// The light-catching edge: a 1 px gradient stroke bright at the top, and a
/// hairline just outside so the shape holds on any background.
pub fn rim(rt: &ID2D1RenderTarget, r: D2D_RECT_F, radius: f32, _s: f32) {
    let p = pal();
    unsafe {
        if let Some(h) = brush(rt, p.hairline) {
            rt.DrawRoundedRectangle(&round(inset(r, -0.5), radius + 0.5), &h, 1.0, None);
        }
        let stops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: color(p.rim_top) },
            D2D1_GRADIENT_STOP { position: 0.45, color: color(p.rim_bottom) },
            D2D1_GRADIENT_STOP { position: 1.0, color: color(Rgba(p.rim_bottom.0, p.rim_bottom.1 * 1.6)) },
        ];
        if let Ok(coll) = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) {
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: windows_numerics::Vector2 { X: r.left, Y: r.top },
                endPoint: windows_numerics::Vector2 { X: r.left, Y: r.bottom },
            };
            if let Ok(g) = rt.CreateLinearGradientBrush(&props, None, &coll) {
                rt.DrawRoundedRectangle(&round(inset(r, 0.5), (radius - 0.5).max(0.0)), &g, 1.0, None);
            }
        }
    }
}

/// Draws a white-on-transparent icon bitmap tinted with `c`.
pub fn tinted(rt: &ID2D1RenderTarget, bmp: &ID2D1Bitmap, dst: D2D_RECT_F, c: Rgba) {
    let Some(b) = brush(rt, c) else { return };
    unsafe {
        let mode = rt.GetAntialiasMode();
        rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_ALIASED);
        rt.FillOpacityMask(bmp, &b, D2D1_OPACITY_MASK_CONTENT_GRAPHICS, Some(&dst), None);
        rt.SetAntialiasMode(mode);
    }
}

/// Eases hover highlights in and out (ease-out, 120 ms). Keys are any small
/// numbers the caller picks per control.
pub struct Fader {
    vals: Vec<(u64, f32)>,
    target: Option<u64>,
    last: Instant,
}

impl Default for Fader {
    fn default() -> Self {
        Fader { vals: Vec::new(), target: None, last: Instant::now() }
    }
}

impl Fader {
    pub const MS: f32 = 120.0;

    pub fn set(&mut self, key: Option<u64>) {
        if key != self.target {
            self.step();
            self.target = key;
            if let Some(k) = key {
                if !self.vals.iter().any(|(x, _)| *x == k) {
                    self.vals.push((k, 0.0));
                }
            }
        }
    }

    /// Advances time; true while something is still moving.
    pub fn step(&mut self) -> bool {
        let dt = self.last.elapsed().as_secs_f32() * 1000.0;
        self.last = Instant::now();
        let d = (dt / Self::MS).min(1.0);
        let t = self.target;
        for (k, v) in &mut self.vals {
            if Some(*k) == t {
                *v = (*v + d).min(1.0);
            } else {
                *v = (*v - d).max(0.0);
            }
        }
        self.vals.retain(|(k, v)| *v > 0.0 || Some(*k) == t);
        self.vals.iter().any(|(k, v)| if Some(*k) == t { *v < 1.0 } else { *v > 0.0 })
    }

    /// Eased value 0..1 for a key.
    pub fn get(&self, key: u64) -> f32 {
        let v = self.vals.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or(0.0);
        1.0 - (1.0 - v).powi(3)
    }
}

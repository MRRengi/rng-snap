//! Direct2D / DirectWrite plumbing shared by the overlay and the preview card.
//! Render targets run at 96 DPI so one DIP is one physical pixel; UI sizes are
//! scaled by hand with the monitor's DPI.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::core::{BOOL, HSTRING, Interface, Result, w};

pub struct Gfx {
    pub d2d: ID2D1Factory1,
    pub dw: IDWriteFactory,
    pub icon_family: HSTRING,
}

impl Gfx {
    pub fn new() -> Result<Gfx> {
        unsafe {
            let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let icon_family = pick_icon_family(&dw);
            Ok(Gfx { d2d, dw, icon_family })
        }
    }

    pub fn target(&self, hwnd: HWND, w: u32, h: u32) -> Result<ID2D1HwndRenderTarget> {
        unsafe {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U { width: w, height: h },
                presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
            };
            let rt = self.d2d.CreateHwndRenderTarget(&props, &hprops)?;
            rt.SetDpi(96.0, 96.0);
            Ok(rt)
        }
    }

    pub fn text_format(&self, family: &HSTRING, size: f32, bold: bool) -> Result<IDWriteTextFormat> {
        unsafe {
            let f = self.dw.CreateTextFormat(
                family,
                None,
                if bold { DWRITE_FONT_WEIGHT_SEMI_BOLD } else { DWRITE_FONT_WEIGHT_NORMAL },
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok(f)
        }
    }

    pub fn ui_font(&self, size: f32, bold: bool) -> Result<IDWriteTextFormat> {
        self.text_format(&HSTRING::from("Segoe UI"), size, bold)
    }

    pub fn icon_font(&self, size: f32) -> Result<IDWriteTextFormat> {
        let f = self.text_format(&self.icon_family, size, false)?;
        unsafe {
            f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        }
        Ok(f)
    }

    /// Width of a single line of text in pixels.
    pub fn measure(&self, fmt: &IDWriteTextFormat, text: &str) -> f32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            match self.dw.CreateTextLayout(&wide, fmt, 10000.0, 1000.0) {
                Ok(l) => {
                    let mut m = DWRITE_TEXT_METRICS::default();
                    if l.GetMetrics(&mut m).is_ok() { m.widthIncludingTrailingWhitespace } else { 0.0 }
                }
                Err(_) => 0.0,
            }
        }
    }
}

/// Windows 11 ships "Segoe Fluent Icons", Windows 10 "Segoe MDL2 Assets"; same code points.
fn pick_icon_family(dw: &IDWriteFactory) -> HSTRING {
    unsafe {
        let mut coll = None;
        if dw.GetSystemFontCollection(&mut coll, false).is_ok() {
            if let Some(coll) = coll {
                let mut idx = 0;
                let mut exists = BOOL(0);
                if coll.FindFamilyName(w!("Segoe Fluent Icons"), &mut idx, &mut exists).is_ok() && exists.as_bool() {
                    return HSTRING::from("Segoe Fluent Icons");
                }
            }
        }
    }
    HSTRING::from("Segoe MDL2 Assets")
}

pub fn rgba(hex: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xFF) as f32 / 255.0,
        g: ((hex >> 8) & 0xFF) as f32 / 255.0,
        b: (hex & 0xFF) as f32 / 255.0,
        a,
    }
}

pub fn rect_f(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: l, top: t, right: r, bottom: b }
}

pub fn brush(rt: &ID2D1RenderTarget, color: D2D1_COLOR_F) -> Result<ID2D1SolidColorBrush> {
    unsafe { rt.CreateSolidColorBrush(&color, None) }
}

/// Uploads top-down BGRA pixels as an opaque bitmap.
pub fn bitmap(rt: &ID2D1RenderTarget, w: u32, h: u32, pixels: &[u8]) -> Result<ID2D1Bitmap> {
    unsafe {
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        rt.CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(pixels.as_ptr() as *const _), w * 4, &props)
    }
}

pub fn draw_text(rt: &ID2D1RenderTarget, fmt: &IDWriteTextFormat, text: &str, r: D2D_RECT_F, b: &ID2D1SolidColorBrush) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        rt.DrawText(&wide, fmt, &r, b, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
    }
}

pub fn round_rect(r: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: r, radiusX: radius, radiusY: radius }
}

pub fn as_device_context(rt: &ID2D1HwndRenderTarget) -> Option<ID2D1DeviceContext> {
    rt.cast::<ID2D1DeviceContext>().ok()
}

//! Embeds the app icon and version info into RengiSnap.exe (MSVC builds),
//! and writes the icon as an .ico for the installer. No resource compiler
//! needed: the `.res` is written by hand (src/winres.rs) and linked directly.

#[path = "src/icon.rs"]
mod icon;
#[path = "src/winres.rs"]
mod winres;

fn png(n: usize, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, n as u32, n as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(rgba).expect("png data");
    }
    out
}

/// 24-bit bottom-up BMP from a top-down RGB buffer.
fn bmp(w: usize, h: usize, rgb: &[u8]) -> Vec<u8> {
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut b = Vec::with_capacity(size);
    b.extend_from_slice(b"BM");
    b.extend_from_slice(&(size as u32).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    b.extend_from_slice(&54u32.to_le_bytes());
    b.extend_from_slice(&40u32.to_le_bytes());
    b.extend_from_slice(&(w as i32).to_le_bytes());
    b.extend_from_slice(&(h as i32).to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&24u16.to_le_bytes());
    b.extend_from_slice(&[0u8; 24]);
    for y in (0..h).rev() {
        for x in 0..w {
            let i = (y * w + x) * 3;
            b.extend_from_slice(&[rgb[i + 2], rgb[i + 1], rgb[i]]);
        }
        b.extend(std::iter::repeat_n(0u8, row - w * 3));
    }
    b
}

/// Background `bg(x, y)` with the icon (premultiplied BGRA) laid over it.
fn art(w: usize, h: usize, icon_px: usize, icon_y: usize, bg: impl Fn(usize, usize) -> [f32; 3]) -> Vec<u8> {
    let icon = icon::pixels(icon_px);
    let ix = (w - icon_px) / 2;
    let mut rgb = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let mut c = bg(x, y);
            if x >= ix && x < ix + icon_px && y >= icon_y && y < icon_y + icon_px {
                let p = &icon[((y - icon_y) * icon_px + x - ix) * 4..][..4];
                let a = p[3] as f32 / 255.0;
                c = [p[2] as f32 + c[0] * (1.0 - a), p[1] as f32 + c[1] * (1.0 - a), p[0] as f32 + c[2] * (1.0 - a)];
            }
            rgb.extend_from_slice(&[c[0].round() as u8, c[1].round() as u8, c[2].round() as u8]);
        }
    }
    rgb
}

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=src/winres.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let images = winres::icon_images(&icon::pixels, &png);
    let ico = winres::ico_file(&images);
    let _ = std::fs::write(out.join("app.ico"), &ico);
    // Also next to the build output, where the installer script finds it.
    let manifest = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let _ = std::fs::create_dir_all(manifest.join("target"));
    let _ = std::fs::write(manifest.join("target").join("app.ico"), &ico);
    // Installer artwork: a calm light panel with the icon, and the small corner icon.
    let (ww, wh) = (164, 314);
    let big = art(ww, wh, 96, 92, |_, y| {
        let t = y as f32 / wh as f32;
        [246.0 - 10.0 * t, 247.0 - 9.0 * t, 250.0 - 6.0 * t]
    });
    let _ = std::fs::write(manifest.join("target").join("wizard.bmp"), bmp(ww, wh, &big));
    let small = art(58, 58, 50, 4, |_, _| [255.0, 255.0, 255.0]);
    let _ = std::fs::write(manifest.join("target").join("wizard-small.bmp"), bmp(58, 58, &small));

    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        let version = env!("CARGO_PKG_VERSION");
        let info = winres::version_info(
            version,
            &[
                ("CompanyName", "rng"),
                ("FileDescription", "Snap by rng"),
                ("FileVersion", version),
                ("InternalName", "RengiSnap"),
                ("LegalCopyright", "MIT License"),
                ("OriginalFilename", "RengiSnap.exe"),
                ("ProductName", "Snap by rng"),
                ("ProductVersion", version),
            ],
        );
        let res = out.join("rengi-snap.res");
        std::fs::write(&res, winres::res_file(&images, &info)).expect("write .res");
        println!("cargo:rustc-link-arg-bin=RengiSnap={}", res.display());
    }
}

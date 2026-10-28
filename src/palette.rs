//! Colours for the light and dark themes: Apple system colours and the
//! Liquid Glass material, as plain numbers so they can be tested anywhere.

/// 0xRRGGBB plus alpha 0..1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba(pub u32, pub f32);

impl Rgba {
    /// The same colour, fully opaque.
    pub fn solid(self) -> Rgba {
        Rgba(self.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub accent: Rgba,
    pub red: Rgba,
    pub purple: Rgba,
    pub teal: Rgba,
    pub green: Rgba,
    /// Text on any surface.
    pub label: Rgba,
    pub label2: Rgba,
    pub label3: Rgba,
    pub separator: Rgba,
    pub hover: Rgba,
    pub selected: Rgba,
    /// Window background where there is no backdrop material.
    pub window: Rgba,
    /// Behind the image in the editors.
    pub canvas: Rgba,
    /// Grouped rows and tiles on the window background.
    pub card: Rgba,
    /// Glass tint over a blurred backdrop.
    pub glass: Rgba,
    /// Glass with nothing to blur behind it.
    pub glass_solid: Rgba,
    /// Gradient rim: top and bottom.
    pub rim_top: Rgba,
    pub rim_bottom: Rgba,
    /// Thin line just outside the glass.
    pub hairline: Rgba,
    pub shadow: Rgba,
    /// The raised thumb of a segmented control / selected tool.
    pub thumb: Rgba,
    /// Text on accent-coloured buttons.
    pub on_accent: Rgba,
}

pub const DARK: Palette = Palette {
    dark: true,
    accent: Rgba(0x0A84FF, 1.0),
    red: Rgba(0xFF453A, 1.0),
    purple: Rgba(0xBF5AF2, 1.0),
    teal: Rgba(0x64D2FF, 1.0),
    green: Rgba(0x30D158, 1.0),
    label: Rgba(0xFFFFFF, 0.92),
    label2: Rgba(0xEBEBF5, 0.60),
    label3: Rgba(0xEBEBF5, 0.30),
    separator: Rgba(0xFFFFFF, 0.10),
    hover: Rgba(0xFFFFFF, 0.08),
    selected: Rgba(0xFFFFFF, 0.16),
    window: Rgba(0x1E1E20, 1.0),
    canvas: Rgba(0x121214, 1.0),
    card: Rgba(0xFFFFFF, 0.06),
    glass: Rgba(0x232326, 0.70),
    glass_solid: Rgba(0x2A2A2E, 0.97),
    rim_top: Rgba(0xFFFFFF, 0.30),
    rim_bottom: Rgba(0xFFFFFF, 0.06),
    hairline: Rgba(0x000000, 0.45),
    shadow: Rgba(0x000000, 0.34),
    thumb: Rgba(0xFFFFFF, 0.17),
    on_accent: Rgba(0xFFFFFF, 1.0),
};

pub const LIGHT: Palette = Palette {
    dark: false,
    accent: Rgba(0x007AFF, 1.0),
    red: Rgba(0xFF3B30, 1.0),
    purple: Rgba(0xAF52DE, 1.0),
    teal: Rgba(0x32ADE6, 1.0),
    green: Rgba(0x34C759, 1.0),
    label: Rgba(0x000000, 0.85),
    label2: Rgba(0x3C3C43, 0.76),
    label3: Rgba(0x3C3C43, 0.40),
    separator: Rgba(0x000000, 0.10),
    hover: Rgba(0x000000, 0.05),
    selected: Rgba(0x000000, 0.09),
    window: Rgba(0xF2F2F5, 1.0),
    canvas: Rgba(0xE6E6EB, 1.0),
    card: Rgba(0xFFFFFF, 0.70),
    glass: Rgba(0xFAFAFC, 0.66),
    glass_solid: Rgba(0xF7F7F9, 0.97),
    rim_top: Rgba(0xFFFFFF, 0.95),
    rim_bottom: Rgba(0xFFFFFF, 0.35),
    hairline: Rgba(0x000000, 0.10),
    shadow: Rgba(0x000000, 0.16),
    thumb: Rgba(0xFFFFFF, 0.95),
    on_accent: Rgba(0xFFFFFF, 1.0),
};

pub fn get(dark: bool) -> Palette {
    if dark { DARK } else { LIGHT }
}

fn channels(c: u32) -> [f32; 3] {
    [((c >> 16) & 0xFF) as f32 / 255.0, ((c >> 8) & 0xFF) as f32 / 255.0, (c & 0xFF) as f32 / 255.0]
}

/// `top` composited over the opaque colour `under`.
pub fn over(top: Rgba, under: u32) -> u32 {
    let (t, u) = (channels(top.0), channels(under));
    let mix = |i: usize| ((t[i] * top.1 + u[i] * (1.0 - top.1)) * 255.0).round() as u32;
    (mix(0) << 16) | (mix(1) << 8) | mix(2)
}

/// WCAG relative luminance.
pub fn luminance(c: u32) -> f32 {
    let lin = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    let [r, g, b] = channels(c);
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG contrast ratio between two opaque colours.
pub fn contrast(a: u32, b: u32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Worst-case things a glass panel can sit over: a white page, a black
    /// terminal, a mid-grey photo.
    const BACKDROPS: [u32; 3] = [0xFFFFFF, 0x000000, 0x808080];

    #[test]
    fn glass_text_is_readable_over_anything() {
        for p in [DARK, LIGHT] {
            for b in BACKDROPS {
                // The backdrop is blurred, then tinted.
                let surface = over(p.glass, b);
                let text = over(p.label, surface);
                let c = contrast(text, surface);
                // Body text needs 4.5:1 (WCAG AA). Light glass over black is
                // the hardest case and still has to pass.
                assert!(c >= 4.5, "dark={} backdrop {b:06X}: label contrast {c:.2}", p.dark);
            }
        }
    }

    #[test]
    fn secondary_text_is_readable_on_solid_surfaces() {
        for p in [DARK, LIGHT] {
            for surface in [p.glass_solid.0, p.window.0, over(p.card, p.window.0)] {
                let c = contrast(over(p.label2, surface), surface);
                assert!(c >= 4.5, "dark={} surface {surface:06X}: secondary {c:.2}", p.dark);
            }
        }
    }

    #[test]
    fn accent_works_as_a_button_and_as_text() {
        for p in [DARK, LIGHT] {
            // White text on the accent button: Apple's own blue gives ~4:1;
            // the bar is the large-text 3:1 used for button labels.
            assert!(contrast(p.on_accent.0, p.accent.0) >= 3.0);
            // Accent as a selection outline must stand out from the canvas.
            assert!(contrast(p.accent.0, p.canvas.0) >= 3.0, "dark={}", p.dark);
        }
    }

    #[test]
    fn over_and_contrast_basics() {
        assert_eq!(over(Rgba(0xFFFFFF, 1.0), 0x000000), 0xFFFFFF);
        assert_eq!(over(Rgba(0xFFFFFF, 0.0), 0x123456), 0x123456);
        assert!((contrast(0xFFFFFF, 0x000000) - 21.0).abs() < 0.01);
    }
}

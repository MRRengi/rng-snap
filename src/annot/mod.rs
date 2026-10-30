//! Annotation model shared by the overlay quick mode and the editor window.
//! Items live in image coordinates (pixels of the base image).

pub mod edit;
pub mod font;
pub mod icons;
pub mod render;

use crate::geom::Rect;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct P {
    pub x: f32,
    pub y: f32,
}

impl P {
    pub const fn new(x: f32, y: f32) -> P {
        P { x, y }
    }
    pub fn dist(self, o: P) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
    pub fn offset(self, dx: f32, dy: f32) -> P {
        P::new(self.x + dx, self.y + dy)
    }
}

/// sRGB colour, no alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color(pub u8, pub u8, pub u8);

impl Color {
    pub fn hex(self) -> u32 {
        (self.0 as u32) << 16 | (self.1 as u32) << 8 | self.2 as u32
    }
    pub fn from_hex(h: u32) -> Color {
        Color((h >> 16) as u8, (h >> 8) as u8, h as u8)
    }
    /// Relative brightness 0..1.
    pub fn luma(self) -> f32 {
        (0.2126 * self.0 as f32 + 0.7152 * self.1 as f32 + 0.0722 * self.2 as f32) / 255.0
    }
}

pub const PALETTE: [Color; 8] = [
    Color(0xE5, 0x48, 0x4D), // red
    Color(0xF7, 0x6B, 0x15), // orange
    Color(0xFF, 0xC5, 0x3D), // yellow
    Color(0x30, 0xA4, 0x6C), // green
    Color(0x00, 0x90, 0xFF), // blue
    Color(0x8E, 0x4E, 0xC6), // purple
    Color(0x1C, 0x20, 0x24), // black
    Color(0xFF, 0xFF, 0xFF), // white
];

/// Stroke widths for the three thickness steps, before UI scale.
pub const WIDTHS: [f32; 3] = [3.0, 5.0, 8.0];
pub const TEXT_SIZES: [f32; 3] = [18.0, 26.0, 38.0];
pub const STEP_RADII: [f32; 3] = [14.0, 18.0, 25.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Arrow,
    Line,
    Rect,
    Ellipse,
    Pen,
    Highlighter,
    Text,
    Blur,
    Step,
    Crop,
    /// Video editor only: a camera zoom on part of the frame.
    Zoom,
    /// Photo editor only: lift an object off the picture by its shape.
    CutOut,
}

impl Tool {
    pub fn key(self) -> char {
        match self {
            Tool::Select => 'V',
            Tool::Arrow => 'A',
            Tool::Line => 'L',
            Tool::Rect => 'R',
            Tool::Ellipse => 'E',
            Tool::Pen => 'P',
            Tool::Highlighter => 'H',
            Tool::Text => 'T',
            Tool::Blur => 'B',
            Tool::Step => 'N',
            Tool::Crop => 'C',
            Tool::Zoom => 'Z',
            Tool::CutOut => 'K',
        }
    }

    pub fn from_key(c: char) -> Option<Tool> {
        ALL_TOOLS.iter().copied().find(|t| t.key() == c.to_ascii_uppercase())
    }

    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Arrow => "Arrow",
            Tool::Line => "Line",
            Tool::Rect => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Pen => "Pen",
            Tool::Highlighter => "Highlighter",
            Tool::Text => "Text",
            Tool::Blur => "Blur",
            Tool::Step => "Step",
            Tool::Crop => "Crop",
            Tool::Zoom => "Zoom",
            Tool::CutOut => "Cut out",
        }
    }

    pub fn parse(s: &str) -> Tool {
        ALL_TOOLS.iter().copied().find(|t| t.name().eq_ignore_ascii_case(s)).unwrap_or(Tool::Arrow)
    }
}

pub const ALL_TOOLS: [Tool; 11] = [
    Tool::Select,
    Tool::Arrow,
    Tool::Line,
    Tool::Rect,
    Tool::Ellipse,
    Tool::Pen,
    Tool::Highlighter,
    Tool::Text,
    Tool::Blur,
    Tool::Step,
    Tool::Crop,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlurMode {
    Blur,
    Pixelate,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Arrow { a: P, b: P },
    Line { a: P, b: P },
    Rect { a: P, b: P },
    Ellipse { a: P, b: P },
    Pen { pts: Vec<P> },
    Highlighter { pts: Vec<P> },
    Text { at: P, text: String, size: f32 },
    Blur { a: P, b: P, mode: BlurMode },
    Step { c: P, r: f32 },
    /// Video editor only: the camera zooms to this area. Never drawn into the image.
    Zoom { a: P, b: P },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub shape: Shape,
    pub color: Color,
    pub width: f32,
    /// Video editor: when the item is on screen, as [start, end) in ms of the
    /// edited video. None = always (screenshots).
    pub span: Option<(u32, u32)>,
}

impl Item {
    /// True when the item is on screen at `t` ms (always for screenshots).
    pub fn visible_at(&self, t: Option<u32>) -> bool {
        match (self.span, t) {
            (Some((a, b)), Some(t)) => t >= a && t < b,
            _ => true,
        }
    }

    pub fn translate(&mut self, dx: f32, dy: f32) {
        match &mut self.shape {
            Shape::Arrow { a, b } | Shape::Line { a, b } | Shape::Rect { a, b } | Shape::Ellipse { a, b } | Shape::Blur { a, b, .. } | Shape::Zoom { a, b } => {
                *a = a.offset(dx, dy);
                *b = b.offset(dx, dy);
            }
            Shape::Pen { pts } | Shape::Highlighter { pts } => {
                for p in pts {
                    *p = p.offset(dx, dy);
                }
            }
            Shape::Text { at, .. } => *at = at.offset(dx, dy),
            Shape::Step { c, .. } => *c = c.offset(dx, dy),
        }
    }
}

/// Normalized float rect from two corners.
pub fn norm(a: P, b: P) -> (f32, f32, f32, f32) {
    (a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y))
}

pub fn to_rect(a: P, b: P) -> Rect {
    let (l, t, r, bt) = norm(a, b);
    Rect::new(l.round() as i32, t.round() as i32, r.round() as i32, bt.round() as i32)
}

//! Fonts, shaping and the glyph cache.
//!
//! - **Shaping** uses `harfrust`, but runs that can't need it (code points below U+0300,
//!   ligatures off) take a fast path that maps characters straight to glyphs. Most source
//!   code is ASCII, so most lines never touch the shaper. A test checks the fast path
//!   produces exactly what the shaper would.
//! - **Rasterization** uses `swash`, cached per (font, glyph, size, ¼-pixel x offset).
//! - **Fonts:** JetBrains Mono is bundled, so the first frame needs no font discovery.
//!   System fonts and fallback (CJK, emoji) arrive in a later phase.

mod cache;
mod face;
mod layout;

use cache::GlyphCache;
use face::Face;
use fude_render::{Color, DrawList, ImageStore};
use harfrust::Feature;

pub use layout::{LineLayout, PositionedGlyph};

/// Index of a font face loaded into a [`TextSystem`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FontId(pub u16);

impl FontId {
    pub const MONO: FontId = FontId(0);
    pub const MONO_BOLD: FontId = FontId(1);
    pub const MONO_ITALIC: FontId = FontId(2);
}

/// How to lay out and draw a run of text.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextStyle {
    pub font: FontId,
    /// Font size in physical pixels.
    pub px: f32,
    /// Tab stops every this many space widths.
    pub tab_width: u32,
    /// Programming ligatures (`calt`/`liga`). Off by default: it also disables the fast path.
    pub ligatures: bool,
    /// TrueType hinting. Sharper at low DPI; usually off on high-DPI screens.
    pub hint: bool,
}

impl TextStyle {
    pub fn new(font: FontId, px: f32) -> TextStyle {
        TextStyle {
            font,
            px,
            tab_width: 4,
            ligatures: false,
            hint: px < 24.0,
        }
    }
}

/// Vertical metrics of a font at a size, in physical pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LineMetrics {
    /// Distance from the baseline up to the top of the line box.
    pub ascent: f32,
    /// Distance from the baseline down to the bottom of the line box (positive).
    pub descent: f32,
    pub line_gap: f32,
    /// Advance width of a space; the column width for monospace fonts.
    pub space_advance: f32,
}

static JETBRAINS_MONO: &[u8] =
    include_bytes!("../../../assets/fonts/jetbrains-mono/JetBrainsMono[wght].ttf");
static JETBRAINS_MONO_ITALIC: &[u8] =
    include_bytes!("../../../assets/fonts/jetbrains-mono/JetBrainsMono-Italic[wght].ttf");

/// Owns fonts, shaping state and rasterized glyphs.
pub struct TextSystem {
    faces: Vec<Face>,
    glyphs: GlyphCache,
    shape_buffer: Option<harfrust::Buffer>,
    no_ligatures: Vec<Feature>,
}

impl TextSystem {
    /// Loads the bundled fonts. Cheap: font tables are parsed lazily.
    pub fn new() -> TextSystem {
        let faces = vec![
            Face::from_static(JETBRAINS_MONO, 400.0).expect("bundled font is valid"),
            Face::from_static(JETBRAINS_MONO, 700.0).expect("bundled font is valid"),
            Face::from_static(JETBRAINS_MONO_ITALIC, 400.0).expect("bundled font is valid"),
        ];
        let no_ligatures = ["-liga", "-calt"]
            .iter()
            .map(|f| f.parse().expect("valid feature"))
            .collect();
        TextSystem {
            faces,
            glyphs: GlyphCache::new(),
            shape_buffer: None,
            no_ligatures,
        }
    }

    pub fn metrics(&self, font: FontId, px: f32) -> LineMetrics {
        let face = self.face(font);
        let s = px / face.upem;
        let space = face.space_advance();
        LineMetrics {
            ascent: face.ascender * s,
            descent: -face.descender * s,
            line_gap: face.line_gap * s,
            space_advance: space * s,
        }
    }

    /// Lays out one line of text (no line breaks) into `out`, reusing its allocation.
    pub fn layout_line(&mut self, text: &str, style: &TextStyle, out: &mut LineLayout) {
        let face = &self.faces[style.font.0 as usize];
        let buffer = self.shape_buffer.get_or_insert_with(harfrust::Buffer::new);
        let features: &[Feature] = if style.ligatures {
            &[]
        } else {
            &self.no_ligatures
        };
        layout::layout_line(face, style, features, buffer, text, out);
    }

    /// Appends draw commands for `layout`, with its origin at `x` and the baseline at
    /// `baseline`. Glyphs entirely outside `visible_x` (a horizontal pixel range) are
    /// skipped.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_line(
        &mut self,
        layout: &LineLayout,
        style: &TextStyle,
        x: f32,
        baseline: i32,
        color: Color,
        visible_x: (f32, f32),
        list: &mut DrawList,
        images: &mut ImageStore,
    ) {
        let face = &self.faces[style.font.0 as usize];
        let margin = style.px * 2.0;
        for g in &layout.glyphs {
            let gx = x + g.x;
            if gx > visible_x.1 {
                break;
            }
            if gx + margin < visible_x.0 {
                continue;
            }
            // Snap to a quarter pixel horizontally; whole pixels vertically.
            let quarters = (gx * 4.0).round() as i32;
            let ix = quarters.div_euclid(4);
            let sub = quarters.rem_euclid(4) as u8;
            let iy = baseline + g.y.round() as i32;
            if let Some(cached) = self.glyphs.get(face, style, g.glyph, sub, images) {
                let (px, py) = (ix + cached.left, iy - cached.top);
                if cached.color {
                    list.image(cached.image, px, py);
                } else {
                    list.mask(cached.image, px, py, color);
                }
            }
        }
    }

    /// Number of rasterized glyphs held in the cache.
    pub fn cached_glyphs(&self) -> usize {
        self.glyphs.len()
    }

    fn face(&self, font: FontId) -> &Face {
        &self.faces[font.0 as usize]
    }
}

impl Default for TextSystem {
    fn default() -> TextSystem {
        TextSystem::new()
    }
}

#[cfg(test)]
mod tests;

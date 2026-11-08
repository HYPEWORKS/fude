//! Fonts, shaping and the glyph cache.
//!
//! - **Shaping** uses `harfrust`, but runs that can't need it (code points below U+0300,
//!   ligatures off) take a fast path that maps characters straight to glyphs. Most source
//!   code is ASCII, so most lines never touch the shaper. A test checks the fast path
//!   produces exactly what the shaper would.
//! - **Rasterization** uses `swash`, cached per (font, glyph, size, ¼-pixel x offset).
//! - **Fonts:** JetBrains Mono is bundled, so the first frame needs no font discovery.
//!   Characters it lacks (CJK, emoji, most scripts) come from system fonts, found through
//!   `fontique` the first time one appears and cached per character.
//! - **Chips:** control characters and invalid bytes are laid out as boxes; see [`Chip`].

mod cache;
mod chip;
mod face;
mod fallback;
mod layout;

use cache::GlyphCache;
use face::Face;
use fallback::Fallback;
use fude_render::{Color, DrawList, ImageStore};
use harfrust::Feature;
use rustc_hash::FxHashMap;

pub use chip::Chip;
pub use layout::{LineLayout, PositionedGlyph};

/// Index of a font face loaded into a [`TextSystem`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FontId(pub u16);

impl FontId {
    pub const MONO: FontId = FontId(0);
    pub const MONO_BOLD: FontId = FontId(1);
    pub const MONO_ITALIC: FontId = FontId(2);
    /// Marks chip "glyphs" in a layout; see [`Chip`].
    pub const CHIP: FontId = FontId(u16::MAX);
}

/// Where to draw a chip within its advance.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ChipGeometry {
    /// Gap between the advance's edges and the box.
    pub margin: f32,
    /// Box width.
    pub width: f32,
    /// Label style (smaller than the text).
    pub label: TextStyle,
    /// Label x offset from the box's left edge.
    pub label_x: f32,
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
    /// The font chosen for a character the fast path can't decide, per style font.
    font_for: FxHashMap<(FontId, char), FontId>,
    /// System fonts, opened on first need.
    fallback: Option<Fallback>,
    /// Fallback faces already loaded, by file and face index.
    loaded: FxHashMap<(u64, u32), FontId>,
    /// Scratch: font runs of the segment being laid out.
    runs: Vec<(usize, FontId)>,
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
            font_for: FxHashMap::default(),
            fallback: None,
            loaded: FxHashMap::default(),
            runs: Vec::new(),
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
        out.glyphs.clear();
        let tab = self.metrics(style.font, style.px).space_advance * style.tab_width.max(1) as f32;
        let mut pen = 0.0f32;
        let mut seg_start = 0;
        let mut runs = std::mem::take(&mut self.runs);
        // Tabs split the line into segments; each segment splits into runs by font.
        for (i, seg) in text.split('\t').enumerate() {
            if i > 0 {
                pen = ((pen / tab).floor() + 1.0) * tab;
            }
            runs.clear();
            for (off, c) in seg.char_indices() {
                let current = runs.last().map(|&(_, f)| f);
                let font = self.font_for_char(c, style.font, current);
                if current != Some(font) {
                    runs.push((off, font));
                }
            }
            for (k, &(start, font)) in runs.iter().enumerate() {
                let end = runs.get(k + 1).map_or(seg.len(), |&(s, _)| s);
                pen = self.layout_run(&seg[start..end], seg_start + start, font, style, pen, out);
            }
            seg_start += seg.len() + 1;
        }
        self.runs = runs;
        out.width = pen;
    }

    fn layout_run(
        &mut self,
        text: &str,
        cluster_base: usize,
        font: FontId,
        style: &TextStyle,
        mut pen: f32,
        out: &mut LineLayout,
    ) -> f32 {
        if font == FontId::CHIP {
            for (off, c) in text.char_indices() {
                out.glyphs.push(PositionedGlyph {
                    glyph: c as u32,
                    font,
                    x: pen,
                    y: 0.0,
                    cluster: (cluster_base + off) as u32,
                });
                let chip = Chip::decode(c).expect("chip run");
                pen += self.chip_advance(chip, style);
            }
            return pen;
        }
        let face = &self.faces[font.0 as usize];
        let scale = style.px / face.upem;
        if font == style.font && !style.ligatures && !layout::needs_shaping(text) {
            return layout::fast_run(face, font, text, cluster_base, pen, scale, out);
        }
        let features: &[Feature] = if style.ligatures {
            &[]
        } else {
            &self.no_ligatures
        };
        let buffer = self.shape_buffer.get_or_insert_with(harfrust::Buffer::new);
        layout::shaped_run(
            face,
            font,
            features,
            buffer,
            text,
            cluster_base,
            pen,
            scale,
            out,
        )
    }

    /// The font to draw `c` with when the line's font is `primary`. `current` is the font
    /// of the run so far, which combining marks and joiners stay in.
    fn font_for_char(&mut self, c: char, primary: FontId, current: Option<FontId>) -> FontId {
        if (c as u32) < 0x300 {
            return primary;
        }
        if Chip::decode(c).is_some() {
            return FontId::CHIP;
        }
        if let Some(cur) = current
            && cur != FontId::CHIP
            && layout::continues_cluster(c)
        {
            return cur;
        }
        if let Some(&font) = self.font_for.get(&(primary, c)) {
            return font;
        }
        let font = if self.faces[primary.0 as usize].covers(c) {
            primary
        } else {
            self.fallback_font(c).unwrap_or(primary)
        };
        self.font_for.insert((primary, c), font);
        font
    }

    fn fallback_font(&mut self, c: char) -> Option<FontId> {
        // Emoji fonts can cover a character with a glyph swash can't draw (COLRv1-only
        // fonts, like Fedora's Noto Color Emoji); skip those for the next candidate.
        let check_ink = fallback::is_emoji(c);
        let mut skip = Vec::new();
        loop {
            let found = self
                .fallback
                .get_or_insert_with(Fallback::new)
                .find(c, &skip)?;
            let id = match self.loaded.get(&found.key) {
                Some(&id) => id,
                None => {
                    let face = Face::new(found.data, found.index, None)?;
                    let id = FontId(
                        u16::try_from(self.faces.len())
                            .ok()
                            .filter(|&n| n < u16::MAX)?,
                    );
                    self.faces.push(face);
                    self.loaded.insert(found.key, id);
                    id
                }
            };
            if !check_ink || self.renders(id, c) {
                return Some(id);
            }
            skip.push(found.key);
        }
    }

    fn renders(&mut self, font: FontId, c: char) -> bool {
        let face = &self.faces[font.0 as usize];
        let Some(glyph) = face.shaper().nominal_glyph(c as u32) else {
            return false;
        };
        self.glyphs.has_ink(face, glyph.to_u32())
    }

    /// How a chip is drawn at `style`'s size.
    pub fn chip_geometry(&self, chip: Chip, style: &TextStyle) -> ChipGeometry {
        let label = TextStyle::new(style.font, (style.px * 0.72).round().max(1.0));
        let label_advance = self.metrics(label.font, label.px).space_advance;
        let margin = (style.px * 0.08).max(1.0);
        let pad = (style.px * 0.18).max(1.0);
        let label_w = chip.label().len() as f32 * label_advance;
        ChipGeometry {
            margin,
            width: label_w + 2.0 * pad,
            label,
            label_x: pad,
        }
    }

    fn chip_advance(&self, chip: Chip, style: &TextStyle) -> f32 {
        let g = self.chip_geometry(chip, style);
        g.width + 2.0 * g.margin
    }

    /// Number of loaded font faces (bundled plus fallbacks).
    pub fn face_count(&self) -> usize {
        self.faces.len()
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
        let margin = style.px * 2.0;
        for g in &layout.glyphs {
            if g.font == FontId::CHIP {
                continue;
            }
            let face = &self.faces[g.font.0 as usize];
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
            if let Some(cached) = self.glyphs.get(face, g.font, style, g.glyph, sub, images) {
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

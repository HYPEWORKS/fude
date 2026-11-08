use harfrust::{ShaperFont, Tag};

/// One font at one point in its design space (e.g. JetBrains Mono at weight 700).
pub(crate) struct Face {
    data: &'static [u8],
    /// The instance used for shaping and metrics. Cloning is cheap (shared tables).
    pub(crate) font: harfrust::Font,
    swash_offset: u32,
    swash_key: swash::CacheKey,
    /// The same instance's normalized coordinates, for the rasterizer.
    pub(crate) coords: Vec<i16>,
    pub(crate) upem: f32,
    /// Font units; `descender` is negative.
    pub(crate) ascender: f32,
    pub(crate) descender: f32,
    pub(crate) line_gap: f32,
}

impl Face {
    pub(crate) fn from_static(data: &'static [u8], weight: f32) -> Option<Face> {
        let base = harfrust::Font::new(data, 0)?;
        let font = base
            .instance_builder()
            .variations([(Tag::new(b"wght"), weight)])
            .build();
        let coords = font
            .normalized_coords()
            .iter()
            .map(|c| c.to_bits())
            .collect();
        let swash = swash::FontRef::from_index(data, 0)?;
        let metrics = font.metrics();
        let line = metrics.h_line()?;
        Some(Face {
            data,
            swash_offset: swash.offset,
            swash_key: swash.key,
            coords,
            upem: metrics.units_per_em as f32,
            ascender: line.ascender.to_f32(),
            descender: line.descender.to_f32(),
            line_gap: line.line_gap.to_f32(),
            font,
        })
    }

    pub(crate) fn swash(&self) -> swash::FontRef<'_> {
        swash::FontRef {
            data: self.data,
            offset: self.swash_offset,
            key: self.swash_key,
        }
    }

    pub(crate) fn shaper(&self) -> ShaperFont<'_, '_> {
        ShaperFont::new(&self.font)
    }

    /// Advance of U+0020 in font units.
    pub(crate) fn space_advance(&self) -> f32 {
        let shaper = self.shaper();
        let gid = shaper.nominal_glyph(' ' as u32).unwrap_or_default();
        shaper.glyph_h_advance(gid) as f32
    }
}

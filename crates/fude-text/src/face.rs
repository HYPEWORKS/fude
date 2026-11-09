use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use harfrust::{GlyphId, ShaperFont, Tag};

/// Characters below this are cached by [`Face::nominal`]: what the fast path lays out.
pub(crate) const NOMINAL_CACHED: u32 = 0x300;
const UNCACHED: u64 = u64::MAX;

/// Font file data: bundled (static) or a memory-mapped system font.
pub(crate) type FontData = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// One font at one point in its design space (e.g. JetBrains Mono at weight 700).
pub(crate) struct Face {
    data: FontData,
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
    /// Glyph and advance (font units) of each character below [`NOMINAL_CACHED`],
    /// filled in as they're first used: looking them up in the font each time (a variable
    /// font's advances need its variation tables) costs several times as much.
    nominal: Box<[AtomicU64]>,
}

impl Face {
    /// Face `index` of `data` (a font collection can hold several), at `weight` if the
    /// font has a weight axis and one is given.
    pub(crate) fn new(data: FontData, index: u32, weight: Option<f32>) -> Option<Face> {
        let base = harfrust::Font::new(data.clone(), index)?;
        let font = match weight {
            Some(w) => base
                .instance_builder()
                .variations([(Tag::new(b"wght"), w)])
                .build(),
            None => base,
        };
        let coords = font
            .normalized_coords()
            .iter()
            .map(|c| c.to_bits())
            .collect();
        let swash = swash::FontRef::from_index((*data).as_ref(), index as usize)?;
        let (swash_offset, swash_key) = (swash.offset, swash.key);
        let metrics = font.metrics();
        let line = metrics.h_line()?;
        Some(Face {
            swash_offset,
            swash_key,
            coords,
            upem: metrics.units_per_em as f32,
            ascender: line.ascender.to_f32(),
            descender: line.descender.to_f32(),
            line_gap: line.line_gap.to_f32(),
            font,
            data,
            nominal: (0..NOMINAL_CACHED)
                .map(|_| AtomicU64::new(UNCACHED))
                .collect(),
        })
    }

    /// The glyph for `c` without shaping, and its advance in font units.
    pub(crate) fn nominal(&self, shaper: &ShaperFont<'_, '_>, c: char) -> (GlyphId, f32) {
        let slot = self.nominal.get(c as usize);
        if let Some(slot) = slot {
            let v = slot.load(Ordering::Relaxed);
            if v != UNCACHED {
                return (GlyphId::new(v as u32), f32::from_bits((v >> 32) as u32));
            }
        }
        let gid = shaper.nominal_glyph(c as u32).unwrap_or(GlyphId::NOTDEF);
        let advance = shaper.glyph_h_advance(gid) as f32;
        if let Some(slot) = slot {
            slot.store(
                u64::from(gid.to_u32()) | (u64::from(advance.to_bits()) << 32),
                Ordering::Relaxed,
            );
        }
        (gid, advance)
    }

    pub(crate) fn from_static(data: &'static [u8], weight: f32) -> Option<Face> {
        Face::new(Arc::new(data), 0, Some(weight))
    }

    pub(crate) fn swash(&self) -> swash::FontRef<'_> {
        swash::FontRef {
            data: (*self.data).as_ref(),
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

impl Face {
    /// Whether the font has a glyph for `c`.
    pub(crate) fn covers(&self, c: char) -> bool {
        self.shaper().nominal_glyph(c as u32).is_some()
    }
}

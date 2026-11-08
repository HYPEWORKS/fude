use crate::FontId;
use crate::face::Face;
use harfrust::{Feature, GlyphId, ShapeOptions};

/// A glyph placed on a line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PositionedGlyph {
    /// Glyph index in `font`; for [`FontId::CHIP`], the chip's placeholder character.
    pub glyph: u32,
    pub font: FontId,
    /// Pen position in pixels from the line's origin.
    pub x: f32,
    /// Vertical offset from the baseline in pixels (positive is down).
    pub y: f32,
    /// Byte offset in the line of the text this glyph came from.
    pub cluster: u32,
}

/// The shaped glyphs of one line.
#[derive(Clone, Default, Debug)]
pub struct LineLayout {
    pub glyphs: Vec<PositionedGlyph>,
    /// Total advance in pixels.
    pub width: f32,
}

/// Code points below U+0300 never need shaping in the fonts we use (no combining marks,
/// no complex scripts), as long as ligatures are off.
pub(crate) fn needs_shaping(text: &str) -> bool {
    text.chars().any(|c| c as u32 >= 0x300)
}

/// Characters that belong to the cluster before them (combining marks, joiners, variation
/// selectors, emoji modifiers), so they stay in that run's font.
pub(crate) fn continues_cluster(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x0483..=0x0489 | 0x0591..=0x05BD | 0x0610..=0x061A
        | 0x064B..=0x065F | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C..=0x200D | 0x20D0..=0x20FF
        | 0x3099..=0x309A | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x1F3FB..=0x1F3FF
        | 0xE0020..=0xE007F | 0xE0100..=0xE01EF)
}

/// Maps characters straight to glyphs: correct for the bundled monospace font when
/// [`needs_shaping`] is false.
pub(crate) fn fast_run(
    face: &Face,
    font: FontId,
    text: &str,
    cluster_base: usize,
    mut pen: f32,
    scale: f32,
    out: &mut LineLayout,
) -> f32 {
    let shaper = face.shaper();
    for (off, ch) in text.char_indices() {
        let gid = shaper.nominal_glyph(ch as u32).unwrap_or(GlyphId::NOTDEF);
        out.glyphs.push(PositionedGlyph {
            glyph: gid.to_u32(),
            font,
            x: pen,
            y: 0.0,
            cluster: (cluster_base + off) as u32,
        });
        pen += shaper.glyph_h_advance(gid) as f32 * scale;
    }
    pen
}

/// Shapes a run with harfrust.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shaped_run(
    face: &Face,
    font: FontId,
    features: &[Feature],
    buffer: &mut harfrust::Buffer,
    text: &str,
    cluster_base: usize,
    pen: f32,
    scale: f32,
    out: &mut LineLayout,
) -> f32 {
    let start = out.glyphs.len();
    let pen = shape_segment(
        &face.shaper(),
        features,
        buffer,
        text,
        cluster_base,
        pen,
        scale,
        out,
    );
    for g in &mut out.glyphs[start..] {
        g.font = font;
    }
    pen
}

#[allow(clippy::too_many_arguments)]
fn shape_segment(
    shaper: &harfrust::ShaperFont<'_, '_>,
    features: &[Feature],
    buffer: &mut harfrust::Buffer,
    seg: &str,
    start: usize,
    mut pen: f32,
    scale: f32,
    out: &mut LineLayout,
) -> f32 {
    if seg.is_empty() {
        return pen;
    }
    buffer.clear();
    buffer.push_str(seg);
    buffer.guess_segment_properties();
    if harfrust::shape(shaper, buffer, ShapeOptions::new().features(features)).is_err() {
        buffer.clear();
        return pen;
    }
    for (info, pos) in buffer.glyph_infos().iter().zip(buffer.glyph_positions()) {
        out.glyphs.push(PositionedGlyph {
            glyph: info.glyph_id,
            font: FontId(0),
            x: pen + pos.x_offset as f32 * scale,
            y: -pos.y_offset as f32 * scale,
            cluster: start as u32 + info.cluster,
        });
        pen += pos.x_advance as f32 * scale;
    }
    buffer.clear();
    pen
}

/// Shapes `text` with harfrust unconditionally (bypassing the fast path).
#[cfg(test)]
pub(crate) fn shape_segment_for_test(
    face: &Face,
    features: &[Feature],
    buffer: &mut harfrust::Buffer,
    text: &str,
    scale: f32,
    out: &mut LineLayout,
) -> f32 {
    shape_segment(&face.shaper(), features, buffer, text, 0, 0.0, scale, out)
}

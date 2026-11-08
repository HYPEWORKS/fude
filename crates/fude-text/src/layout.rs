use crate::TextStyle;
use crate::face::Face;
use harfrust::{Feature, GlyphId, ShapeOptions};

/// A glyph placed on a line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PositionedGlyph {
    pub glyph: u32,
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
fn needs_shaping(text: &str) -> bool {
    text.chars().any(|c| c as u32 >= 0x300)
}

pub(crate) fn layout_line(
    face: &Face,
    style: &TextStyle,
    features: &[Feature],
    buffer: &mut harfrust::Buffer,
    text: &str,
    out: &mut LineLayout,
) {
    out.glyphs.clear();
    let shaper = face.shaper();
    let scale = style.px / face.upem;
    let tab = face.space_advance() * scale * style.tab_width.max(1) as f32;
    let mut pen = 0.0f32;
    let mut start = 0usize;

    // Tabs split the line into segments; each segment is laid out on its own.
    for (i, seg) in text.split('\t').enumerate() {
        if i > 0 {
            pen = ((pen / tab).floor() + 1.0) * tab;
        }
        if style.ligatures || needs_shaping(seg) {
            pen = shape_segment(&shaper, features, buffer, seg, start, pen, scale, out);
        } else {
            for (off, ch) in seg.char_indices() {
                let gid = shaper.nominal_glyph(ch as u32).unwrap_or(GlyphId::NOTDEF);
                out.glyphs.push(PositionedGlyph {
                    glyph: gid.to_u32(),
                    x: pen,
                    y: 0.0,
                    cluster: (start + off) as u32,
                });
                pen += shaper.glyph_h_advance(gid) as f32 * scale;
            }
        }
        start += seg.len() + 1;
    }
    out.width = pen;
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

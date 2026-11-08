//! Horizontal geometry, which only the view knows (fonts, tab stops).

use fude_buffer::Rope;
use unicode_segmentation::UnicodeSegmentation;

/// Positions within lines, for vertical movement and mouse clicks.
pub trait Layout {
    /// The x position of `at` (a grapheme boundary on `line`), from the line's start.
    fn x_of(&mut self, text: &Rope, line: usize, at: usize) -> f32;
    /// The grapheme boundary on `line` nearest to `x`.
    fn offset_at(&mut self, text: &Rope, line: usize, x: f32) -> usize;
}

/// One unit per grapheme cluster or invalid byte, with tab stops: for tests and anything
/// without fonts.
#[derive(Clone, Copy, Debug)]
pub struct MonoLayout {
    pub tab_width: usize,
}

impl MonoLayout {
    /// Boundaries of `line` with their x positions, in order.
    fn stops(&self, text: &Rope, line: usize) -> Vec<(usize, f32)> {
        let span = text.line_span(line);
        let mut bytes = Vec::new();
        text.copy_range(span.start..span.end, &mut bytes);
        let tab = self.tab_width.max(1);
        let mut col = 0;
        let mut at = span.start;
        let mut stops = vec![(at, 0.0)];
        for chunk in bytes.utf8_chunks() {
            for g in chunk.valid().graphemes(true) {
                col = if g == "\t" {
                    (col / tab + 1) * tab
                } else {
                    col + 1
                };
                at += g.len();
                stops.push((at, col as f32));
            }
            for _ in chunk.invalid() {
                col += 1;
                at += 1;
                stops.push((at, col as f32));
            }
        }
        stops
    }
}

impl Layout for MonoLayout {
    fn x_of(&mut self, text: &Rope, line: usize, at: usize) -> f32 {
        let stops = self.stops(text, line);
        stops
            .iter()
            .rev()
            .find(|(off, _)| *off <= at)
            .map_or(0.0, |&(_, x)| x)
    }

    fn offset_at(&mut self, text: &Rope, line: usize, x: f32) -> usize {
        nearest(&self.stops(text, line), x)
    }
}

/// The boundary in `stops` (sorted by x) nearest to `x`.
pub fn nearest(stops: &[(usize, f32)], x: f32) -> usize {
    let i = stops.partition_point(|&(_, sx)| sx < x);
    match (i.checked_sub(1).map(|j| stops[j]), stops.get(i)) {
        (Some((a, ax)), Some(&(b, bx))) => {
            if x - ax <= bx - x {
                a
            } else {
                b
            }
        }
        (Some((a, _)), None) => a,
        (None, Some(&(b, _))) => b,
        (None, None) => 0,
    }
}

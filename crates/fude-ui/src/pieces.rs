//! Lines in pieces: the unit the view lays out.
//!
//! A line up to [`PIECE`] bytes is one piece. A longer one (minified code, a CSV row, a
//! log of one giant JSON value) is cut into pieces of about that size, each laid out on
//! its own and starting a new row, so a frame lays out at most a screenful of pieces
//! however long the line is. Without word wrap, a long line shows as rows of one piece
//! each (classic Notepad wraps long lines the same way); with it, each piece wraps at the
//! window's edge.
//!
//! Where a piece ends depends only on the bytes near that point, so pieces need no index
//! and nothing to update when the text changes: the k-th break of a line is near
//! `start + k * PIECE`, moved back to just after a space or punctuation if there is one
//! close by, and never inside a character.

use fude_buffer::{LineSpan, Rope};

/// Lines longer than this are laid out in pieces of about this size.
pub const PIECE: usize = 4096;
/// How far back from a piece's nominal end to look for a nicer place to break.
const SNAP: usize = 256;

/// A piece of line `line`: bytes `start..end` of its content `span`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Piece {
    pub line: usize,
    pub span: LineSpan,
    pub start: usize,
    pub end: usize,
}

impl Piece {
    pub fn is_first(&self) -> bool {
        self.start == self.span.start
    }

    pub fn is_last(&self) -> bool {
        self.end == self.span.end
    }

    /// Where the piece starts within its line.
    pub fn offset(&self) -> usize {
        self.start - self.span.start
    }

    /// Whether a cursor at `at` is drawn on this piece. At a break between pieces it
    /// belongs to the later one; past the line's end (inside its line break), to the
    /// last piece.
    pub fn holds(&self, at: usize) -> bool {
        at >= self.start
            && if self.is_last() {
                at < self.span.next || (at == self.span.end && self.span.next == self.span.end)
            } else {
                at < self.end
            }
    }
}

/// The piece of line `line` (with content `span`) holding offset `at`.
pub fn piece_at(doc: &Rope, line: usize, span: LineSpan, at: usize) -> Piece {
    let whole = Piece {
        line,
        span,
        start: span.start,
        end: span.end,
    };
    if span.end - span.start <= PIECE {
        return whole;
    }
    let at = at.clamp(span.start, span.end);
    // Breaks never fall after their nominal place, so `at`'s piece is the k-th or the next.
    // (The end of the line is in the last piece, even when the line is a multiple of
    // PIECE long.)
    let k = (at.min(span.end - 1) - span.start) / PIECE;
    let mut start = break_at(doc, span, k);
    let mut end = break_at(doc, span, k + 1);
    if at >= end && end < span.end {
        start = end;
        end = break_at(doc, span, k + 2);
    }
    Piece {
        line,
        span,
        start,
        end,
    }
}

/// The piece holding offset `at` of the document.
pub fn piece_of(doc: &Rope, at: usize) -> Piece {
    let at = at.min(doc.len());
    let line = doc.byte_to_line(at);
    piece_at(doc, line, doc.line_span(line), at)
}

pub fn first_piece(doc: &Rope, line: usize) -> Piece {
    let span = doc.line_span(line);
    piece_at(doc, line, span, span.start)
}

pub fn last_piece(doc: &Rope, line: usize) -> Piece {
    let span = doc.line_span(line);
    piece_at(doc, line, span, span.end)
}

pub fn next_piece(doc: &Rope, p: &Piece) -> Option<Piece> {
    if !p.is_last() {
        Some(piece_at(doc, p.line, p.span, p.end))
    } else if p.line + 1 < doc.line_count() {
        Some(first_piece(doc, p.line + 1))
    } else {
        None
    }
}

pub fn prev_piece(doc: &Rope, p: &Piece) -> Option<Piece> {
    if !p.is_first() {
        Some(piece_at(doc, p.line, p.span, p.start - 1))
    } else if p.line > 0 {
        Some(last_piece(doc, p.line - 1))
    } else {
        None
    }
}

/// The k-th break of a long line (the 0th is its start; past the end, its end).
fn break_at(doc: &Rope, span: LineSpan, k: usize) -> usize {
    let nominal = span.start + k * PIECE;
    if k == 0 {
        return span.start;
    }
    if nominal >= span.end {
        return span.end;
    }
    let lo = nominal - SNAP;
    let mut bytes = Vec::with_capacity(SNAP + 1);
    doc.copy_range(lo..nominal + 1, &mut bytes);
    // `bytes[i]` is the byte at `lo + i`; a break at `lo + i` falls between bytes i-1, i.
    let after = |pred: fn(u8) -> bool| {
        (1..=SNAP)
            .rev()
            .find(|&i| pred(bytes[i - 1]) && bytes[i] & 0xC0 != 0x80)
            .map(|i| lo + i)
    };
    after(|b| b == b' ' || b == b'\t')
        .or_else(|| after(|b| matches!(b, b',' | b';' | b':' | b')' | b']' | b'}' | b'>')))
        .unwrap_or_else(|| {
            // No nice place: the last character boundary, then grapheme boundary, as long
            // as it's after `lo` (a grapheme longer than SNAP bytes gets cut).
            let char_start = (lo + 1..=nominal)
                .rev()
                .find(|&i| bytes[i - lo] & 0xC0 != 0x80)
                .unwrap_or(nominal);
            let at = fude_editor::text::floor_grapheme(doc, char_start);
            if at > lo { at } else { char_start }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every piece of every line, walking forward.
    fn all(doc: &Rope) -> Vec<Piece> {
        let mut out = vec![first_piece(doc, 0)];
        while let Some(p) = next_piece(doc, out.last().unwrap()) {
            out.push(p);
        }
        out
    }

    fn check(doc: &Rope) {
        let pieces = all(doc);
        // They tile the text, line by line, without gaps.
        for w in pieces.windows(2) {
            let (a, b) = (w[0], w[1]);
            if a.line == b.line {
                assert_eq!(a.end, b.start);
                assert!(a.end - a.start <= PIECE + SNAP && a.end > a.start);
            } else {
                assert!(a.is_last() && b.is_first() && b.line == a.line + 1);
            }
        }
        // Walking back finds the same pieces.
        let mut back = vec![*pieces.last().unwrap()];
        while let Some(p) = prev_piece(doc, back.last().unwrap()) {
            back.push(p);
        }
        back.reverse();
        assert_eq!(back, pieces);
        // Every offset maps to the piece that holds it, and breaks fall on characters.
        let bytes = doc.to_vec();
        for p in &pieces {
            assert!(p.start == doc.len() || bytes[p.start] & 0xC0 != 0x80);
            for at in [p.start, (p.start + p.end) / 2, p.end] {
                let q = piece_of(doc, at);
                assert!(q.holds(at), "{at} in {q:?}");
                if at < p.end || p.is_last() {
                    assert_eq!(q, *p, "piece of {at}");
                }
            }
        }
    }

    #[test]
    fn short_lines_are_one_piece() {
        let doc = Rope::from("one\r\ntwo\n\nthree");
        let pieces = all(&doc);
        assert_eq!(pieces.len(), 4);
        assert!(pieces.iter().all(|p| p.is_first() && p.is_last()));
        check(&doc);
    }

    #[test]
    fn long_lines_break_near_spaces() {
        let line = "word ".repeat(5000);
        let doc = Rope::from(format!("short\n{line}\nend").as_str());
        check(&doc);
        let pieces = all(&doc);
        assert!(pieces.len() > 7);
        let bytes = doc.to_vec();
        for p in pieces.iter().filter(|p| p.line == 1 && !p.is_first()) {
            assert_eq!(bytes[p.start - 1], b' ', "breaks after a space");
        }
    }

    #[test]
    fn long_lines_without_spaces_break_on_characters() {
        let doc = Rope::from("日本語".repeat(3000).as_str());
        check(&doc);
        let doc = Rope::from("x".repeat(3 * PIECE + 1).as_str());
        let pieces = all(&doc);
        assert_eq!(pieces.len(), 4);
        assert_eq!(pieces[3].end - pieces[3].start, 1);
        check(&doc);
        // Exactly three pieces long: the end is in the third.
        let doc = Rope::from("x".repeat(3 * PIECE).as_str());
        assert_eq!(all(&doc).len(), 3);
        assert_eq!(piece_of(&doc, 3 * PIECE), all(&doc)[2]);
        check(&doc);
        // Combining marks stay with their letter.
        let doc = Rope::from("e\u{301}".repeat(3000).as_str());
        check(&doc);
        for p in all(&doc) {
            assert_eq!(doc.byte(p.start), b'e');
        }
    }

    /// Typing early in a long line moves later breaks with the text (or back to the
    /// previous space, when one crosses a break's nominal place), so rows far from the edit
    /// change little.
    #[test]
    fn breaks_follow_the_text() {
        let line = "word ".repeat(5000);
        let mut doc = Rope::from(line.as_str());
        let before = all(&doc);
        doc.insert(10, b"x");
        let after = all(&doc);
        assert_eq!(before.len(), after.len());
        let mut followed = 0;
        for (a, b) in before.iter().zip(&after).skip(1) {
            assert!(
                b.start <= a.start + 1 && b.start + SNAP > a.start,
                "{a:?} → {b:?}"
            );
            followed += usize::from(b.start == a.start + 1);
        }
        assert!(
            followed * 2 > before.len(),
            "most breaks move with the text"
        );
    }
}

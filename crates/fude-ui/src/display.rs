//! A document line as it is laid out: control characters and invalid bytes become chips,
//! and IME composition text is spliced in at the cursor. Offsets map both ways, so
//! clicks and cursors land on the document's bytes.

use std::ops::Range;

use fude_text::Chip;

/// Where display text and document bytes differ in length.
#[derive(Clone, Copy, Debug)]
struct Piece {
    /// Offset in the document line.
    doc: usize,
    doc_len: usize,
    /// Offset in the display text.
    display: usize,
    display_len: usize,
}

#[derive(Default, Debug)]
pub struct DisplayLine {
    pub text: String,
    pieces: Vec<Piece>,
    /// Display range of the IME composition, if any.
    pub preedit: Option<Range<usize>>,
}

impl DisplayLine {
    /// Builds the display text for `bytes` (one line, without its line break). `preedit`
    /// inserts composition text at a document offset.
    pub fn build(&mut self, bytes: &[u8], preedit: Option<(usize, &str)>) {
        self.text.clear();
        self.pieces.clear();
        self.preedit = None;
        // Printable ASCII (and tabs) shows as it is: no chips, offsets the same both ways.
        if preedit.is_none()
            && bytes
                .iter()
                .all(|&b| (b' '..=b'~').contains(&b) || b == b'\t')
        {
            self.text
                .push_str(std::str::from_utf8(bytes).expect("ASCII is UTF-8"));
            return;
        }
        let mut doc = 0;
        for chunk in bytes.utf8_chunks() {
            for c in chunk.valid().chars() {
                self.maybe_preedit(doc, preedit);
                match Chip::for_char(c) {
                    Some(chip) => self.chip(doc, c.len_utf8(), chip),
                    None => self.text.push(c),
                }
                doc += c.len_utf8();
            }
            for &b in chunk.invalid() {
                self.maybe_preedit(doc, preedit);
                self.chip(doc, 1, Chip::Byte(b));
                doc += 1;
            }
        }
        self.maybe_preedit(doc, preedit);
    }

    fn chip(&mut self, doc: usize, doc_len: usize, chip: Chip) {
        let c = chip.encode();
        self.pieces.push(Piece {
            doc,
            doc_len,
            display: self.text.len(),
            display_len: c.len_utf8(),
        });
        self.text.push(c);
    }

    fn maybe_preedit(&mut self, doc: usize, preedit: Option<(usize, &str)>) {
        if let Some((at, s)) = preedit
            && at == doc
            && self.preedit.is_none()
        {
            let start = self.text.len();
            self.pieces.push(Piece {
                doc,
                doc_len: 0,
                display: start,
                display_len: s.len(),
            });
            self.text.push_str(s);
            self.preedit = Some(start..self.text.len());
        }
    }

    /// The display offset of a document offset (before any composition text there).
    pub fn to_display(&self, doc: usize) -> usize {
        let mut delta = 0isize;
        for p in &self.pieces {
            if p.doc + p.doc_len <= doc && !(p.doc_len == 0 && p.doc == doc) {
                delta += p.display_len as isize - p.doc_len as isize;
            } else if p.doc < doc {
                return p.display;
            } else {
                break;
            }
        }
        doc.saturating_add_signed(delta)
    }

    /// The document offset of a display offset. Anything inside a chip or the composition
    /// maps to where it starts.
    pub fn to_doc(&self, display: usize) -> usize {
        let mut delta = 0isize;
        for p in &self.pieces {
            if display < p.display {
                break;
            }
            if display < p.display + p.display_len {
                return p.doc;
            }
            delta += p.display_len as isize - p.doc_len as isize;
        }
        display.saturating_add_signed(-delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_and_preedit_map_both_ways() {
        let mut d = DisplayLine::default();
        // a, ESC (1 byte), b, invalid FF, c, U+0085 (2 bytes, C1 control), d
        let bytes = b"a\x1bb\xffc\xc2\x85d";
        d.build(bytes, Some((4, "XY")));
        let chip = |c: Chip| c.encode().to_string();
        assert_eq!(
            d.text,
            format!(
                "a{}b{}XYc{}d",
                chip(Chip::Control(0x1B)),
                chip(Chip::Byte(0xFF)),
                chip(Chip::Control(0x85))
            )
        );
        for doc in 0..=bytes.len() {
            if doc == 6 {
                continue; // inside the two-byte control character
            }
            assert_eq!(d.to_doc(d.to_display(doc)), doc, "round trip of {doc}");
        }
        assert_eq!(d.to_display(4), 10, "before the composition");
        assert_eq!(d.preedit, Some(10..12));
        assert_eq!(d.to_doc(11), 4, "inside the composition");
    }
}

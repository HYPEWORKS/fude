//! Moving through text: grapheme clusters, words and lines.
//!
//! The rope holds raw bytes. Valid UTF-8 moves by grapheme cluster (so `é` written as
//! `e` + combining accent, an emoji with modifiers, or a CRLF pair is one step); each
//! byte that isn't valid UTF-8 is a step of its own. Everything here looks at a bounded
//! window around the position, so the cost doesn't depend on line or file length.

use fude_buffer::Rope;
use unicode_segmentation::UnicodeSegmentation;

/// Bytes of context read around a position. Grapheme clusters longer than this (only
/// seen in deliberately pathological text) are split.
const WINDOW: usize = 256;

fn bytes(text: &Rope, start: usize, end: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(end - start);
    text.copy_range(start..end, &mut out);
    out
}

/// The grapheme boundary after `at`, or the end of the text.
pub fn next_grapheme(text: &Rope, at: usize) -> usize {
    let len = text.len();
    if at >= len {
        return len;
    }
    let window = bytes(text, at, (at + WINDOW).min(len));
    if window.starts_with(b"\r\n") {
        return at + 2;
    }
    let valid = match std::str::from_utf8(&window) {
        Ok(s) => s,
        Err(e) if e.valid_up_to() == 0 => return at + 1,
        Err(e) => std::str::from_utf8(&window[..e.valid_up_to()]).expect("valid prefix"),
    };
    at + valid.graphemes(true).next().map_or(1, str::len)
}

/// The grapheme boundary before `at`, or 0.
pub fn prev_grapheme(text: &Rope, at: usize) -> usize {
    if at == 0 {
        return 0;
    }
    let start = at.saturating_sub(WINDOW);
    let window = bytes(text, start, at);
    if window.ends_with(b"\r\n") {
        return at - 2;
    }
    let Some(last) = window.utf8_chunks().last() else {
        return at - 1;
    };
    if !last.invalid().is_empty() {
        return at - 1;
    }
    at - last.valid().graphemes(true).next_back().map_or(1, str::len)
}

/// Snaps `at` to the nearest grapheme boundary at or before it.
pub fn floor_grapheme(text: &Rope, at: usize) -> usize {
    let at = at.min(text.len());
    if at == 0 || at == text.len() {
        return at;
    }
    let prev = prev_grapheme(text, at);
    if next_grapheme(text, prev) == at {
        at
    } else {
        prev
    }
}

/// One step of text: a character, or a byte that isn't valid UTF-8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unit {
    Char(char),
    Invalid(u8),
}

impl Unit {
    pub fn len(self) -> usize {
        match self {
            Unit::Char(c) => c.len_utf8(),
            Unit::Invalid(_) => 1,
        }
    }

    pub fn class(self) -> Class {
        match self {
            Unit::Char('\r' | '\n') => Class::LineBreak,
            Unit::Char(c) if c.is_whitespace() => Class::Space,
            Unit::Char(c) if c.is_alphanumeric() || c == '_' => Class::Word,
            Unit::Char(_) => Class::Punct,
            Unit::Invalid(_) => Class::Punct,
        }
    }
}

/// What word motions stop between.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Class {
    Word,
    Punct,
    Space,
    LineBreak,
}

/// Decodes `bytes` into units, starting each window on a character boundary.
fn decode(bytes: &[u8], start: usize, out: &mut Vec<(usize, Unit)>) {
    let mut offset = start;
    for chunk in bytes.utf8_chunks() {
        for c in chunk.valid().chars() {
            out.push((offset, Unit::Char(c)));
            offset += c.len_utf8();
        }
        for &b in chunk.invalid() {
            out.push((offset, Unit::Invalid(b)));
            offset += 1;
        }
    }
}

/// Units after `at`, with their offsets, read a window at a time.
pub(crate) fn units_after(text: &Rope, at: usize) -> impl Iterator<Item = (usize, Unit)> + '_ {
    let mut pos = at;
    let mut buf: Vec<(usize, Unit)> = Vec::new();
    let mut next = 0;
    std::iter::from_fn(move || {
        if next == buf.len() {
            if pos >= text.len() {
                return None;
            }
            // End the window on a character boundary (or at most 3 bytes later).
            let mut end = (pos + WINDOW).min(text.len());
            while end < text.len() && end < pos + WINDOW + 3 && text.byte(end) & 0xC0 == 0x80 {
                end += 1;
            }
            buf.clear();
            decode(&bytes(text, pos, end), pos, &mut buf);
            next = 0;
            pos = end;
        }
        next += 1;
        Some(buf[next - 1])
    })
}

/// Units before `at`, nearest first.
pub(crate) fn units_before(text: &Rope, at: usize) -> impl Iterator<Item = (usize, Unit)> + '_ {
    let mut pos = at;
    let mut buf: Vec<(usize, Unit)> = Vec::new();
    std::iter::from_fn(move || {
        if buf.is_empty() {
            if pos == 0 {
                return None;
            }
            // Start the window on a character boundary (or at most 3 bytes earlier).
            let mut start = pos.saturating_sub(WINDOW);
            while start > 0 && start + WINDOW + 3 > pos && text.byte(start) & 0xC0 == 0x80 {
                start -= 1;
            }
            decode(&bytes(text, start, pos), start, &mut buf);
            pos = start;
        }
        buf.pop()
    })
}

/// How Ctrl+Right (Option+Right on macOS) moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordStop {
    /// To the start of the next word (Windows, Linux).
    Start,
    /// To the end of the current or next word (macOS).
    End,
}

impl WordStop {
    pub const PLATFORM: WordStop = if cfg!(target_os = "macos") {
        WordStop::End
    } else {
        WordStop::Start
    };
}

/// The position a word motion to the right reaches from `at`. A line break is a stop of
/// its own: at the end of a line, the motion goes to the start of the next.
pub fn word_right(text: &Rope, at: usize, stop: WordStop) -> usize {
    let mut units = units_after(text, at).peekable();
    let Some(&(_, first)) = units.peek() else {
        return at;
    };
    if first.class() == Class::LineBreak {
        return next_grapheme(text, at);
    }
    let mut pos = at;
    let skip = |units: &mut std::iter::Peekable<_>, pos: &mut usize, class: Class| {
        while let Some(&(off, u)) = units.peek() {
            let u: Unit = u;
            if u.class() != class {
                break;
            }
            *pos = off + u.len();
            units.next();
        }
    };
    match stop {
        WordStop::Start => {
            if first.class() != Class::Space {
                skip(&mut units, &mut pos, first.class());
            }
            skip(&mut units, &mut pos, Class::Space);
        }
        WordStop::End => {
            skip(&mut units, &mut pos, Class::Space);
            if let Some(&(_, u)) = units.peek()
                && u.class() != Class::LineBreak
            {
                skip(&mut units, &mut pos, u.class());
            }
        }
    }
    pos
}

/// The start of the word before `at` (both platforms): skips spaces, then one run of
/// word or punctuation characters. At the start of a line, moves to the end of the
/// previous one.
pub fn word_left(text: &Rope, at: usize) -> usize {
    let mut units = units_before(text, at).peekable();
    let Some(&(_, first)) = units.peek() else {
        return at;
    };
    if first.class() == Class::LineBreak {
        return prev_grapheme(text, at);
    }
    let mut pos = at;
    let mut skip = |class: Class, pos: &mut usize| {
        while let Some(&(off, u)) = units.peek() {
            if u.class() != class {
                break;
            }
            *pos = off;
            units.next();
        }
    };
    skip(Class::Space, &mut pos);
    let class = units_before(text, pos).next().map(|(_, u)| u.class());
    if let Some(class) = class
        && class != Class::LineBreak
    {
        skip(class, &mut pos);
    }
    pos
}

/// The run of same-class characters around `at` (what a double-click selects). Prefers
/// the character after `at`, unless that is a line break.
pub fn word_range(text: &Rope, at: usize) -> std::ops::Range<usize> {
    let after = units_after(text, at).next();
    let before = units_before(text, at).next();
    let class = match (after, before) {
        (Some((_, u)), _) if u.class() != Class::LineBreak => u.class(),
        (_, Some((_, u))) if u.class() != Class::LineBreak => u.class(),
        _ => return at..at,
    };
    // A "word" can be megabytes of base64; a double click (and every move of the drag
    // after it) looks this far each way at most.
    const MAX: usize = 64 * 1024;
    let mut start = at;
    for (off, u) in units_before(text, at) {
        if u.class() != class || at - off > MAX {
            break;
        }
        start = off;
    }
    let mut end = at;
    for (off, u) in units_after(text, at) {
        if u.class() != class || off - at > MAX {
            break;
        }
        end = off + u.len();
    }
    start..end
}

/// Where the line's indentation (spaces and tabs) ends.
pub fn indent_end(text: &Rope, line_start: usize, line_end: usize) -> usize {
    let mut pos = line_start;
    for chunk in text.chunks_in(line_start..line_end) {
        match chunk.iter().position(|&b| b != b' ' && b != b'\t') {
            Some(i) => return pos + i,
            None => pos += chunk.len(),
        }
    }
    pos
}

/// The 1-based column of `at` for display: grapheme clusters, with tabs expanded to
/// `tab_width` stops. Lines longer than a megabyte count bytes past that point.
pub fn display_column(text: &Rope, line_start: usize, at: usize, tab_width: usize) -> usize {
    // Counted exactly this far into a line; past it, a byte is a column (exact for ASCII),
    // so a cursor deep in a huge line costs no more than one near its start.
    const LIMIT: usize = 64 * 1024;
    let counted_end = at.min(line_start + LIMIT);
    let prefix = bytes(text, line_start, counted_end);
    let tab = tab_width.max(1);
    let mut col = 0;
    for chunk in prefix.utf8_chunks() {
        let valid = chunk.valid();
        if valid.is_ascii() {
            for b in valid.bytes() {
                col = if b == b'\t' {
                    (col / tab + 1) * tab
                } else {
                    col + 1
                };
            }
        } else {
            for g in valid.graphemes(true) {
                col = if g == "\t" {
                    (col / tab + 1) * tab
                } else {
                    col + 1
                };
            }
        }
        col += chunk.invalid().len();
    }
    col + (at - counted_end) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(s: &[u8]) -> Rope {
        Rope::from(s)
    }

    /// All grapheme boundaries reached by stepping right, then left.
    fn steps(s: &[u8]) -> (Vec<usize>, Vec<usize>) {
        let t = rope(s);
        let mut right = vec![0];
        while *right.last().unwrap() < t.len() {
            right.push(next_grapheme(&t, *right.last().unwrap()));
        }
        let mut left = vec![t.len()];
        while *left.last().unwrap() > 0 {
            left.push(prev_grapheme(&t, *left.last().unwrap()));
        }
        left.reverse();
        (right, left)
    }

    #[test]
    fn graphemes_crlf_combining_emoji_and_invalid_bytes() {
        // a, CRLF, e + combining acute, family emoji (ZWJ sequence), invalid byte, flag.
        let s = "a\r\ne\u{301}👨‍👩‍👧\u{FF}🇯🇵".as_bytes().to_vec();
        let mut s = s;
        // Replace the U+00FF (two bytes) with a single invalid byte.
        let ff = s.windows(2).position(|w| w == "\u{FF}".as_bytes()).unwrap();
        s.splice(ff..ff + 2, [0xFF]);
        let (right, left) = steps(&s);
        assert_eq!(right, left, "both directions agree");
        let lens: Vec<usize> = right.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(lens, [1, 2, 3, "👨‍👩‍👧".len(), 1, "🇯🇵".len()]);
    }

    #[test]
    fn floor_snaps_into_clusters() {
        let t = rope("ae\u{301}b".as_bytes());
        assert_eq!(floor_grapheme(&t, 2), 1, "inside e + accent");
        assert_eq!(floor_grapheme(&t, 4), 4);
    }

    #[test]
    fn word_motions() {
        let t = rope(b"foo.bar  baz\nnext");
        let stops = |mut at: usize, f: &dyn Fn(usize) -> usize| {
            let mut v = vec![];
            loop {
                let n = f(at);
                if n == at {
                    return v;
                }
                v.push(n);
                at = n;
            }
        };
        assert_eq!(
            stops(0, &|at| word_right(&t, at, WordStop::Start)),
            [3, 4, 9, 12, 13, 17]
        );
        assert_eq!(
            stops(0, &|at| word_right(&t, at, WordStop::End)),
            [3, 4, 7, 12, 13, 17]
        );
        assert_eq!(stops(17, &|at| word_left(&t, at)), [13, 12, 9, 4, 3, 0]);
    }

    #[test]
    fn word_range_for_double_click() {
        let t = rope(b"let foo_bar = 1;");
        assert_eq!(word_range(&t, 5), 4..11);
        assert_eq!(word_range(&t, 11), 11..12, "space after the word");
        assert_eq!(
            word_range(&t, 16),
            15..16,
            "end of line takes the char before"
        );
    }

    #[test]
    fn long_words_cross_windows() {
        let mut s = vec![b'x'; 1000];
        s.extend_from_slice(b" y");
        let t = rope(&s);
        assert_eq!(word_right(&t, 0, WordStop::Start), 1001);
        assert_eq!(word_left(&t, 1000), 0);
    }

    #[test]
    fn columns_expand_tabs() {
        let t = rope("\tab\tc筆".as_bytes());
        assert_eq!(display_column(&t, 0, 0, 4), 1);
        assert_eq!(display_column(&t, 0, 1, 4), 5);
        assert_eq!(display_column(&t, 0, 4, 4), 9);
        assert_eq!(display_column(&t, 0, t.len(), 4), 11);
    }
}

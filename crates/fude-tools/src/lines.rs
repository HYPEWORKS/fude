//! Tools on lines: sorting, removing duplicates, reversing, trimming, indentation.
//!
//! Tools that reorder or drop lines join them with the text's first line break (so a
//! CRLF file stays CRLF) and keep a final line break only if the text had one.

use std::cmp::Ordering;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Bytes,
    IgnoringCase,
    /// Runs of digits compare as numbers: "file2" before "file10".
    Natural,
}

/// The lines of `text` (without breaks), its first line break, and whether it ends with
/// one.
fn split(text: &[u8]) -> (Vec<&[u8]>, &'static [u8], bool) {
    let le: &'static [u8] = match text.iter().position(|&b| b == b'\n' || b == b'\r') {
        Some(i) if text[i] == b'\r' && text.get(i + 1) == Some(&b'\n') => b"\r\n",
        Some(i) if text[i] == b'\r' => b"\r",
        _ => b"\n",
    };
    let mut lines = Vec::new();
    let mut rest = text;
    let mut ended = false;
    while !rest.is_empty() {
        match rest.iter().position(|&b| b == b'\n' || b == b'\r') {
            Some(i) => {
                lines.push(&rest[..i]);
                let crlf = rest[i] == b'\r' && rest.get(i + 1) == Some(&b'\n');
                rest = &rest[i + 1 + usize::from(crlf)..];
                ended = rest.is_empty();
            }
            None => {
                lines.push(rest);
                rest = &[];
                ended = false;
            }
        }
    }
    (lines, le, ended)
}

fn join(lines: &[&[u8]], le: &[u8], ended: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(le);
        }
        out.extend_from_slice(line);
    }
    if ended && !lines.is_empty() {
        out.extend_from_slice(le);
    }
    out
}

pub fn sort(text: &[u8], order: Order, descending: bool) -> Vec<u8> {
    let (mut lines, le, ended) = split(text);
    match order {
        Order::Bytes => lines.sort(),
        Order::IgnoringCase => lines.sort_by_cached_key(|l| lower(l)),
        Order::Natural => lines.sort_by(|a, b| natural(a, b)),
    }
    if descending {
        lines.reverse();
    }
    join(&lines, le, ended)
}

fn lower(line: &[u8]) -> Vec<u8> {
    match std::str::from_utf8(line) {
        Ok(s) => s.to_lowercase().into_bytes(),
        Err(_) => line.to_ascii_lowercase(),
    }
}

/// Compares runs of digits by value, everything else byte by byte.
fn natural(a: &[u8], b: &[u8]) -> Ordering {
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let ea = i + a[i..].iter().take_while(|c| c.is_ascii_digit()).count();
            let eb = j + b[j..].iter().take_while(|c| c.is_ascii_digit()).count();
            let na = trim_zeros(&a[i..ea]);
            let nb = trim_zeros(&b[j..eb]);
            let by_value = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
            if by_value != Ordering::Equal {
                return by_value;
            }
            (i, j) = (ea, eb);
        } else {
            match a[i].cmp(&b[j]) {
                Ordering::Equal => (i, j) = (i + 1, j + 1),
                other => return other,
            }
        }
    }
    (a.len() - i).cmp(&(b.len() - j))
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let zeros = digits.iter().take_while(|&&d| d == b'0').count();
    &digits[zeros.min(digits.len() - 1)..]
}

/// Keeps the first of each set of identical lines.
pub fn unique(text: &[u8]) -> Vec<u8> {
    let (lines, le, ended) = split(text);
    let mut seen = std::collections::HashSet::new();
    let kept: Vec<&[u8]> = lines.into_iter().filter(|l| seen.insert(*l)).collect();
    join(&kept, le, ended)
}

pub fn reverse(text: &[u8]) -> Vec<u8> {
    let (mut lines, le, ended) = split(text);
    lines.reverse();
    join(&lines, le, ended)
}

/// Calls `f` with each line's content and its line break (empty for the last line
/// without one), building the output from what it pushes.
fn each_line(text: &[u8], mut f: impl FnMut(&[u8], &mut Vec<u8>)) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let (line, brk, next) = match rest.iter().position(|&b| b == b'\n' || b == b'\r') {
            Some(i) => {
                let n = 1 + usize::from(rest[i] == b'\r' && rest.get(i + 1) == Some(&b'\n'));
                (&rest[..i], &rest[i..i + n], &rest[i + n..])
            }
            None => (rest, &rest[rest.len()..], &rest[rest.len()..]),
        };
        f(line, &mut out);
        out.extend_from_slice(brk);
        rest = next;
    }
    out
}

pub fn trim_trailing(text: &[u8]) -> Vec<u8> {
    each_line(text, |line, out| {
        out.extend_from_slice(line.trim_ascii_end())
    })
}

/// Leading tabs become spaces, to the same tab stops.
pub fn indent_with_spaces(text: &[u8], tab_width: usize) -> Vec<u8> {
    let tab = tab_width.max(1);
    each_line(text, |line, out| {
        let lead = line
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
        let mut col = 0;
        for &b in &line[..lead] {
            col = if b == b'\t' {
                (col / tab + 1) * tab
            } else {
                col + 1
            };
        }
        out.extend(std::iter::repeat_n(b' ', col));
        out.extend_from_slice(&line[lead..]);
    })
}

/// Leading spaces become tabs where they fill whole tab stops.
pub fn indent_with_tabs(text: &[u8], tab_width: usize) -> Vec<u8> {
    let tab = tab_width.max(1);
    each_line(text, |line, out| {
        let lead = line
            .iter()
            .take_while(|&&b| b == b' ' || b == b'\t')
            .count();
        let mut col = 0;
        for &b in &line[..lead] {
            col = if b == b'\t' {
                (col / tab + 1) * tab
            } else {
                col + 1
            };
        }
        out.extend(std::iter::repeat_n(b'\t', col / tab));
        out.extend(std::iter::repeat_n(b' ', col % tab));
        out.extend_from_slice(&line[lead..]);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: Vec<u8>) -> String {
        String::from_utf8(v).unwrap()
    }

    #[test]
    fn sorting() {
        assert_eq!(s(sort(b"b\nC\na\n", Order::Bytes, false)), "C\na\nb\n");
        assert_eq!(
            s(sort(b"b\r\nC\r\na", Order::IgnoringCase, false)),
            "a\r\nb\r\nC"
        );
        assert_eq!(s(sort(b"b\nc\na", Order::Bytes, true)), "c\nb\na");
        assert_eq!(
            s(sort(
                b"file10\nfile2\nfile02x\nfile1",
                Order::Natural,
                false
            )),
            "file1\nfile2\nfile02x\nfile10"
        );
        assert_eq!(s(sort(b"", Order::Bytes, false)), "");
    }

    #[test]
    fn unique_reverse_trim() {
        assert_eq!(s(unique(b"a\nb\na\nc\nb\n")), "a\nb\nc\n");
        assert_eq!(s(reverse(b"1\n2\n3")), "3\n2\n1");
        assert_eq!(s(trim_trailing(b"a  \nb\t\r\n  c ")), "a\nb\r\n  c");
    }

    #[test]
    fn indentation() {
        assert_eq!(s(indent_with_spaces(b"\tx\n \ty\n", 4)), "    x\n    y\n");
        assert_eq!(s(indent_with_tabs(b"      x\n    y", 4)), "\t  x\n\ty");
    }
}

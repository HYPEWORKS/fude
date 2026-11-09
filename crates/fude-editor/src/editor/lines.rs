//! Commands on whole lines: indenting, duplicating, moving, deleting, joining, opening a
//! line, and comments.

use std::ops::Range;
use std::time::Instant;

use super::Editor;
use crate::selection::{Selection, Selections};
use crate::text::{floor_grapheme, indent_end};

impl Editor {
    /// The lines the selections cover, as (first, last) line numbers, sorted, with blocks
    /// that overlap or touch merged. A selection ending at the start of a line doesn't
    /// take that line.
    pub fn line_blocks(&self) -> Vec<(usize, usize)> {
        let text = self.text();
        let mut blocks: Vec<(usize, usize)> = Vec::new();
        for s in self.selections.iter() {
            let first = text.byte_to_line(s.start());
            let mut last = text.byte_to_line(s.end());
            if last > first && text.line_to_byte(last) == s.end() {
                last -= 1;
            }
            match blocks.last_mut() {
                Some(b) if first <= b.1 + 1 => b.1 = b.1.max(last),
                _ => blocks.push((first, last)),
            }
        }
        blocks
    }

    /// Whether some selection spans more than one line (Tab then indents instead of typing).
    pub fn spans_lines(&self) -> bool {
        let text = self.text();
        self.selections.iter().any(|s| {
            let first = text.byte_to_line(s.start());
            let last = text.byte_to_line(s.end());
            last > first
        })
    }

    /// Tab with lines selected, Ctrl+]: indents each selected line (but empty ones).
    pub fn indent_lines(&mut self) {
        let text = self.text().clone();
        let unit = self.indent_unit();
        let mut edits = Vec::new();
        for (first, last) in self.line_blocks() {
            for line in first..=last {
                let span = text.line_span(line);
                if span.end > span.start {
                    edits.push((span.start..span.start, unit.clone()));
                }
            }
        }
        self.apply_line_edits(&edits);
    }

    /// Shift+Tab, Ctrl+[: removes one level of indentation from each selected line (a tab,
    /// or up to a tab's width of spaces).
    pub fn outdent_lines(&mut self) {
        let text = self.text().clone();
        let width = self.settings.tab_width.max(1);
        let mut edits = Vec::new();
        for (first, last) in self.line_blocks() {
            for line in first..=last {
                let span = text.line_span(line);
                let mut head = Vec::new();
                text.copy_range(span.start..(span.start + width).min(span.end), &mut head);
                let n = if head.first() == Some(&b'\t') {
                    1
                } else {
                    head.iter().take_while(|&&b| b == b' ').count()
                };
                if n > 0 {
                    edits.push((span.start..span.start + n, Vec::new()));
                }
            }
        }
        self.apply_line_edits(&edits);
    }

    /// Ctrl+Shift+D. Each selection is copied right after itself, and the copy selected;
    /// with no text selected, the cursors' lines are copied below and the cursors move to
    /// the copies.
    pub fn duplicate(&mut self) {
        let text = self.text().clone();
        let mut edits = Vec::new();
        let mut delta = 0usize;
        if self.selections.iter().all(Selection::is_empty) {
            let le = self.format().line_ending.as_bytes();
            let blocks = self.line_blocks();
            let mut shifts = Vec::new();
            for (first, last) in blocks {
                let (start, end) = (text.line_to_byte(first), text.line_span(last));
                let mut copy = Vec::new();
                let at = if end.next > end.end {
                    text.copy_range(start..end.next, &mut copy);
                    end.next
                } else {
                    // The last line has no line break: the copy starts with one.
                    copy.extend_from_slice(le);
                    text.copy_range(start..end.end, &mut copy);
                    end.end
                };
                delta += copy.len();
                shifts.push((start..at, delta));
                edits.push((at..at, copy));
            }
            let after = self.selections.map(|s| {
                let (prior, shift) = block_shift(&shifts, s.head);
                Selection {
                    anchor: s.anchor + prior + shift,
                    head: s.head + prior + shift,
                    goal_x: s.goal_x,
                }
            });
            self.commit(&edits, after);
            return;
        }
        let mut ranges = Vec::new();
        for s in self.selections.iter() {
            let mut copy = Vec::new();
            text.copy_range(s.range(), &mut copy);
            let len = copy.len();
            let start = s.end() + delta;
            edits.push((s.end()..s.end(), copy));
            ranges.push(if s.head >= s.anchor {
                Selection::new(start, start + len)
            } else {
                Selection::new(start + len, start)
            });
            delta += len;
        }
        let after = Selections::new(ranges, self.selections.primary_index());
        self.commit(&edits, after);
    }

    /// Alt+Up / Alt+Down: moves the selected lines (or the cursors' lines) past the line
    /// above or below; the selections move with them. Each line keeps its text; line
    /// breaks stay where they were, so a last line without one stays without one.
    pub fn move_lines(&mut self, down: bool) {
        let text = self.text().clone();
        let lines = text.line_count();
        let blocks = self.line_blocks();
        if blocks
            .iter()
            .any(|&(first, last)| if down { last + 1 >= lines } else { first == 0 })
        {
            return;
        }
        let mut edits = Vec::new();
        // Where each line of each moved region starts after the move: (old line, new start).
        let mut moved: Vec<(usize, usize)> = Vec::new();
        for (first, last) in blocks {
            let (lo, hi) = if down {
                (first, last + 1)
            } else {
                (first - 1, last)
            };
            let spans: Vec<_> = (lo..=hi).map(|l| text.line_span(l)).collect();
            let mut order: Vec<usize> = (lo..=hi).collect();
            if down {
                order.rotate_right(1);
            } else {
                order.rotate_left(1);
            }
            let region = spans[0].start..spans[spans.len() - 1].next;
            let mut out = Vec::with_capacity(region.len());
            for (k, &line) in order.iter().enumerate() {
                let span = spans[line - lo];
                moved.push((line, region.start + out.len()));
                text.copy_range(span.start..span.end, &mut out);
                // The break that was at this position.
                text.copy_range(spans[k].end..spans[k].next, &mut out);
            }
            edits.push((region, out));
        }
        let place = |at: usize| {
            let line = text.byte_to_line(at);
            let col = at - text.line_to_byte(line);
            moved
                .iter()
                .find(|m| m.0 == line)
                .map_or(at, |&(_, start)| start + col)
        };
        let after = self.selections.map(|s| Selection {
            anchor: place(s.anchor),
            head: place(s.head),
            goal_x: s.goal_x,
        });
        self.commit(&edits, after);
    }

    /// Ctrl+Shift+K: deletes the selected lines (or the cursors' lines), line breaks and
    /// all. Cursors go to the start of the line that follows.
    pub fn delete_lines(&mut self) {
        let text = self.text().clone();
        let mut edits = Vec::new();
        let mut cursors = Vec::new();
        let mut removed = 0usize;
        for (first, last) in self.line_blocks() {
            let span = text.line_span(last);
            let mut range = text.line_to_byte(first)..span.next;
            if span.next == span.end && first > 0 {
                // The last line: take the break before it instead.
                range.start = text.line_span(first - 1).end;
            }
            cursors.push(Selection::cursor(range.start - removed));
            removed += range.len();
            edits.push((range, Vec::new()));
        }
        let primary = cursors.len() - 1;
        self.commit(&edits, Selections::new(cursors, primary));
    }

    /// Ctrl+J: joins each selection's lines, or a cursor's line and the next, with a
    /// single space in place of the line break and the next line's indentation.
    pub fn join_lines(&mut self) {
        let text = self.text().clone();
        let last_line = text.line_count() - 1;
        let mut edits: Vec<(Range<usize>, Vec<u8>)> = Vec::new();
        for (first, last) in self.line_blocks() {
            let last = last.max(first + 1).min(last_line);
            for line in first..last {
                let span = text.line_span(line);
                let next = text.line_span(line + 1);
                let next_text = indent_end(&text, next.start, next.end);
                let ends_blank =
                    span.end == span.start || matches!(text.byte(span.end - 1), b' ' | b'\t');
                let space = if ends_blank || next_text == next.end {
                    Vec::new()
                } else {
                    vec![b' ']
                };
                edits.push((span.end..next_text, space));
            }
        }
        self.apply_line_edits(&edits);
    }

    /// Ctrl+Enter: a new line below each cursor's line, with its indentation;
    /// Ctrl+Shift+Enter: above.
    pub fn insert_line(&mut self, above: bool, now: Instant) {
        let text = self.text().clone();
        let le = self.format().line_ending.as_bytes().to_vec();
        let reps = self
            .each(|_, s| {
                let span = text.line_span(text.byte_to_line(s.head));
                let mut indent = Vec::new();
                text.copy_range(
                    span.start..indent_end(&text, span.start, span.end),
                    &mut indent,
                );
                Some(if above {
                    let n = indent.len();
                    indent.extend_from_slice(&le);
                    (span.start..span.start, indent, n)
                } else {
                    let mut out = le.clone();
                    out.extend_from_slice(&indent);
                    let n = out.len();
                    (span.end..span.end, out, n)
                })
            })
            .collect();
        self.apply(reps, None, now, false, false);
    }

    /// Ctrl+/: comments out the selected lines with `token` (`//`, `#`…) at their common
    /// indentation, or, if every non-blank one already is, uncomments them.
    pub fn toggle_line_comment(&mut self, token: &str) {
        let text = self.text().clone();
        let token = token.as_bytes();
        let blocks = self.line_blocks();
        let lines: Vec<usize> = blocks.iter().flat_map(|&(a, b)| a..=b).collect();
        // Each line's span, where its text starts, and whether it's blank.
        let info: Vec<_> = lines
            .iter()
            .map(|&l| {
                let span = text.line_span(l);
                let at = indent_end(&text, span.start, span.end);
                (span, at, at == span.end)
            })
            .collect();
        let starts_with = |at: usize, end: usize, s: &[u8]| {
            end - at >= s.len() && {
                let mut b = Vec::new();
                text.copy_range(at..at + s.len(), &mut b);
                b == s
            }
        };
        let all_blank = info.iter().all(|i| i.2);
        let commented = !all_blank
            && info
                .iter()
                .all(|&(span, at, blank)| blank || starts_with(at, span.end, token));
        let mut edits = Vec::new();
        if commented {
            for &(span, at, blank) in &info {
                if !blank {
                    let space = usize::from(starts_with(at + token.len(), span.end, b" "));
                    edits.push((at..at + token.len() + space, Vec::new()));
                }
            }
        } else {
            let column = info
                .iter()
                .filter(|i| all_blank || !i.2)
                .map(|&(span, at, _)| at - span.start)
                .min()
                .unwrap_or(0);
            let mut insert = token.to_vec();
            insert.push(b' ');
            for &(span, _, blank) in &info {
                if all_blank || !blank {
                    let at = span.start + column;
                    edits.push((at..at, insert.clone()));
                }
            }
        }
        self.apply_line_edits(&edits);
    }

    /// Wraps each selection in `open` … `close` (a cursor gets an empty comment around
    /// it), or unwraps each if they all already are.
    pub fn toggle_block_comment(&mut self, open: &str, close: &str) {
        let text = self.text().clone();
        let (open, close) = (open.as_bytes(), close.as_bytes());
        let contents: Vec<Vec<u8>> = self
            .selections
            .iter()
            .map(|s| {
                let mut b = Vec::new();
                text.copy_range(s.range(), &mut b);
                b
            })
            .collect();
        let wrapped = |b: &[u8]| {
            b.len() >= open.len() + close.len() && b.starts_with(open) && b.ends_with(close)
        };
        let unwrap = contents.iter().all(|b| wrapped(b.trim_ascii()));
        let mut edits = Vec::new();
        let mut ranges = Vec::new();
        let mut delta = 0isize;
        for (s, b) in self.selections.iter().zip(&contents) {
            let start = s.start() as isize + delta;
            let new = if unwrap {
                let lead = b.len() - b.trim_ascii_start().len();
                let inner = &b.trim_ascii()[open.len()..b.trim_ascii().len() - close.len()];
                let inner = inner.strip_prefix(b" ").unwrap_or(inner);
                let inner = inner.strip_suffix(b" ").unwrap_or(inner);
                let trail = b.len() - b.trim_ascii_end().len();
                [&b[..lead], inner, &b[b.len() - trail..]].concat()
            } else {
                [open, b" ", b, b" ", close].concat()
            };
            let new_start = start as usize;
            ranges.push(if s.is_empty() {
                Selection::cursor(new_start + open.len() + 1)
            } else if s.head >= s.anchor {
                Selection::new(new_start, new_start + new.len())
            } else {
                Selection::new(new_start + new.len(), new_start)
            });
            delta += new.len() as isize - b.len() as isize;
            edits.push((s.range(), new));
        }
        let after = Selections::new(ranges, self.selections.primary_index());
        self.commit(&edits, after);
    }

    /// Replaces the text of each selection, or with nothing selected the whole text, with
    /// what `f` makes of it: what text tools do. Each result stays selected. Returns the
    /// first error, changing nothing.
    pub fn transform(
        &mut self,
        mut f: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    ) -> Result<(), String> {
        let ranges: Vec<Range<usize>> = if self.selections.iter().all(Selection::is_empty) {
            std::iter::once(0..self.text().len()).collect()
        } else {
            self.selections
                .iter()
                .filter(|s| !s.is_empty())
                .map(Selection::range)
                .collect()
        };
        let text = self.text().clone();
        let mut edits = Vec::new();
        for r in ranges {
            let mut b = Vec::new();
            text.copy_range(r.clone(), &mut b);
            let out = f(&b)?;
            if out != b {
                edits.push((r, out));
            }
        }
        self.replace_ranges(edits);
        Ok(())
    }

    /// Replaces ranges (sorted, non-overlapping) as one undo step, selecting what each
    /// became. For results computed elsewhere (a tool on a worker).
    pub fn replace_ranges(&mut self, edits: Vec<(Range<usize>, Vec<u8>)>) {
        let whole = edits.len() == 1 && edits[0].0 == (0..self.text().len());
        let mut delta = 0isize;
        let mut ranges = Vec::new();
        for (r, out) in &edits {
            let start = (r.start as isize + delta) as usize;
            ranges.push(Selection::new(start, start + out.len()));
            delta += out.len() as isize - r.len() as isize;
        }
        if edits.is_empty() {
            return;
        }
        let at = self.selections.primary().head;
        let after = if whole {
            // A whole-document change keeps the cursor near where it was.
            let len = (self.text().len() as isize + delta) as usize;
            Selections::single(Selection::cursor(at.min(len)))
        } else {
            Selections::new(ranges, 0)
        };
        self.commit(&edits, after);
        if whole {
            let at = floor_grapheme(self.text(), self.selections.primary().head);
            self.selections = Selections::single(Selection::cursor(at));
        }
    }
}

/// For a cursor at `at`: how far the blocks before its own moved it, and how far its own
/// block's copy moves it (onto the copy).
fn block_shift(shifts: &[(Range<usize>, usize)], at: usize) -> (usize, usize) {
    let mut prior = 0;
    for (range, total) in shifts {
        let own = total - prior;
        if range.contains(&at) || at == range.end {
            return (prior, own);
        }
        prior = *total;
    }
    (prior, 0)
}

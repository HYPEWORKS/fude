//! Multiple cursors: added by click, above and below, at occurrences of the selection,
//! and per line of a selection; column (box) selection.

use std::ops::Range;

use fude_buffer::Rope;

use super::Editor;
use crate::layout::Layout;
use crate::search::{Query, Searcher};
use crate::selection::{Selection, Selections};
use crate::text::word_range;

/// No command makes more cursors than this: each keystroke costs time per cursor, and
/// "select all occurrences" of a common word could otherwise make millions.
pub const MAX_CURSORS: usize = 10_000;

impl Editor {
    /// Ctrl+Click: adds a cursor at `at`, or, with several selections, removes the one
    /// there.
    pub fn toggle_cursor(&mut self, at: usize) {
        let sels = &self.selections;
        let here = sels.iter().position(|s| s.start() <= at && at <= s.end());
        let next = match here {
            Some(i) if sels.len() > 1 => sels.without(i),
            _ if sels.len() < MAX_CURSORS => sels.with(Selection::cursor(at)),
            _ => return,
        };
        self.set_selections(next);
    }

    /// Adds a cursor `rows` rows below the lowest cursor (above the highest, if negative),
    /// aiming for the same x. Returns false at the start or end of the text.
    pub fn add_cursor_vertical(&mut self, rows: isize, layout: &mut dyn Layout) -> bool {
        let sels = &self.selections;
        if sels.len() >= MAX_CURSORS {
            return false;
        }
        let edge = if rows > 0 {
            sels.iter().max_by_key(|s| s.head)
        } else {
            sels.iter().min_by_key(|s| s.head)
        };
        let edge = *edge.expect("never empty");
        let text = self.text().clone();
        let (at, goal) = layout.vertical(&text, edge.head, edge.goal_x, rows);
        if goal.is_none() {
            return false;
        }
        let next = sels.with(Selection {
            anchor: at,
            head: at,
            goal_x: goal,
        });
        self.set_selections(next);
        true
    }

    /// Ctrl+D. With a bare cursor, selects the word around it; then each press adds the
    /// next occurrence of the selected text (whole words only, if it began as a word),
    /// wrapping around. Returns false if there was nothing to add.
    pub fn select_next_occurrence(&mut self) -> bool {
        let text = self.text().clone();
        let primary = *self.selections.primary();
        if primary.is_empty() {
            let word = word_range(&text, primary.head);
            if word.is_empty() {
                return false;
            }
            let i = self.selections.primary_index();
            let next = self
                .selections
                .without(i)
                .with(Selection::new(word.start, word.end));
            self.set_selections(if self.selections.len() == 1 {
                Selections::single(Selection::new(word.start, word.end))
            } else {
                next
            });
            self.occurrence_word = true;
            return true;
        }
        if self.selections.len() >= MAX_CURSORS {
            return false;
        }
        let word = self.occurrence_word;
        let Some(searcher) = occurrence_searcher(&text, primary.range(), word) else {
            return false;
        };
        let mut from = primary.end();
        for _ in 0..=self.selections.len() {
            let Some(found) = searcher.next(&text, from) else {
                return false;
            };
            let taken = self
                .selections
                .iter()
                .any(|s| s.start() < found.end && found.start < s.end());
            if !taken {
                let next = self.selections.with(Selection::new(found.start, found.end));
                self.set_selections(next);
                self.occurrence_word = word;
                return true;
            }
            from = found.end;
        }
        false
    }

    /// Ctrl+Shift+L: selects every occurrence of the selection (or of the word at the
    /// cursor), up to [`MAX_CURSORS`]. Returns how many there are.
    pub fn select_all_occurrences(&mut self) -> usize {
        if self.selections.primary().is_empty() && !self.select_next_occurrence() {
            return 0;
        }
        let text = self.text().clone();
        let primary = *self.selections.primary();
        let word = self.occurrence_word;
        let Some(searcher) = occurrence_searcher(&text, primary.range(), word) else {
            return 0;
        };
        let mut found = Vec::new();
        let mut from = 0;
        let mut primary_index = 0;
        while found.len() < MAX_CURSORS {
            let Some(r) = searcher.find_from(&text, from) else {
                break;
            };
            if r.start == primary.start() {
                primary_index = found.len();
            }
            from = r.end;
            found.push(Selection::new(r.start, r.end));
        }
        let n = found.len();
        if n > 0 {
            self.set_selections(Selections::new(found, primary_index));
            self.occurrence_word = word;
        }
        n
    }

    /// Splits each selection into one per line it covers (a selection that ends at the
    /// start of a line doesn't take that line).
    pub fn split_into_lines(&mut self) {
        let text = self.text().clone();
        let mut out = Vec::new();
        let mut primary = 0;
        for (i, s) in self.selections.iter().enumerate() {
            if i == self.selections.primary_index() {
                primary = out.len();
            }
            let (first, last) = (text.byte_to_line(s.start()), text.byte_to_line(s.end()));
            if first == last || s.is_empty() {
                out.push(*s);
                continue;
            }
            for line in first..=last {
                let span = text.line_span(line);
                let (a, b) = (s.start().max(span.start), s.end().min(span.end));
                if line == last && s.end() == span.start {
                    break;
                }
                out.push(Selection::new(a, b));
                if out.len() >= MAX_CURSORS {
                    break;
                }
            }
        }
        self.set_selections(Selections::new(out, primary));
    }

    /// A column (box) selection from line `anchor.0` at x `anchor.1` to line `head.0` at
    /// x `head.1`: on each line between, the text between those x positions (a line too
    /// short gets a cursor at its end).
    pub fn select_column(
        &mut self,
        anchor: (usize, f32),
        head: (usize, f32),
        layout: &mut dyn Layout,
    ) {
        let text = self.text().clone();
        let last = text.line_count() - 1;
        let (a, h) = (anchor.0.min(last), head.0.min(last));
        let lines: Vec<usize> = if a <= h {
            (a..=h).collect()
        } else {
            (h..=a).rev().collect()
        };
        // The cap keeps the lines nearest the head (where the user is looking).
        let lines = &lines[lines.len().saturating_sub(MAX_CURSORS)..];
        let ranges: Vec<Selection> = lines
            .iter()
            .map(|&line| Selection {
                anchor: layout.offset_at(&text, line, anchor.1),
                head: layout.offset_at(&text, line, head.1),
                goal_x: Some(head.1),
            })
            .collect();
        let primary = ranges.len() - 1;
        self.set_selections(Selections::new(ranges, primary));
    }
}

/// A literal, case-sensitive search for the text in `range` (whole words only if `word`).
fn occurrence_searcher(text: &Rope, range: Range<usize>, word: bool) -> Option<Searcher> {
    let mut bytes = Vec::new();
    text.copy_range(range, &mut bytes);
    let query = Query {
        text: String::from_utf8(bytes).ok()?,
        regex: false,
        case_sensitive: true,
        whole_word: word,
    };
    Searcher::new(&query).ok().flatten()
}

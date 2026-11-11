//! Multiple cursors: added by click, above and below, at occurrences of the selection,
//! and per line of a selection; column (box) selection.

use std::ops::Range;

use fude_buffer::Rope;

use super::Editor;
use crate::layout::Layout;
use crate::search::{Query, Searcher, Stop, Stopped};
use crate::selection::{Selection, Selections};
use crate::text::word_range;

/// No command makes more cursors than this: each keystroke costs time per cursor, and
/// "select all occurrences" of a common word could otherwise make millions.
pub const MAX_CURSORS: usize = 10_000;

/// A search for the occurrences of the selected text, to be run wherever there's time for
/// it: [`Editor::select_next_occurrence`] and [`Editor::select_all_occurrences`] run it
/// on the spot; an application can give those a deadline and, when it passes, run this
/// on a worker thread and hand the result back to [`Editor::add_occurrence`] or
/// [`Editor::set_occurrences`].
#[derive(Clone, Debug)]
pub struct Occurrences {
    searcher: Searcher,
    /// The selections when the search began: occurrences already selected are skipped.
    selections: Selections,
}

impl Occurrences {
    /// The selections the search began with. Its result is for these, and no others.
    pub fn selections(&self) -> &Selections {
        &self.selections
    }

    /// The next occurrence after the primary selection that isn't selected yet, wrapping
    /// around. None at [`MAX_CURSORS`].
    pub fn next(&self, text: &Rope, stop: &Stop<'_>) -> Result<Option<Range<usize>>, Stopped> {
        if self.selections.len() >= MAX_CURSORS {
            return Ok(None);
        }
        let mut from = self.selections.primary().end();
        for _ in 0..=self.selections.len() {
            let Some(found) = self.searcher.next_until(text, from, stop)? else {
                return Ok(None);
            };
            let taken = self
                .selections
                .iter()
                .any(|s| s.start() < found.end && found.start < s.end());
            if !taken {
                return Ok(Some(found));
            }
            from = found.end;
        }
        Ok(None)
    }

    /// Every occurrence, up to [`MAX_CURSORS`].
    pub fn all(&self, text: &Rope, stop: &Stop<'_>) -> Result<Vec<Range<usize>>, Stopped> {
        let mut found = Vec::new();
        let mut from = 0;
        while found.len() < MAX_CURSORS {
            let Some(r) = self.searcher.find_from_until(text, from, stop)? else {
                break;
            };
            from = r.end;
            found.push(r);
        }
        Ok(found)
    }
}

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
        self.select_next_occurrence_until(&Stop::never())
            .unwrap_or(false)
    }

    /// [`Editor::select_next_occurrence`], giving up when `stop` says: then nothing has
    /// changed, and [`Editor::occurrences`] is the search to finish elsewhere.
    pub fn select_next_occurrence_until(&mut self, stop: &Stop<'_>) -> Result<bool, Stopped> {
        if self.select_word_at_cursor() {
            return Ok(true);
        }
        let Some(search) = self.occurrences() else {
            return Ok(false);
        };
        let found = search.next(self.text(), stop)?;
        Ok(found.is_some_and(|range| self.add_occurrence(&search, range)))
    }

    /// With a bare primary cursor on a word, selects the word (the first step of Ctrl+D
    /// and Ctrl+Shift+L). Returns whether it did.
    fn select_word_at_cursor(&mut self) -> bool {
        let text = self.text().clone();
        let primary = *self.selections.primary();
        if !primary.is_empty() {
            return false;
        }
        {
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
            true
        }
    }

    /// The search for occurrences of the primary selection's text (whole words only, if
    /// the selection began as the word at a cursor). `None` with nothing selected, or
    /// with text that can't be searched for.
    pub fn occurrences(&self) -> Option<Occurrences> {
        let primary = self.selections.primary();
        if primary.is_empty() {
            return None;
        }
        let searcher = occurrence_searcher(self.text(), primary.range(), self.occurrence_word)?;
        Some(Occurrences {
            searcher,
            selections: self.selections.clone(),
        })
    }

    /// Adds `range`, which `search` found, to the selections. Returns false, changing
    /// nothing, if the selections aren't the ones the search began with any more.
    pub fn add_occurrence(&mut self, search: &Occurrences, range: Range<usize>) -> bool {
        if self.selections != search.selections {
            return false;
        }
        let word = self.occurrence_word;
        let next = self.selections.with(Selection::new(range.start, range.end));
        self.set_selections(next);
        self.occurrence_word = word;
        true
    }

    /// Ctrl+Shift+L: selects every occurrence of the selection (or of the word at the
    /// cursor), up to [`MAX_CURSORS`]. Returns how many there are.
    pub fn select_all_occurrences(&mut self) -> usize {
        self.select_all_occurrences_until(&Stop::never())
            .unwrap_or(0)
    }

    /// [`Editor::select_all_occurrences`], giving up when `stop` says: then at most the
    /// word at the cursor has been selected, and [`Editor::occurrences`] is the search to
    /// finish elsewhere.
    pub fn select_all_occurrences_until(&mut self, stop: &Stop<'_>) -> Result<usize, Stopped> {
        let selected_word = self.select_word_at_cursor();
        let Some(search) = self.occurrences() else {
            return Ok(usize::from(selected_word));
        };
        let found = search.all(self.text(), stop)?;
        Ok(self.set_occurrences(&search, found))
    }

    /// Selects `found`, every occurrence `search` found. Returns how many there are, or
    /// 0, changing nothing, if the selections aren't the ones the search began with.
    pub fn set_occurrences(&mut self, search: &Occurrences, found: Vec<Range<usize>>) -> usize {
        if self.selections != search.selections || found.is_empty() {
            return 0;
        }
        let word = self.occurrence_word;
        let primary = self.selections.primary().start();
        let primary_index = found.iter().position(|r| r.start == primary).unwrap_or(0);
        let n = found.len();
        let found = found
            .into_iter()
            .map(|r| Selection::new(r.start, r.end))
            .collect();
        self.set_selections(Selections::new(found, primary_index));
        self.occurrence_word = word;
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

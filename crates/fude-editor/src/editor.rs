//! The editor: a buffer, its selections, and the commands that move and edit them.

use std::ops::Range;
use std::time::{Duration, Instant};

use fude_buffer::{Buffer, Edit, Format, LineEnding, Rope, Version};

use crate::layout::Layout;
use crate::selection::{Selection, Selections};
use crate::text::{
    WordStop, display_column, indent_end, next_grapheme, prev_grapheme, word_left, word_right,
};

/// Where a cursor motion goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    /// First non-blank character of the line, or the line start if already there.
    LineStart,
    LineEnd,
    Up,
    Down,
    PageUp(usize),
    PageDown(usize),
    DocStart,
    DocEnd,
}

/// Editing preferences.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub tab_width: usize,
    pub word_stop: WordStop,
    /// Typing after a pause this long starts a new undo step.
    pub undo_pause: Duration,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            tab_width: 4,
            word_stop: WordStop::PLATFORM,
            undo_pause: Duration::from_secs(1),
        }
    }
}

/// Edits that consecutive keystrokes of the same kind merge into one undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Typing,
    DeleteBack,
    DeleteForward,
}

struct LastEdit {
    kind: Kind,
    version: Version,
    at: Instant,
    ended_with_space: bool,
}

/// One selection's replacement: `text` replaces `range`, and the cursor lands `cursor`
/// bytes into `text`.
struct Replace {
    range: Range<usize>,
    text: Vec<u8>,
    cursor: usize,
    primary: bool,
}

/// A document being edited: text, format, undo history and selections.
pub struct Editor {
    buffer: Buffer<Selections>,
    selections: Selections,
    last_edit: Option<LastEdit>,
    /// What the last copy of whole lines (copy with nothing selected) put on the clipboard,
    /// so pasting it inserts whole lines.
    line_copy: Option<String>,
    pub settings: Settings,
}

impl Editor {
    pub fn new(text: Rope, format: Format) -> Editor {
        Editor {
            buffer: Buffer::new(text, format),
            selections: Selections::single(Selection::cursor(0)),
            last_edit: None,
            line_copy: None,
            settings: Settings::default(),
        }
    }

    pub fn text(&self) -> &Rope {
        self.buffer.text()
    }

    pub fn format(&self) -> &Format {
        self.buffer.format()
    }

    /// The undo history, version and saved state.
    pub fn buffer(&self) -> &Buffer<Selections> {
        &self.buffer
    }

    pub fn buffer_mut(&mut self) -> &mut Buffer<Selections> {
        &mut self.buffer
    }

    pub fn selections(&self) -> &Selections {
        &self.selections
    }

    /// Replaces the whole text and format as one undo step (reloading a file that changed
    /// on disk), keeping cursors where they were as far as the new text allows.
    pub fn replace_content(&mut self, text: Rope, format: Format) {
        let len = text.len();
        let before = self.selections.clone();
        let after = before.map(|s| {
            let clamp = |at: usize| crate::text::floor_grapheme(&text, at.min(len));
            Selection::new(clamp(s.anchor), clamp(s.head))
        });
        self.buffer.set_content(text, format, before, after.clone());
        self.selections = after;
        self.seal();
    }

    /// Replaces the selections (mouse selection, select all). Ends the current undo step.
    pub fn set_selections(&mut self, selections: Selections) {
        self.selections = selections;
        self.seal();
    }

    /// Ends the current undo step: the next edit starts a new one.
    pub fn seal(&mut self) {
        self.last_edit = None;
        self.buffer.seal();
    }

    pub fn select_all(&mut self) {
        let len = self.text().len();
        self.set_selections(Selections::single(Selection::new(0, len)));
    }

    /// Escape: drops extra cursors, or collapses the selection to its head.
    pub fn collapse(&mut self) {
        let next = if self.selections.len() > 1 {
            self.selections.only_primary()
        } else {
            Selections::single(Selection::cursor(self.selections.primary().head))
        };
        self.set_selections(next);
    }

    /// Line (1-based) and display column (1-based, tabs expanded) of the primary cursor.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let text = self.text();
        let head = self.selections.primary().head;
        let line = text.byte_to_line(head);
        let start = text.line_to_byte(line);
        (
            line + 1,
            display_column(text, start, head, self.settings.tab_width),
        )
    }

    // --- Movement ----------------------------------------------------------------------

    pub fn move_cursors(&mut self, motion: Motion, extend: bool, layout: &mut dyn Layout) {
        let text = self.text().clone();
        let settings = self.settings;
        let next = self.selections.map(|s| {
            let (head, goal_x) = target(&text, s, motion, extend, settings, layout);
            Selection {
                anchor: if extend { s.anchor } else { head },
                head,
                goal_x,
            }
        });
        self.set_selections(next);
    }

    // --- Editing -----------------------------------------------------------------------

    /// Types `text` at every selection, replacing what's selected.
    pub fn insert(&mut self, text: &str, now: Instant) {
        if text.is_empty() {
            return;
        }
        let bytes = normalize_breaks(text, self.format().line_ending);
        let reps = self
            .each(|_, s| Some((s.range(), bytes.clone(), bytes.len())))
            .collect();
        let ended_with_space = text.ends_with(char::is_whitespace);
        let starts_word = !text.starts_with(char::is_whitespace);
        self.apply(reps, Some(Kind::Typing), now, ended_with_space, starts_word);
    }

    /// Enter: a line break in the document's style, keeping the line's indentation.
    pub fn newline(&mut self, now: Instant) {
        let text = self.text().clone();
        let le = self.format().line_ending;
        let reps = self
            .each(|_, s| {
                let span = text.line_span(text.byte_to_line(s.start()));
                let indent_to = indent_end(&text, span.start, span.end).min(s.start());
                let mut insert = le.as_bytes().to_vec();
                text.copy_range(span.start..indent_to, &mut insert);
                let len = insert.len();
                Some((s.range(), insert, len))
            })
            .collect();
        self.apply(reps, None, now, false, false);
    }

    /// Backspace: deletes the selection, or the grapheme before each cursor.
    pub fn backspace(&mut self, now: Instant) {
        let text = self.text().clone();
        self.delete_with(Some(Kind::DeleteBack), now, |s| {
            (s.head > 0).then(|| prev_grapheme(&text, s.head)..s.head)
        });
    }

    /// Delete: deletes the selection, or the grapheme after each cursor.
    pub fn delete(&mut self, now: Instant) {
        let text = self.text().clone();
        self.delete_with(Some(Kind::DeleteForward), now, |s| {
            (s.head < text.len()).then(|| s.head..next_grapheme(&text, s.head))
        });
    }

    /// Ctrl+Backspace (Option+Backspace on macOS).
    pub fn delete_word_left(&mut self, now: Instant) {
        let text = self.text().clone();
        self.delete_with(None, now, |s| Some(word_left(&text, s.head)..s.head));
    }

    /// Ctrl+Delete (Option+Delete on macOS).
    pub fn delete_word_right(&mut self, now: Instant) {
        let text = self.text().clone();
        let stop = self.settings.word_stop;
        self.delete_with(None, now, |s| Some(s.head..word_right(&text, s.head, stop)));
    }

    /// Cmd+Backspace on macOS: deletes to the start of the line.
    pub fn delete_to_line_start(&mut self, now: Instant) {
        let text = self.text().clone();
        self.delete_with(None, now, |s| {
            Some(text.line_to_byte(text.byte_to_line(s.head))..s.head)
        });
    }

    /// Deletes each selection, or for a cursor the range `range` gives. `kind: None` makes
    /// it an undo step of its own.
    fn delete_with(
        &mut self,
        kind: Option<Kind>,
        now: Instant,
        mut range: impl FnMut(&Selection) -> Option<Range<usize>>,
    ) {
        let reps: Vec<Replace> = self
            .each(|_, s| {
                let r = if s.is_empty() { range(s)? } else { s.range() };
                Some((r, Vec::new(), 0))
            })
            .collect();
        if reps.iter().all(|r| r.range.is_empty()) {
            return;
        }
        self.apply(reps, kind, now, false, false);
    }

    pub fn undo(&mut self) -> bool {
        self.last_edit = None;
        match self.buffer.undo() {
            Some(selections) => {
                self.selections = selections.clone();
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        self.last_edit = None;
        match self.buffer.redo() {
            Some(selections) => {
                self.selections = selections.clone();
                true
            }
            None => false,
        }
    }

    // --- Clipboard ---------------------------------------------------------------------

    /// The text to put on the clipboard: the selections joined by line breaks, or, when
    /// nothing is selected, the cursors' whole lines (pasting those inserts whole lines).
    /// Invalid UTF-8 becomes U+FFFD.
    pub fn copy(&mut self) -> String {
        let text = self.text();
        let le = self.format().line_ending.as_bytes();
        let mut out = Vec::new();
        if self.selections.iter().all(Selection::is_empty) {
            for range in self.cursor_lines() {
                text.copy_range(range.clone(), &mut out);
                if !out.ends_with(b"\n") && !out.ends_with(b"\r") {
                    out.extend_from_slice(le);
                }
            }
            let copied = String::from_utf8_lossy(&out).into_owned();
            self.line_copy = Some(copied.clone());
            copied
        } else {
            for (i, s) in self.selections.iter().filter(|s| !s.is_empty()).enumerate() {
                if i > 0 {
                    out.extend_from_slice(le);
                }
                text.copy_range(s.range(), &mut out);
            }
            self.line_copy = None;
            String::from_utf8_lossy(&out).into_owned()
        }
    }

    /// Copies, then deletes what was copied (whole lines when nothing is selected).
    pub fn cut(&mut self, now: Instant) -> String {
        let copied = self.copy();
        if self.selections.iter().all(Selection::is_empty) {
            let text = self.text().clone();
            let reps = self
                .cursor_lines()
                .into_iter()
                .map(|line| {
                    // Cutting the last line takes the line break before it instead.
                    let range = if line.end == text.len()
                        && line.start > 0
                        && !ends_with_break(&text, line.end)
                    {
                        let prev = text.line_span(text.byte_to_line(line.start) - 1);
                        prev.end..line.end
                    } else {
                        line
                    };
                    Replace {
                        range,
                        text: Vec::new(),
                        cursor: 0,
                        primary: false,
                    }
                })
                .collect();
            self.apply(reps, None, now, false, false);
        } else {
            self.delete_with(None, now, |_| None);
        }
        copied
    }

    /// Pastes `text` at every selection, with line breaks converted to the document's
    /// style. Lines copied with nothing selected are inserted above the cursor's line.
    pub fn paste(&mut self, text: &str, now: Instant) {
        if text.is_empty() {
            return;
        }
        let bytes = normalize_breaks(text, self.format().line_ending);
        let whole_lines = self.line_copy.as_deref() == Some(text)
            && self.selections.iter().all(Selection::is_empty);
        let doc = self.text().clone();
        let reps = self
            .each(|_, s| {
                if whole_lines {
                    let start = doc.line_to_byte(doc.byte_to_line(s.head));
                    Some((start..start, bytes.clone(), bytes.len() + (s.head - start)))
                } else {
                    Some((s.range(), bytes.clone(), bytes.len()))
                }
            })
            .collect();
        self.apply(reps, None, now, false, false);
    }

    /// Byte ranges (with line breaks) of the lines holding cursors, each once.
    fn cursor_lines(&self) -> Vec<Range<usize>> {
        let text = self.text();
        let mut lines: Vec<usize> = self
            .selections
            .iter()
            .map(|s| text.byte_to_line(s.head))
            .collect();
        lines.dedup();
        lines
            .into_iter()
            .map(|l| {
                let span = text.line_span(l);
                span.start..span.next
            })
            .collect()
    }

    // --- Applying ----------------------------------------------------------------------

    /// One replacement per selection, from `f(index, selection)`; `None` leaves that
    /// selection's cursor where it is.
    fn each(
        &self,
        mut f: impl FnMut(usize, &Selection) -> Option<(Range<usize>, Vec<u8>, usize)>,
    ) -> impl Iterator<Item = Replace> {
        let primary = self.selections.primary_index();
        let reps: Vec<Replace> = self
            .selections
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (range, text, cursor) =
                    f(i, s).unwrap_or_else(|| (s.head..s.head, Vec::new(), 0));
                Replace {
                    range,
                    text,
                    cursor,
                    primary: i == primary,
                }
            })
            .collect();
        reps.into_iter()
    }

    fn apply(
        &mut self,
        mut reps: Vec<Replace>,
        kind: Option<Kind>,
        now: Instant,
        ended_with_space: bool,
        starts_word: bool,
    ) {
        reps.sort_by_key(|r| (r.range.start, r.range.end));
        // Expanded deletions of neighbouring cursors can overlap: merge them.
        let mut merged: Vec<Replace> = Vec::with_capacity(reps.len());
        for r in reps {
            match merged.last_mut() {
                Some(last) if r.range.start < last.range.end => {
                    last.range.end = last.range.end.max(r.range.end);
                    last.primary |= r.primary;
                }
                _ => merged.push(r),
            }
        }
        let merge = kind.is_some_and(|kind| {
            self.last_edit.as_ref().is_some_and(|last| {
                last.kind == kind
                    && last.version == self.buffer.version()
                    && now.saturating_duration_since(last.at) < self.settings.undo_pause
                    && !(kind == Kind::Typing && last.ended_with_space && starts_word)
            })
        });

        let mut delta = 0isize;
        let mut cursors = Vec::with_capacity(merged.len());
        let mut primary = 0;
        for r in &merged {
            let start = r.range.start.checked_add_signed(delta).expect("in bounds");
            if r.primary {
                primary = cursors.len();
            }
            cursors.push(Selection::cursor(start + r.cursor));
            delta += r.text.len() as isize - r.range.len() as isize;
        }
        let edits: Vec<Edit<'_>> = merged
            .iter()
            .map(|r| Edit::replace(r.range.clone(), &r.text))
            .collect();
        let after = Selections::new(cursors, primary);
        self.buffer
            .edit(&edits, self.selections.clone(), after.clone(), merge);
        self.selections = after;
        match kind {
            Some(kind) => {
                self.last_edit = Some(LastEdit {
                    kind,
                    version: self.buffer.version(),
                    at: now,
                    ended_with_space,
                });
            }
            None => self.seal(),
        }
    }
}

/// Where `motion` takes a selection's head, and the goal x to remember.
fn target(
    text: &Rope,
    s: &Selection,
    motion: Motion,
    extend: bool,
    settings: Settings,
    layout: &mut dyn Layout,
) -> (usize, Option<f32>) {
    let head = s.head;
    let line = text.byte_to_line(head);
    let collapse = !extend && !s.is_empty();
    let vertical = |layout: &mut dyn Layout, lines: isize| {
        let goal = s.goal_x.unwrap_or_else(|| layout.x_of(text, line, head));
        let to = line as isize + lines;
        if to < 0 {
            (0, None)
        } else if to as usize >= text.line_count() {
            (text.len(), None)
        } else {
            (layout.offset_at(text, to as usize, goal), Some(goal))
        }
    };
    match motion {
        Motion::Left if collapse => (s.start(), None),
        Motion::Right if collapse => (s.end(), None),
        Motion::Left => (prev_grapheme(text, head), None),
        Motion::Right => (next_grapheme(text, head), None),
        Motion::WordLeft => (word_left(text, head), None),
        Motion::WordRight => (word_right(text, head, settings.word_stop), None),
        Motion::LineStart => {
            let span = text.line_span(line);
            let indent = indent_end(text, span.start, span.end);
            (if head == indent { span.start } else { indent }, None)
        }
        Motion::LineEnd => (text.line_span(line).end, None),
        Motion::Up => vertical(layout, -1),
        Motion::Down => vertical(layout, 1),
        Motion::PageUp(n) => vertical(layout, -(n.max(1) as isize)),
        Motion::PageDown(n) => vertical(layout, n.max(1) as isize),
        Motion::DocStart => (0, None),
        Motion::DocEnd => (text.len(), None),
    }
}

fn ends_with_break(text: &Rope, at: usize) -> bool {
    at > 0 && matches!(text.byte(at - 1), b'\n' | b'\r')
}

/// `text` with every line break in the given style.
fn normalize_breaks(text: &str, le: LineEnding) -> Vec<u8> {
    if !text.contains(['\r', '\n']) {
        return text.as_bytes().to_vec();
    }
    let mut out = Vec::with_capacity(text.len() + 16);
    let mut rest = text.as_bytes();
    while let Some(i) = rest.iter().position(|&b| b == b'\r' || b == b'\n') {
        out.extend_from_slice(&rest[..i]);
        out.extend_from_slice(le.as_bytes());
        let crlf = rest[i] == b'\r' && rest.get(i + 1) == Some(&b'\n');
        rest = &rest[i + 1 + usize::from(crlf)..];
    }
    out.extend_from_slice(rest);
    out
}

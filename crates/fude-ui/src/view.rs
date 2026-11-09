//! The editor view: draws the visible part of a document and turns keys, mouse and IME
//! input into editor commands.
//!
//! Only visible text is ever laid out, so a frame costs the same at any file size: the
//! view works in pieces of lines (see `pieces`), and its scroll position is a piece and a
//! row in it, never a count of everything above. The caret blinks for a while after
//! input, then stays solid, so an idle window wakes no timers at all.

use std::ops::Range;
use std::time::{Duration, Instant};

use fude_buffer::Rope;
use fude_editor::{
    BRACKET_LIMIT, Editor, Layout, Motion, Searcher, Selection, Selections, nearest, text as nav,
};
use fude_platform::{CursorShape, Ime, Key, KeyInput, Modifiers, ScrollUnit};
use fude_render::Rect;
use fude_text::{Chip, FontId, LineLayout, TextStyle, TextSystem};

use fude_syntax::{Span, Style};

use crate::display::DisplayLine;
use crate::pieces::{PIECE, Piece, first_piece, next_piece, piece_at, piece_of, prev_piece};

/// Syntax highlighting for a range of lines, from a document version.
#[derive(Clone, Debug)]
pub struct Highlights {
    /// The document version they were made for (`Version::get`).
    pub version: u64,
    pub first_line: usize,
    pub lines: Vec<Vec<Span>>,
}

impl Highlights {
    pub fn covers(&self, version: u64, lines: &Range<usize>) -> bool {
        self.version == version
            && self.first_line <= lines.start
            && lines.end <= self.first_line + self.lines.len()
    }

    fn line(&self, line: usize) -> Option<&[Span]> {
        self.lines
            .get(line.checked_sub(self.first_line)?)
            .map(Vec::as_slice)
    }
}
use crate::painter::Painter;

pub const DEFAULT_FONT_SIZE: f32 = 14.0;
const BLINK: Duration = Duration::from_millis(530);
/// The caret stops blinking (and the view stops waking up) this long after input.
const BLINK_FOR: Duration = Duration::from_secs(10);
const AUTOSCROLL_TICK: Duration = Duration::from_millis(16);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Unit {
    Char,
    Word,
    Line,
    /// Alt+drag: a box, from a line and x.
    Column,
}

/// A mouse selection in progress: what was clicked, and by what unit it grows.
struct Drag {
    unit: Unit,
    origin: Range<usize>,
}

/// The bracket pair by the cursor (bracket, partner), for (text version, cursor,
/// syntax colors' version).
type BracketCache = ((u64, usize, u64), Option<(usize, usize)>);

/// A column (box) selection being made: its corners as (line, x within the row).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Column {
    anchor: (usize, f32),
    head: (usize, f32),
}

/// Where things are, from the last layout (physical pixels).
#[derive(Clone, Copy, Debug)]
struct Geometry {
    area: Rect,
    /// Text area: right of the gutter, left of the scrollbar.
    text: Rect,
    scrollbar: Rect,
    line_h: i32,
    pad_top: i32,
    /// Gap between the gutter and the first character.
    text_pad: i32,
    /// Font size, physical pixels.
    px: f32,
    /// Lines that fit in the text area.
    visible: f32,
    min_thumb: i32,
}

impl Default for Geometry {
    fn default() -> Geometry {
        Geometry {
            area: Rect::default(),
            text: Rect::default(),
            scrollbar: Rect::default(),
            line_h: 21,
            pad_top: 6,
            text_pad: 8,
            px: DEFAULT_FONT_SIZE,
            visible: 30.0,
            min_thumb: 24,
        }
    }
}

/// The top of the view: row `row` of the piece that starts at `at`, scrolled a further
/// `sub` of a row (smooth scrolling). A byte offset, not a count of what's above, so it
/// costs the same anywhere in any file, and stays put while a file finishes loading.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Top {
    at: usize,
    row: usize,
    sub: f32,
}

/// A view's scroll position, to go back to later (after a preview).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollPosition {
    top: Top,
    x: f32,
}

/// Reused buffers for laying out one line.
#[derive(Default)]
struct Scratch {
    bytes: Vec<u8>,
    display: DisplayLine,
    layout: LineLayout,
}

/// One visual row of a line: display offsets `start..end`, starting at `x` in the line's
/// layout. Without wrapping, a line is one row.
#[derive(Clone, Copy, Debug)]
struct Row {
    start: usize,
    end: usize,
    x: f32,
    /// Where the row's text starts: continuation rows keep the line's indentation.
    indent: f32,
}

impl Row {
    /// Moves an x in the line's layout to where it's drawn in this row.
    fn place(&self, x: f32) -> f32 {
        x - self.x + self.indent
    }
}

/// Lays out single pieces of lines for drawing, hit testing and vertical movement.
struct LineCtx<'a> {
    text: &'a mut TextSystem,
    style: TextStyle,
    s: &'a mut Scratch,
    /// Wrap width, if wrapping.
    wrap: Option<f32>,
    /// Layout may stop past this x: for drawing a row of which only the start shows.
    max_x: f32,
}

impl LineCtx<'_> {
    fn prepare(&mut self, doc: &Rope, piece: &Piece, preedit: Option<(usize, &str)>) {
        self.s.bytes.clear();
        doc.copy_range(piece.start..piece.end, &mut self.s.bytes);
        let preedit = preedit.map(|(at, text)| (at - piece.start, text));
        self.s.display.build(&self.s.bytes, preedit);
        self.text.layout_line_until(
            &self.s.display.text,
            &self.style,
            &mut self.s.layout,
            self.max_x,
        );
    }

    /// x of a display offset in the prepared line.
    fn x_at_display(&self, display: usize) -> f32 {
        self.s
            .layout
            .glyphs
            .iter()
            .find(|g| g.cluster as usize >= display)
            .map_or(self.s.layout.width, |g| g.x)
    }

    /// Cluster starts of the prepared line: display offset and x.
    fn clusters(&self) -> Vec<(usize, f32)> {
        let mut out: Vec<(usize, f32)> = Vec::with_capacity(self.s.layout.glyphs.len());
        for g in &self.s.layout.glyphs {
            if out.last().is_none_or(|&(c, _)| c != g.cluster as usize) {
                out.push((g.cluster as usize, g.x));
            }
        }
        out
    }

    /// The prepared line's rows: breaks after whitespace, or inside a word that's wider
    /// than the wrap width.
    fn rows(&self) -> Vec<Row> {
        let total = self.s.display.text.len();
        let one = vec![Row {
            start: 0,
            end: total,
            x: 0.0,
            indent: 0.0,
        }];
        let Some(width) = self.wrap else { return one };
        let clusters = self.clusters();
        if clusters.is_empty() || self.s.layout.width <= width {
            return one;
        }
        let text = &self.s.display.text;
        // Continuation rows start at the line's indentation (up to half the width).
        let indent = clusters
            .iter()
            .find(|&&(c, _)| !text[c..].starts_with([' ', '\t']))
            .map_or(0.0, |&(_, x)| x)
            .min(width / 2.0);
        let mut rows = Vec::new();
        let (mut row_start, mut row_x, mut row_indent) = (0usize, 0.0f32, 0.0f32);
        let mut last_break: Option<(usize, f32)> = None;
        for (k, &(c, _)) in clusters.iter().enumerate() {
            let next = clusters
                .get(k + 1)
                .map_or((total, self.s.layout.width), |&n| n);
            while next.1 - row_x + row_indent > width && c > row_start {
                let (b, bx) = last_break
                    .filter(|&(b, _)| b > row_start && b <= c)
                    .unwrap_or((c, clusters[k].1));
                rows.push(Row {
                    start: row_start,
                    end: b,
                    x: row_x,
                    indent: row_indent,
                });
                row_start = b;
                row_x = bx;
                row_indent = indent;
                last_break = None;
            }
            if text[c..].starts_with([' ', '\t']) {
                last_break = Some(next);
            }
        }
        rows.push(Row {
            start: row_start,
            end: total,
            x: row_x,
            indent: row_indent,
        });
        rows
    }

    /// The row holding a display offset; at a boundary, the later row.
    fn row_of(rows: &[Row], display: usize) -> usize {
        rows.iter().rposition(|r| r.start <= display).unwrap_or(0)
    }

    /// Cluster boundaries within a row, as (document offset, x within the row). The end
    /// of the line is a stop on its last row only; elsewhere a row's end is the next row's
    /// start.
    fn row_stops(&self, rows: &[Row], index: usize, piece: &Piece) -> Vec<(usize, f32)> {
        let row = rows[index];
        let mut stops: Vec<(usize, f32)> = self
            .clusters()
            .into_iter()
            .filter(|&(c, _)| c >= row.start && c < row.end)
            .map(|(c, x)| (piece.start + self.s.display.to_doc(c), row.place(x)))
            .collect();
        if index + 1 == rows.len() && piece.is_last() {
            stops.push((
                piece.start + self.s.bytes.len(),
                row.place(self.s.layout.width),
            ));
        }
        if stops.is_empty() {
            stops.push((piece.start + self.s.display.to_doc(row.start), row.indent));
        }
        stops
    }

    /// The row of the prepared piece that a cursor at `at` is drawn on.
    fn row_at(&self, rows: &[Row], piece: &Piece, at: usize) -> usize {
        LineCtx::row_of(
            rows,
            self.s.display.to_display(at.min(piece.end) - piece.start),
        )
    }
}

impl Layout for LineCtx<'_> {
    fn x_of(&mut self, doc: &Rope, line: usize, at: usize) -> f32 {
        let piece = piece_at(doc, line, doc.line_span(line), at);
        self.prepare(doc, &piece, None);
        let d = self.s.display.to_display(at.min(piece.end) - piece.start);
        let rows = self.rows();
        rows[LineCtx::row_of(&rows, d)].place(self.x_at_display(d))
    }

    fn offset_at(&mut self, doc: &Rope, line: usize, x: f32) -> usize {
        let piece = first_piece(doc, line);
        self.prepare(doc, &piece, None);
        let rows = self.rows();
        nearest(&self.row_stops(&rows, 0, &piece), x)
    }

    fn vertical(
        &mut self,
        doc: &Rope,
        at: usize,
        goal_x: Option<f32>,
        rows: isize,
    ) -> (usize, Option<f32>) {
        let mut piece = piece_of(doc, at);
        self.prepare(doc, &piece, None);
        let mut rs = self.rows();
        let d = self.s.display.to_display(at.min(piece.end) - piece.start);
        let mut r = LineCtx::row_of(&rs, d);
        let goal = goal_x.unwrap_or_else(|| rs[r].place(self.x_at_display(d)));
        // Without wrapping, a piece is one row: the pieces passed need no layout, only
        // the one arrived at.
        let one_row = self.wrap.is_none();
        let mut prepared = true;
        let mut left = rows;
        while left > 0 {
            if r + 1 < rs.len() {
                r += 1;
            } else if let Some(next) = next_piece(doc, &piece) {
                piece = next;
                prepared = !one_row;
                if !one_row {
                    self.prepare(doc, &piece, None);
                    rs = self.rows();
                }
                r = 0;
            } else {
                return (doc.len(), None);
            }
            left -= 1;
        }
        while left < 0 {
            if r > 0 {
                r -= 1;
            } else if let Some(prev) = prev_piece(doc, &piece) {
                piece = prev;
                prepared = !one_row;
                if !one_row {
                    self.prepare(doc, &piece, None);
                    rs = self.rows();
                }
                r = rs.len() - 1;
            } else {
                return (0, None);
            }
            left += 1;
        }
        if !prepared {
            self.prepare(doc, &piece, None);
            rs = self.rows();
        }
        (nearest(&self.row_stops(&rs, r, &piece), goal), Some(goal))
    }
}

pub struct EditorView {
    pub editor: Editor,
    /// Logical font size; zoom changes it.
    font_size: f32,
    /// Matches of this search are highlighted in the visible lines.
    pub search: Option<Searcher>,
    /// Wrap long lines at the view's edge.
    pub wrap: bool,
    /// Syntax colors (possibly from a slightly older version while new ones are made).
    pub highlights: Option<Highlights>,
    /// The lines drawn in the last frame.
    visible_lines: Range<usize>,
    /// Bytes from the start of the first piece drawn in the last frame to the end of the
    /// last, for sizing the scrollbar.
    visible_bytes: usize,
    /// The whole text fit on screen in the last frame (no scrollbar).
    fits: bool,
    top: Top,
    /// How many rows the top piece has.
    top_rows: usize,
    /// Scroll to this line at the next draw.
    goto_line: Option<f64>,
    /// The column selection being made with Alt+drag or Shift+Alt+arrows.
    column: Option<Column>,
    /// The bracket pair at the cursor, for (version, cursor, colors' version).
    bracket: Option<BracketCache>,
    /// Horizontal scroll, physical pixels.
    scroll_x: f32,
    /// Scroll the primary cursor into view at the next draw.
    reveal: bool,
    /// ...and put it in the middle of the view.
    center: bool,
    geo: Geometry,
    focused: bool,
    blink_start: Instant,
    drag: Option<Drag>,
    mouse: (f32, f32),
    /// Dragging the scrollbar thumb: where in the thumb it was grabbed.
    thumb_grab: Option<f32>,
    hover_thumb: bool,
    /// IME composition: text and the IME's cursor range in it.
    preedit: Option<(String, Option<(usize, usize)>)>,
    /// Widest visible line in the last frame, which bounds horizontal scrolling.
    max_width: f32,
    /// The primary caret in the last frame, for placing the IME window.
    caret: Option<Rect>,
    scratch: Scratch,
}

impl EditorView {
    pub fn new(editor: Editor) -> EditorView {
        EditorView {
            editor,
            font_size: DEFAULT_FONT_SIZE,
            search: None,
            wrap: false,
            highlights: None,
            visible_lines: 0..0,
            visible_bytes: 0,
            fits: true,
            top: Top::default(),
            top_rows: 1,
            goto_line: None,
            column: None,
            bracket: None,
            scroll_x: 0.0,
            reveal: false,
            center: false,
            geo: Geometry::default(),
            focused: true,
            blink_start: Instant::now(),
            drag: None,
            mouse: (0.0, 0.0),
            thumb_grab: None,
            hover_thumb: false,
            preedit: None,
            max_width: 0.0,
            caret: None,
            scratch: Scratch::default(),
        }
    }

    fn style(&self) -> TextStyle {
        TextStyle {
            tab_width: self.editor.settings.tab_width as u32,
            ..TextStyle::new(FontId::MONO, self.geo.px)
        }
    }

    fn wrap_width(&self) -> Option<f32> {
        self.wrap
            .then(|| ((self.geo.text.w - self.geo.text_pad * 2) as f32).max(self.geo.px * 4.0))
    }

    fn line_ctx<'a>(&'a mut self, text: &'a mut TextSystem) -> LineCtx<'a> {
        LineCtx {
            text,
            style: TextStyle {
                tab_width: self.editor.settings.tab_width as u32,
                ..TextStyle::new(FontId::MONO, self.geo.px)
            },
            wrap: self.wrap_width(),
            s: &mut self.scratch,
            max_x: f32::INFINITY,
        }
    }

    /// How many rows a piece takes (1 without wrapping).
    fn piece_rows(&mut self, text: &mut TextSystem, doc: &Rope, piece: &Piece) -> usize {
        if !self.wrap {
            return 1;
        }
        let mut ctx = self.line_ctx(text);
        ctx.prepare(doc, piece, None);
        ctx.rows().len()
    }

    /// The piece and row at the top of the view, after edits or a change of wrapping
    /// have moved things.
    fn top_piece(&mut self, text: &mut TextSystem, doc: &Rope) -> (Piece, usize, f32) {
        let piece = piece_of(doc, self.top.at);
        let rows = self.piece_rows(text, doc, &piece);
        self.top_rows = rows;
        let row = self.top.row.min(rows - 1);
        let sub = if row == self.top.row {
            self.top.sub
        } else {
            0.0
        };
        self.top = Top {
            at: piece.start,
            row,
            sub,
        };
        (piece, row, sub)
    }

    /// Scrolls by `delta` visual rows, laying out only the pieces it passes. The last row
    /// of the text can come up to the top, no further.
    fn scroll_rows(&mut self, text: &mut TextSystem, delta: f32) {
        let doc = self.editor.text().clone();
        let (mut piece, row, sub) = self.top_piece(text, &doc);
        let mut n = self.piece_rows(text, &doc, &piece) as f32;
        let mut pos = row as f32 + sub + delta;
        while pos >= n {
            let Some(next) = next_piece(&doc, &piece) else {
                pos = n - 1.0;
                break;
            };
            pos -= n;
            piece = next;
            n = self.piece_rows(text, &doc, &piece) as f32;
        }
        while pos < 0.0 {
            let Some(prev) = prev_piece(&doc, &piece) else {
                pos = 0.0;
                break;
            };
            piece = prev;
            n = self.piece_rows(text, &doc, &piece) as f32;
            pos += n;
        }
        let pos = pos.clamp(0.0, n - 1.0);
        self.top = Top {
            at: piece.start,
            row: pos.floor() as usize,
            sub: pos.fract(),
        };
        self.top_rows = n as usize;
    }

    /// Puts the top of the view at a fraction of the way through the text: lines, and
    /// within a long line, bytes. What the scrollbar shows and its thumb sets.
    fn scroll_to_fraction(&mut self, text: &mut TextSystem, frac: f64) {
        let doc = self.editor.text().clone();
        let lines = doc.line_count();
        let pos = frac.clamp(0.0, 1.0) * (lines.max(2) - 1) as f64;
        let line = (pos.floor() as usize).min(lines - 1);
        let within = (pos - line as f64).clamp(0.0, 1.0);
        let span = doc.line_span(line);
        let at = span.start + ((span.end - span.start) as f64 * within) as usize;
        let piece = piece_at(&doc, line, span, at);
        self.top = Top {
            at: piece.start,
            row: 0,
            sub: 0.0,
        };
        if piece.is_first() && piece.is_last() {
            // A line of one piece: scroll smoothly through its rows.
            let rows = self.piece_rows(text, &doc, &piece) as f64;
            self.scroll_rows(text, (within * rows) as f32);
        }
    }

    /// How far through the text the top of the view is, from 0 to 1 (see
    /// [`EditorView::scroll_to_fraction`]).
    fn scroll_fraction(&self) -> f64 {
        let doc = self.editor.text();
        let at = self.top.at.min(doc.len());
        let line = doc.byte_to_line(at);
        let span = doc.line_span(line);
        let within = if span.end - span.start <= PIECE {
            (self.top.row as f64 + f64::from(self.top.sub)) / self.top_rows.max(1) as f64
        } else {
            (at - span.start) as f64 / (span.end - span.start) as f64
        };
        ((line as f64 + within) / (doc.line_count().max(2) - 1) as f64).min(1.0)
    }

    /// Restarts the caret blink and scrolls the cursor into view.
    fn touched(&mut self, now: Instant) {
        self.blink_start = now;
        self.reveal = true;
    }

    // --- Status --------------------------------------------------------------------------

    /// "Ln 12, Col 5" plus what's selected, for the status bar.
    pub fn position_label(&self) -> String {
        let (line, col) = self.editor.cursor_line_col();
        let sels = self.editor.selections();
        let mut label = format!("Ln {line}, Col {col}");
        if sels.len() > 1 {
            label.push_str(&format!(" ({} cursors)", sels.len()));
        } else {
            let s = sels.primary();
            if !s.is_empty() {
                let n = s.range().len();
                // Counting characters is linear; above a megabyte, report bytes.
                if n <= 1 << 20 {
                    let mut bytes = Vec::with_capacity(n);
                    self.editor.text().copy_range(s.range(), &mut bytes);
                    let chars = String::from_utf8_lossy(&bytes).chars().count();
                    label.push_str(&format!(" ({chars} selected)"));
                } else {
                    label.push_str(&format!(" ({n} bytes selected)"));
                }
            }
        }
        label
    }

    /// The lines drawn in the last frame.
    pub fn visible_lines(&self) -> Range<usize> {
        self.visible_lines.clone()
    }

    /// The primary caret's rectangle in the last frame.
    pub fn caret_rect(&self) -> Option<Rect> {
        self.caret
    }

    // --- Timers ----------------------------------------------------------------------------

    fn blink_on(&self, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.blink_start);
        elapsed >= BLINK_FOR || (elapsed.as_millis() / BLINK.as_millis()).is_multiple_of(2)
    }

    /// When the view next needs a timer: the next blink, or the next auto-scroll step.
    pub fn next_timer(&self, now: Instant) -> Option<Instant> {
        if self.drag.is_some() && self.outside_vertically(self.mouse.1) != 0.0 {
            return Some(now + AUTOSCROLL_TICK);
        }
        if !self.focused {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.blink_start);
        if elapsed >= BLINK_FOR {
            return None;
        }
        let ticks = (elapsed.as_millis() / BLINK.as_millis()) as u32 + 1;
        Some((self.blink_start + BLINK * ticks).min(self.blink_start + BLINK_FOR))
    }

    /// Handles a timer: auto-scrolls while dragging outside the text. Returns whether
    /// anything changed (the caret blink always redraws).
    pub fn tick(&mut self, p: &mut Painter, _now: Instant) -> bool {
        let dy = self.outside_vertically(self.mouse.1);
        if self.drag.is_some() && dy != 0.0 {
            // Faster the further outside the pointer is.
            let rows = dy / self.geo.line_h as f32 * 0.5;
            self.scroll_rows(&mut p.text, rows);
            let (x, y) = self.mouse;
            self.drag_to(p, x, y);
        }
        true
    }

    /// How far above (negative) or below (positive) the text area `y` is.
    fn outside_vertically(&self, y: f32) -> f32 {
        let text = self.geo.text;
        if y < text.y as f32 {
            y - text.y as f32
        } else if y > text.bottom() as f32 {
            y - text.bottom() as f32
        } else {
            0.0
        }
    }

    // --- Input -----------------------------------------------------------------------------

    pub fn set_focused(&mut self, focused: bool, now: Instant) {
        self.focused = focused;
        self.blink_start = now;
        if !focused {
            self.drag = None;
            self.thumb_grab = None;
        }
    }

    /// Handles navigation, editing and typing keys. Returns whether the key was used.
    pub fn key(&mut self, p: &mut Painter, key: &KeyInput, now: Instant) -> bool {
        // Releases, and keys that do nothing here (Shift and Alt pressed on their own,
        // on the way to Shift+Alt+Down), change nothing.
        if !key.pressed || (key.key == Key::Other && key.text.is_none()) {
            return false;
        }
        let m = key.modifiers;
        // Shift+Alt+arrows make a column selection (as in Notepad++).
        if m.shift && m.alt && !m.primary() {
            let rows = match key.key {
                Key::Up => Some((-1, 0.0)),
                Key::Down => Some((1, 0.0)),
                Key::Left => Some((0, -1.0)),
                Key::Right => Some((0, 1.0)),
                _ => None,
            };
            if let Some((rows, columns)) = rows {
                self.extend_column(p, rows, columns);
                self.touched(now);
                return true;
            }
        }
        self.column = None;
        let mac = cfg!(target_os = "macos");
        let word = if mac { m.alt } else { m.ctrl };
        let page = (self.geo.visible - 1.0).floor().max(1.0) as usize;
        let motion = match key.key {
            Key::Left if mac && m.logo => Some(Motion::LineStart),
            Key::Right if mac && m.logo => Some(Motion::LineEnd),
            Key::Up if mac && m.logo => Some(Motion::DocStart),
            Key::Down if mac && m.logo => Some(Motion::DocEnd),
            Key::Left if word => Some(Motion::WordLeft),
            Key::Right if word => Some(Motion::WordRight),
            Key::Left => Some(Motion::Left),
            Key::Right => Some(Motion::Right),
            Key::Up => Some(Motion::Up),
            Key::Down => Some(Motion::Down),
            Key::Home if m.ctrl => Some(Motion::DocStart),
            Key::End if m.ctrl => Some(Motion::DocEnd),
            Key::Home => Some(Motion::LineStart),
            Key::End => Some(Motion::LineEnd),
            Key::PageUp => Some(Motion::PageUp(page)),
            Key::PageDown => Some(Motion::PageDown(page)),
            _ => None,
        };
        if let Some(motion) = motion {
            // Paging moves the view by the same amount, so the cursor keeps its place.
            match motion {
                Motion::PageUp(n) => self.scroll_rows(&mut p.text, -(n as f32)),
                Motion::PageDown(n) => self.scroll_rows(&mut p.text, n as f32),
                _ => {}
            }
            let style = self.style();
            let wrap = self.wrap_width();
            let mut ctx = LineCtx {
                text: &mut p.text,
                style,
                s: &mut self.scratch,
                wrap,
                max_x: f32::INFINITY,
            };
            self.editor.move_cursors(motion, m.shift, &mut ctx);
            self.touched(now);
            return true;
        }
        match key.key {
            Key::Backspace if mac && m.logo => self.editor.delete_to_line_start(now),
            Key::Backspace if word => self.editor.delete_word_left(now),
            Key::Backspace => self.editor.backspace(now),
            Key::Delete if m.shift => return false,
            Key::Delete if word => self.editor.delete_word_right(now),
            Key::Delete => self.editor.delete(now),
            Key::Enter if !m.primary() => self.editor.newline(now),
            // Shift+Tab outdents; Tab indents when lines are selected.
            Key::Tab if !m.primary() && !m.alt && m.shift => self.editor.outdent_lines(),
            Key::Tab if !m.primary() && !m.alt && self.editor.spans_lines() => {
                self.editor.indent_lines();
            }
            Key::Tab if !m.primary() && !m.alt => {
                if self.editor.settings.insert_spaces {
                    // Spaces up to the next tab stop from the primary cursor.
                    let tab = self.editor.settings.tab_width.max(1);
                    let col = self.editor.cursor_line_col().1 - 1;
                    let spaces = " ".repeat(tab - col % tab);
                    self.editor.insert(&spaces, now);
                } else {
                    self.editor.insert("\t", now);
                }
            }
            Key::Escape => self.editor.collapse(),
            _ => {
                let Some(text) = &key.text else {
                    return false;
                };
                // Ctrl (Cmd) combinations are shortcuts, not text, except AltGr, which
                // Windows reports as Ctrl+Alt.
                let shortcut = if mac { m.logo } else { m.ctrl && !m.alt };
                if shortcut || text.is_empty() || text.chars().any(char::is_control) {
                    return false;
                }
                self.editor.insert(text, now);
            }
        }
        self.touched(now);
        true
    }

    /// Handles input method composition. Returns whether anything changed: input methods
    /// send empty updates freely (KWin answers every caret move with one), and those
    /// mustn't redraw or bring the caret back into view while the user scrolls.
    pub fn ime(&mut self, ime: Ime, now: Instant) -> bool {
        match ime {
            Ime::Preedit(text, cursor) if !text.is_empty() => {
                let preedit = Some((text, cursor));
                if self.preedit == preedit {
                    return false;
                }
                self.preedit = preedit;
            }
            Ime::Commit(text) if !text.is_empty() => {
                self.preedit = None;
                self.editor.insert(&text, now);
            }
            _ => return self.preedit.take().is_some(),
        }
        self.touched(now);
        true
    }

    /// Scrolls by wheel or trackpad input. Shift turns vertical wheel motion horizontal.
    pub fn scroll(
        &mut self,
        p: &mut Painter,
        dx: f32,
        dy: f32,
        unit: ScrollUnit,
        modifiers: Modifiers,
    ) {
        let (dx, dy) = if modifiers.shift && dx == 0.0 {
            (dy, 0.0)
        } else {
            (dx, dy)
        };
        let column = self.scratch_space_advance();
        let (rows, px) = match unit {
            ScrollUnit::Lines => (dy * 3.0, dx * 3.0 * column * 2.0),
            ScrollUnit::Pixels => (dy / self.geo.line_h as f32, dx),
        };
        self.scroll_rows(&mut p.text, rows);
        if !self.wrap {
            self.scroll_px(px);
        }
    }

    fn scratch_space_advance(&self) -> f32 {
        // Monospace column width, near enough without a text system at hand.
        self.geo.px * 0.6
    }

    fn scroll_px(&mut self, px: f32) {
        let limit = (self.max_width - (self.geo.text.w - self.geo.text_pad) as f32 / 2.0).max(0.0);
        self.scroll_x = (self.scroll_x + px).clamp(0.0, limit.max(self.scroll_x));
    }

    /// Sets the font size (logical pixels), keeping the cursor in view.
    pub fn set_font_size(&mut self, size: f32) {
        let size = size.clamp(6.0, 72.0);
        if size != self.font_size {
            self.font_size = size;
            self.reveal = true;
        }
    }

    /// Call after changing the editor from outside (undo, paste, select all).
    pub fn changed(&mut self, now: Instant) {
        self.touched(now);
    }

    /// Like [`EditorView::changed`], and centers the cursor's line (go to line).
    pub fn changed_centered(&mut self, now: Instant) {
        self.touched(now);
        self.center = true;
    }

    /// The scroll position, to go back to after a preview.
    pub fn scroll_position(&self) -> ScrollPosition {
        ScrollPosition {
            top: self.top,
            x: self.scroll_x,
        }
    }

    pub fn set_scroll_position(&mut self, pos: ScrollPosition) {
        self.top = pos.top;
        self.scroll_x = pos.x.max(0.0);
        self.reveal = false;
        self.center = false;
        self.goto_line = None;
    }

    /// The line at the top of the view, and how far into it: what a session remembers.
    pub fn top_line(&self) -> f64 {
        self.scroll_fraction() * (self.editor.text().line_count().max(2) - 1) as f64
    }

    /// Scrolls (at the next draw, once the view's size is known) so the top is where
    /// [`EditorView::top_line`] said.
    pub fn scroll_to_line(&mut self, line: f64) {
        self.goto_line = Some(line.max(0.0));
        self.reveal = false;
        self.center = false;
    }

    /// The pointer shape for a position.
    pub fn cursor_shape(&self, x: f32, y: f32) -> CursorShape {
        let (xi, yi) = (x as i32, y as i32);
        if self.geo.text.contains(xi, yi) || self.drag.is_some() {
            CursorShape::Text
        } else {
            CursorShape::Default
        }
    }

    /// Targets a context menu without starting a selection drag. A click inside any
    /// selection keeps the entire set; elsewhere it places one caret at the click.
    pub fn context_menu_at(&mut self, p: &mut Painter, x: f32, y: f32, now: Instant) -> bool {
        self.mouse_up();
        let (xi, yi) = (x as i32, y as i32);
        if !self.geo.area.contains(xi, yi) || self.geo.scrollbar.contains(xi, yi) {
            return false;
        }
        self.mouse = (x, y);
        let at = self.hit(p, x, y);
        let keep = self
            .editor
            .selections()
            .iter()
            .any(|s| s.range().contains(&at) || (s.is_empty() && s.head == at));
        if !keep {
            self.column = None;
            self.editor.select(at..at);
        }
        self.blink_start = now;
        // In particular, preserving an off-screen primary selection must not scroll
        // away from the secondary selection that was clicked.
        self.reveal = false;
        self.center = false;
        true
    }

    /// Reveals the primary cursor and returns a keyboard context menu anchor at the
    /// bottom of its row. Uses current layout rather than the last frame's caret.
    pub fn context_menu_position(&mut self, p: &mut Painter) -> Option<(f32, f32)> {
        if self.geo.text.w <= 0 || self.geo.text.h <= 0 {
            return None;
        }
        self.mouse_up();
        self.scroll_to_cursor(p);
        self.reveal = false;
        let doc = self.editor.text().clone();
        let head = self.editor.selections().primary().head;
        let target = piece_of(&doc, head);
        let (first, first_row, mut y) = self.top(&mut p.text, &doc);
        let g = self.geo;
        let scroll_x = self.scroll_x;
        let mut ctx = self.line_ctx(&mut p.text);
        let (mut piece, mut skip) = (Some(first), first_row);
        while let Some(pc) = piece {
            ctx.prepare(&doc, &pc, None);
            let rows = ctx.rows();
            if pc.start == target.start {
                let row = ctx.row_at(&rows, &pc, head);
                let display = ctx.s.display.to_display(head.min(pc.end) - pc.start);
                let x = (g.text.x + g.text_pad) as f32 + rows[row].place(ctx.x_at_display(display))
                    - scroll_x;
                y += (row + 1 - skip) as i32 * g.line_h;
                return Some((
                    x.clamp(g.text.x as f32, (g.text.right() - 1) as f32),
                    (y as f32).clamp(g.text.y as f32, g.text.bottom() as f32),
                ));
            }
            y += (rows.len() - skip) as i32 * g.line_h;
            skip = 0;
            piece = next_piece(&doc, &pc);
        }
        None
    }

    pub fn mouse_down(
        &mut self,
        p: &mut Painter,
        x: f32,
        y: f32,
        clicks: u8,
        modifiers: Modifiers,
        now: Instant,
    ) -> bool {
        self.mouse = (x, y);
        let (xi, yi) = (x as i32, y as i32);
        if !self.geo.area.contains(xi, yi) {
            return false;
        }
        if self.geo.scrollbar.contains(xi, yi) {
            if let Some(thumb) = self.thumb() {
                if thumb.contains(xi, yi) {
                    self.thumb_grab = Some(y - thumb.y as f32);
                } else {
                    let page = (self.geo.visible - 1.0).max(1.0);
                    self.scroll_rows(&mut p.text, if yi < thumb.y { -page } else { page });
                }
                return true;
            }
            return false;
        }
        let at = self.hit(p, x, y);
        let doc = self.editor.text().clone();
        self.column = None;
        let in_text = x >= self.geo.text.x as f32;
        // Ctrl+Click (Cmd+Click on macOS) adds a cursor, or removes one.
        let add = if cfg!(target_os = "macos") {
            modifiers.logo
        } else {
            modifiers.ctrl
        };
        if add && clicks == 1 && in_text && !modifiers.shift {
            self.editor.toggle_cursor(at);
            self.blink_start = now;
            return true;
        }
        // Alt+drag selects a column (a box).
        if modifiers.alt && clicks == 1 && in_text {
            let corner = (doc.byte_to_line(at), self.row_x(x));
            let column = Column {
                anchor: corner,
                head: corner,
            };
            self.select_column(p, column);
            self.drag = Some(Drag {
                unit: Unit::Column,
                origin: at..at,
            });
            self.blink_start = now;
            return true;
        }
        let unit = if !in_text {
            Unit::Line
        } else {
            match clicks {
                2 => Unit::Word,
                3 => Unit::Line,
                _ => Unit::Char,
            }
        };
        let origin = match unit {
            Unit::Char | Unit::Column => at..at,
            Unit::Word => nav::word_range(&doc, at),
            Unit::Line => line_range(&doc, at),
        };
        let selection = if modifiers.shift && unit == Unit::Char {
            Selection::new(self.editor.selections().primary().anchor, at)
        } else {
            Selection::new(origin.start, origin.end)
        };
        self.editor.set_selections(Selections::single(selection));
        self.drag = Some(Drag { unit, origin });
        self.blink_start = now;
        true
    }

    /// Returns whether to redraw.
    pub fn mouse_move(&mut self, p: &mut Painter, x: f32, y: f32) -> bool {
        self.mouse = (x, y);
        if let Some(grab) = self.thumb_grab {
            let track = self.track();
            let thumb_h = self.thumb().map_or(0, |t| t.h);
            let span = (track.h - thumb_h).max(1) as f32;
            let frac = ((y - grab - track.y as f32) / span).clamp(0.0, 1.0);
            self.scroll_to_fraction(&mut p.text, f64::from(frac));
            return true;
        }
        let hover = self
            .thumb()
            .is_some_and(|t| self.geo.scrollbar.contains(x as i32, y as i32) && t.h > 0);
        let mut redraw = hover != self.hover_thumb;
        self.hover_thumb = hover;
        if self.drag.is_some() {
            self.drag_to(p, x, y);
            redraw = true;
        }
        redraw
    }

    pub fn mouse_up(&mut self) {
        self.drag = None;
        self.thumb_grab = None;
    }

    pub fn mouse_leave(&mut self) -> bool {
        std::mem::replace(&mut self.hover_thumb, false)
    }

    fn drag_to(&mut self, p: &mut Painter, x: f32, y: f32) {
        let Some(drag) = &self.drag else { return };
        let (unit, origin) = (drag.unit, drag.origin.clone());
        let at = self.hit(p, x, y);
        let doc = self.editor.text().clone();
        if unit == Unit::Column {
            if let Some(column) = self.column {
                let head = (doc.byte_to_line(at), self.row_x(x));
                self.select_column(p, Column { head, ..column });
            }
            self.reveal = true;
            return;
        }
        let selection = match unit {
            Unit::Column => unreachable!("handled above"),
            Unit::Char => Selection::new(self.editor.selections().primary().anchor, at),
            Unit::Word | Unit::Line => {
                let r = if unit == Unit::Word {
                    nav::word_range(&doc, at)
                } else {
                    line_range(&doc, at)
                };
                if at < origin.start {
                    Selection::new(origin.end, r.start)
                } else {
                    Selection::new(origin.start, r.end.max(origin.end))
                }
            }
        };
        self.editor.set_selections(Selections::single(selection));
        self.reveal = true;
    }

    // --- Multiple cursors and brackets ------------------------------------------------

    /// A window x as an x within its row of text.
    fn row_x(&self, x: f32) -> f32 {
        (x - (self.geo.text.x + self.geo.text_pad) as f32 + self.scroll_x).max(0.0)
    }

    fn select_column(&mut self, p: &mut Painter, column: Column) {
        self.column = Some(column);
        let style = self.style();
        let wrap = self.wrap_width();
        let mut ctx = LineCtx {
            text: &mut p.text,
            style,
            s: &mut self.scratch,
            wrap,
            max_x: f32::INFINITY,
        };
        self.editor
            .select_column(column.anchor, column.head, &mut ctx);
    }

    /// Shift+Alt+arrows: grows the column selection by `rows` lines or `columns` columns,
    /// starting one at the cursor.
    fn extend_column(&mut self, p: &mut Painter, rows: isize, columns: f32) {
        let doc = self.editor.text().clone();
        let column = self.column.unwrap_or_else(|| {
            let head = self.editor.selections().primary().head;
            let line = doc.byte_to_line(head);
            let x = self.line_ctx(&mut p.text).x_of(&doc, line, head);
            Column {
                anchor: (line, x),
                head: (line, x),
            }
        });
        let cell = p.metrics(&self.style()).space_advance;
        let (line, x) = column.head;
        let line = line.saturating_add_signed(rows).min(doc.line_count() - 1);
        let x = (x + columns * cell).max(0.0);
        self.select_column(
            p,
            Column {
                head: (line, x),
                ..column
            },
        );
    }

    /// Ctrl+Alt+Down (rows 1) / Up (-1): adds a cursor below the lowest (above the highest).
    pub fn add_cursor_vertical(&mut self, p: &mut Painter, rows: isize, now: Instant) -> bool {
        let style = self.style();
        let wrap = self.wrap_width();
        let mut ctx = LineCtx {
            text: &mut p.text,
            style,
            s: &mut self.scratch,
            wrap,
            max_x: f32::INFINITY,
        };
        let added = self.editor.add_cursor_vertical(rows, &mut ctx);
        self.touched(now);
        added
    }

    /// Whether the bracket at `pos` is in a string or comment, as far as syntax colors for
    /// the current text say.
    fn in_string_or_comment(&self, doc: &Rope, pos: usize) -> bool {
        let version = self.editor.buffer().version().get();
        let Some(h) = self.highlights.as_ref().filter(|h| h.version == version) else {
            return false;
        };
        let line = doc.byte_to_line(pos);
        let Some(spans) = h.line(line) else {
            return false;
        };
        let at = (pos - doc.line_to_byte(line)) as u32;
        let i = spans.partition_point(|s| s.end <= at);
        spans.get(i).is_some_and(|s| {
            s.start <= at && matches!(s.style, Style::String | Style::Comment | Style::Escape)
        })
    }

    /// The bracket next to the primary cursor and its partner, if it has one.
    fn bracket_pair(&mut self) -> Option<(usize, usize)> {
        let primary = *self.editor.selections().primary();
        if !primary.is_empty() {
            return None;
        }
        let colors = self.highlights.as_ref().map_or(0, |h| h.version);
        let key = (self.editor.buffer().version().get(), primary.head, colors);
        if let Some((k, pair)) = self.bracket
            && k == key
        {
            return pair;
        }
        let doc = self.editor.text().clone();
        let ignore = |pos: usize| self.in_string_or_comment(&doc, pos);
        let pair = nav::matching_bracket(&doc, primary.head, BRACKET_LIMIT, &ignore);
        self.bracket = Some((key, pair));
        pair
    }

    /// Moves each cursor to the bracket matching the one next to it (before the bracket,
    /// so doing it again comes back). Returns whether any moved.
    pub fn go_to_bracket(&mut self, now: Instant) -> bool {
        let doc = self.editor.text().clone();
        let mut moved = false;
        let next = self.editor.selections().map(|s| {
            let ignore = |pos: usize| self.in_string_or_comment(&doc, pos);
            match nav::matching_bracket(&doc, s.head, BRACKET_LIMIT, &ignore) {
                Some((_, to)) => {
                    moved = true;
                    Selection::cursor(to)
                }
                None => *s,
            }
        });
        if moved {
            self.editor.set_selections(next);
            self.touched(now);
        }
        moved
    }

    /// The piece at the top, its first visible row, and the y of that row.
    fn top(&mut self, text: &mut TextSystem, doc: &Rope) -> (Piece, usize, i32) {
        let g = self.geo;
        let (piece, row, sub) = self.top_piece(text, doc);
        (
            piece,
            row,
            g.text.y + g.pad_top - (sub * g.line_h as f32).round() as i32,
        )
    }

    /// The document offset under a point.
    fn hit(&mut self, p: &mut Painter, x: f32, y: f32) -> usize {
        let g = self.geo;
        let doc = self.editor.text().clone();
        let (first, first_row, y0) = self.top(&mut p.text, &doc);
        if (y as i32) < y0 && first.start == 0 && first_row == 0 {
            return 0;
        }
        let mut target = ((y - y0 as f32) / g.line_h as f32).floor().max(0.0) as usize;
        let lx = x - (g.text.x + g.text_pad) as f32 + self.scroll_x;
        let mut ctx = self.line_ctx(&mut p.text);
        let (mut piece, mut skip) = (Some(first), first_row);
        while let Some(pc) = piece {
            ctx.prepare(&doc, &pc, None);
            let rows = ctx.rows();
            let here = rows.len() - skip;
            if target < here {
                return nearest(&ctx.row_stops(&rows, skip + target, &pc), lx);
            }
            target -= here;
            skip = 0;
            piece = next_piece(&doc, &pc);
        }
        doc.len()
    }

    // --- Scrollbar ---------------------------------------------------------------------

    fn track(&self) -> Rect {
        let s = self.geo.scrollbar;
        let inset = (s.w / 4).max(1);
        Rect::new(s.x + inset, s.y + inset, s.w - 2 * inset, s.h - 2 * inset)
    }

    fn thumb(&self) -> Option<Rect> {
        if self.fits {
            return None;
        }
        let doc = self.editor.text();
        let visible = self.geo.visible;
        let track = self.track();
        // The share of the text on screen, by lines or, for long lines, by bytes.
        let by_lines = visible / (doc.line_count() as f32 - 1.0 + visible);
        let by_bytes = self.visible_bytes as f32 / doc.len().max(1) as f32;
        let h = ((track.h as f32 * by_lines.min(by_bytes)) as i32)
            .max(self.geo.min_thumb)
            .min(track.h);
        let frac = self.scroll_fraction() as f32;
        let y = track.y + ((track.h - h) as f32 * frac).round() as i32;
        Some(Rect::new(track.x, y, track.w, h))
    }

    // --- Drawing -----------------------------------------------------------------------

    fn update_geometry(&mut self, p: &mut Painter, area: Rect) {
        let px = (self.font_size * p.scale).round().max(1.0);
        let style = TextStyle::new(FontId::MONO, px);
        let m = p.metrics(&style);
        let line_h = (px * 1.5).round().max(1.0) as i32;
        let lines = self.editor.text().line_count();
        let digits = (lines.ilog10() + 1).max(3) as f32;
        let gutter_w = (digits * m.space_advance).ceil() as i32 + p.px(28.0);
        let scrollbar_w = p.px(12.0);
        let text = Rect::new(
            area.x + gutter_w,
            area.y,
            (area.w - gutter_w - scrollbar_w).max(0),
            area.h,
        );
        let pad_top = p.px(6.0);
        self.geo = Geometry {
            area,
            text,
            scrollbar: Rect::new(area.right() - scrollbar_w, area.y, scrollbar_w, area.h),
            line_h,
            pad_top,
            text_pad: p.px(8.0),
            px,
            visible: ((area.h - pad_top) as f32 / line_h as f32).max(1.0),
            min_thumb: p.px(24.0),
        };
    }

    /// Scrolls so the primary cursor's row is on screen (in the middle, if asked), and,
    /// without wrapping, its column too. Counts rows only between the top of the view and
    /// the cursor, at most a screenful.
    fn scroll_to_cursor(&mut self, p: &mut Painter) {
        let doc = self.editor.text().clone();
        let head = self.editor.selections().primary().head;
        let visible = self.geo.visible;
        let center = std::mem::take(&mut self.center);
        let piece = piece_of(&doc, head);
        let row = {
            let mut ctx = self.line_ctx(&mut p.text);
            ctx.prepare(&doc, &piece, None);
            let rows = ctx.rows();
            ctx.row_at(&rows, &piece, head)
        };
        let (top, top_row, sub) = self.top_piece(&mut p.text, &doc);
        let here = (piece.start, row);
        let at_top = (top.start, top_row);
        let back = if center {
            Some((visible / 2.0).floor())
        } else if here < at_top || (here == at_top && sub > 0.0) {
            Some(0.0)
        } else {
            // Rows from the top of the view to the bottom of the cursor's row.
            let mut below = -sub;
            let (mut pc, mut r) = (top, top_row);
            loop {
                if pc.start == piece.start {
                    below += (row + 1 - r) as f32;
                    break;
                }
                below += (self.piece_rows(&mut p.text, &doc, &pc) - r) as f32;
                match next_piece(&doc, &pc) {
                    Some(next) if below <= visible => (pc, r) = (next, 0),
                    _ => break,
                }
            }
            (below > visible).then_some(visible - 1.0)
        };
        if let Some(back) = back {
            self.top = Top {
                at: piece.start,
                row,
                sub: 0.0,
            };
            self.scroll_rows(&mut p.text, -back);
        }

        if self.wrap {
            self.scroll_x = 0.0;
            return;
        }
        let style = self.style();
        let column = p.metrics(&style).space_advance;
        let x = self.line_ctx(&mut p.text).x_of(&doc, piece.line, head);
        let width = (self.geo.text.w - self.geo.text_pad) as f32;
        let margin = (column * 4.0).min(width / 3.0);
        if x < self.scroll_x + margin {
            self.scroll_x = (x - margin).max(0.0);
        } else if x > self.scroll_x + width - margin {
            self.scroll_x = x - width + margin;
        }
    }

    pub fn draw(&mut self, p: &mut Painter, area: Rect, now: Instant) {
        self.update_geometry(p, area);
        if let Some(line) = self.goto_line.take() {
            let lines = self.editor.text().line_count().max(2) - 1;
            self.scroll_to_fraction(&mut p.text, line / lines as f64);
        }
        if std::mem::take(&mut self.reveal) {
            self.scroll_to_cursor(p);
        }
        if self.wrap {
            self.scroll_x = 0.0;
        }
        let t = p.theme;
        let g = self.geo;
        let style = self.style();
        let m = p.metrics(&style);
        p.rect(area, t.editor_bg);

        let doc = self.editor.text().clone();
        let sels = self.editor.selections().clone();
        let primary = *sels.primary();
        let cursor_line = doc.byte_to_line(primary.head);
        let (first, first_row, y0) = self.top(&mut p.text, &doc);

        // The visible pieces, so matches can be found once for the range.
        let mut visible: Vec<Piece> = Vec::new();
        let mut reached_end = false;
        let mut rows_shown = 0usize;
        {
            let mut rows_left = ((g.text.bottom() - y0) / g.line_h + 1).max(0) as usize;
            let mut ctx = self.line_ctx(&mut p.text);
            let (mut piece, mut skip) = (Some(first), first_row);
            while let Some(pc) = piece {
                if rows_left == 0 {
                    break;
                }
                let n = if ctx.wrap.is_some() {
                    ctx.prepare(&doc, &pc, None);
                    ctx.rows().len()
                } else {
                    1
                };
                let shown = n - skip.min(n - 1);
                rows_left = rows_left.saturating_sub(shown);
                rows_shown += shown;
                visible.push(pc);
                skip = 0;
                piece = next_piece(&doc, &pc);
                reached_end = piece.is_none();
            }
        }
        let last = *visible.last().unwrap_or(&first);
        self.visible_lines = first.line..last.line + 1;
        // Only the selections on screen are drawn: there can be thousands elsewhere.
        let shown_sels: Vec<Selection> = sels
            .touching(first.start..last.span.next.max(last.end))
            .to_vec();
        let active_lines: Vec<usize> = shown_sels
            .iter()
            .map(|s| doc.byte_to_line(s.head))
            .collect();
        self.visible_bytes = last.end - first.start;
        self.fits = first.start == 0
            && first_row == 0
            && y0 == g.text.y + g.pad_top
            && reached_end
            && rows_shown as f32 <= g.visible;
        let matches = match &self.search {
            Some(s) if !visible.is_empty() => {
                let end = if last.is_last() {
                    last.span.next.max(last.end + 1)
                } else {
                    last.end + 1
                };
                s.matches_in(&doc, first.start..end.min(doc.len()))
            }
            _ => Vec::new(),
        };
        let origin = (g.text.x + g.text_pad) as f32 - self.scroll_x;
        let caret_on = self.focused && self.blink_on(now);
        let selection_color = if self.focused {
            t.selection
        } else {
            t.selection_inactive
        };
        let caret_w = p.px(2.0).max(1);
        let t_bracket = t.bracket_match;
        let outline = p.px(1.0).max(1);
        let caret_h = ((m.ascent + m.descent) * 1.15).round() as i32;
        let gap = m.space_advance * 0.5;
        let preedit = self.preedit.clone();
        let brackets = self.bracket_pair();
        let wrap = self.wrap_width();
        let mut max_width = 0.0f32;
        let mut row_layout = LineLayout::default();
        self.caret = None;

        p.list.push_clip(area);
        // Without wrapping, a row only needs laying out as far as the view shows.
        let layout_until = if wrap.is_none() {
            self.scroll_x + g.text.w as f32 + 4.0 * m.space_advance
        } else {
            f32::INFINITY
        };
        let mut y = y0;
        for (k, &piece) in visible.iter().enumerate() {
            let span = piece.span;
            let composing = preedit
                .as_ref()
                .filter(|_| piece.holds(primary.head))
                .map(|(s, _)| (primary.head, s.as_str()));
            let mut ctx = LineCtx {
                text: &mut p.text,
                style,
                s: &mut self.scratch,
                wrap,
                max_x: layout_until,
            };
            ctx.prepare(&doc, &piece, composing);
            let rows = ctx.rows();
            max_width = max_width.max(self.scratch.layout.width);
            // Syntax colors (line-relative) as display ranges of this piece, in order.
            let (off, len) = (piece.offset(), piece.end - piece.start);
            let colored: Vec<(usize, usize, fude_render::Color)> = self
                .highlights
                .as_ref()
                .and_then(|h| h.line(piece.line))
                .unwrap_or(&[])
                .iter()
                .filter(|s| s.end as usize > off && (s.start as usize) < off + len)
                .map(|s| {
                    let d = &self.scratch.display;
                    let at = |x: u32| (x as usize).saturating_sub(off).min(len);
                    (
                        d.to_display(at(s.start)),
                        d.to_display(at(s.end)),
                        t.style(s.style),
                    )
                })
                .collect();
            let skip = if k == 0 {
                first_row.min(rows.len() - 1)
            } else {
                0
            };
            let line_top = y;
            let piece_rows = (rows.len() - skip) as i32;
            if piece.line == cursor_line && primary.is_empty() {
                p.rect(
                    Rect::new(area.x, y, area.w, g.line_h * piece_rows),
                    t.line_highlight,
                );
            }

            for (r, row) in rows.iter().enumerate().skip(skip) {
                if y >= g.text.bottom() {
                    break;
                }
                let last_row = r + 1 == rows.len();
                let baseline = p.center_baseline(y, g.line_h, &style);
                // A display range's x extent within this row, if it touches the row.
                let ctx = LineCtx {
                    text: &mut p.text,
                    style,
                    s: &mut self.scratch,
                    wrap,
                    max_x: f32::INFINITY,
                };
                let extent = |a: usize, b: usize| -> Option<(f32, f32)> {
                    let (a, b) = (a.max(row.start), b.min(row.end));
                    if a > b || (a == b && a != row.start) {
                        return None;
                    }
                    Some((
                        origin + row.place(ctx.x_at_display(a)),
                        origin + row.place(ctx.x_at_display(b)),
                    ))
                };
                p.list.push_clip(g.text);
                for m in matches
                    .iter()
                    .filter(|m| m.start <= piece.end && m.end > piece.start)
                {
                    let a = ctx
                        .s
                        .display
                        .to_display(m.start.max(piece.start) - piece.start);
                    let b = ctx.s.display.to_display(m.end.min(piece.end) - piece.start);
                    if let Some((x0, x1)) = extent(a, b).filter(|_| b > row.start || r == 0) {
                        p.list.rect(
                            Rect::new(
                                x0.round() as i32,
                                y,
                                (x1 - x0).round().max(2.0) as i32,
                                g.line_h,
                            ),
                            t.find_match,
                        );
                    }
                }
                // Selections, extended a little past the line end when they include the break.
                for s in shown_sels.iter().filter(|s| !s.is_empty()) {
                    if s.end() < piece.start
                        || s.start() > piece.end
                        || s.end() == piece.start && piece.start != piece.end
                    {
                        continue;
                    }
                    let a = ctx
                        .s
                        .display
                        .to_display(s.start().max(piece.start) - piece.start);
                    let b = ctx
                        .s
                        .display
                        .to_display(s.end().min(piece.end) - piece.start);
                    let Some((x0, mut x1)) = extent(a, b) else {
                        continue;
                    };
                    if b > row.end && !last_row {
                        x1 = origin + row.place(ctx.x_at_display(row.end));
                    }
                    if last_row && piece.is_last() && s.end() > span.end && span.next > span.end {
                        x1 += gap;
                    }
                    if x1 > x0 {
                        p.list.rect(
                            Rect::new(x0.round() as i32, y, (x1 - x0).round() as i32, g.line_h),
                            selection_color,
                        );
                    }
                }
                // The bracket by the cursor and its partner, outlined.
                for b in brackets.iter().flat_map(|&(a, b)| [a, b]) {
                    if b < piece.start || b >= piece.end {
                        continue;
                    }
                    let d = ctx.s.display.to_display(b - piece.start);
                    if d < row.start || d >= row.end {
                        continue;
                    }
                    let x0 = (origin + row.place(ctx.x_at_display(d))).round() as i32;
                    let x1 = (origin + row.place(ctx.x_at_display(d + 1))).round() as i32;
                    let (w, t) = ((x1 - x0).max(2), outline);
                    let top = y + (g.line_h - caret_h) / 2;
                    let color = t_bracket;
                    p.list.rect(Rect::new(x0, top, w, t), color);
                    p.list.rect(Rect::new(x0, top + caret_h - t, w, t), color);
                    p.list.rect(Rect::new(x0, top, t, caret_h), color);
                    p.list.rect(Rect::new(x0 + w - t, top, t, caret_h), color);
                }
                // The row's glyphs on screen, moved to the row's start. Glyphs advance
                // left to right (shaped runs come in visual order), so a row of a long
                // piece scrolled sideways is cut to what shows by binary search.
                let visible_x = (g.text.x as f32, g.text.right() as f32);
                let glyphs = &ctx.s.layout.glyphs;
                let shown = if rows.len() == 1 {
                    let lo = visible_x.0 - origin + row.x - row.indent - 4.0 * m.space_advance;
                    let hi = visible_x.1 - origin + row.x - row.indent;
                    let a = glyphs.partition_point(|gl| gl.x < lo);
                    let b = glyphs.partition_point(|gl| gl.x <= hi);
                    &glyphs[a..b.max(a)]
                } else {
                    &glyphs[..]
                };
                row_layout.glyphs.clear();
                row_layout.glyphs.extend(
                    shown
                        .iter()
                        .filter(|gl| {
                            (gl.cluster as usize) >= row.start && (gl.cluster as usize) < row.end
                        })
                        .map(|gl| fude_text::PositionedGlyph {
                            x: row.place(gl.x),
                            ..*gl
                        }),
                );
                // Runs of glyphs with the same color.
                let mut run = LineLayout::default();
                let mut run_color = t.text;
                let mut k = 0;
                for gl in &row_layout.glyphs {
                    let c = gl.cluster as usize;
                    while k < colored.len() && colored[k].1 <= c {
                        k += 1;
                    }
                    let color = match colored.get(k) {
                        Some(&(a, _, color)) if a <= c => color,
                        _ => t.text,
                    };
                    if color != run_color && !run.glyphs.is_empty() {
                        p.text.draw_line(
                            &run,
                            &style,
                            origin,
                            baseline,
                            run_color,
                            visible_x,
                            &mut p.list,
                            &mut p.images,
                        );
                        run.glyphs.clear();
                    }
                    run_color = color;
                    run.glyphs.push(*gl);
                }
                if !run.glyphs.is_empty() {
                    p.text.draw_line(
                        &run,
                        &style,
                        origin,
                        baseline,
                        run_color,
                        visible_x,
                        &mut p.list,
                        &mut p.images,
                    );
                }
                Self::draw_chips(p, &row_layout, &style, origin, y, g.line_h);
                if let Some(range) = self.scratch.display.preedit.clone() {
                    let ctx = LineCtx {
                        text: &mut p.text,
                        style,
                        s: &mut self.scratch,
                        wrap,
                        max_x: f32::INFINITY,
                    };
                    let (a, b) = (range.start.max(row.start), range.end.min(row.end));
                    if a < b {
                        let x0 = origin + row.place(ctx.x_at_display(a));
                        let x1 = origin + row.place(ctx.x_at_display(b));
                        let thickness = p.px(1.0).max(1);
                        p.rect(
                            Rect::new(
                                x0 as i32,
                                baseline + thickness * 2,
                                (x1 - x0) as i32,
                                thickness,
                            ),
                            t.preedit,
                        );
                    }
                }
                // Carets on this row.
                for s in shown_sels.iter() {
                    if !piece.holds(s.head) {
                        continue;
                    }
                    let ctx = LineCtx {
                        text: &mut p.text,
                        style,
                        s: &mut self.scratch,
                        wrap,
                        max_x: f32::INFINITY,
                    };
                    let mut display = ctx.s.display.to_display(s.head - piece.start);
                    if *s == primary
                        && let (Some(range), Some((_, Some((c, _))))) =
                            (&ctx.s.display.preedit, &preedit)
                    {
                        display = range.start + c;
                    }
                    if LineCtx::row_of(&rows, display) != r {
                        continue;
                    }
                    let x = (origin + row.place(ctx.x_at_display(display))).round() as i32;
                    let rect = Rect::new(x, y + (g.line_h - caret_h) / 2, caret_w, caret_h);
                    if *s == primary {
                        self.caret = Some(rect);
                    }
                    if caret_on {
                        p.rect(rect, t.caret);
                    }
                }
                p.list.pop_clip();
                y += g.line_h;
            }

            // Line number, on the line's first row.
            if piece.is_first() && skip == 0 {
                let baseline = p.center_baseline(line_top, g.line_h, &style);
                let active = active_lines.contains(&piece.line);
                let number = (piece.line + 1).to_string();
                let w = p.measure(&number, &style);
                let x = (g.text.x - p.px(16.0)) as f32 - w;
                let color = if active {
                    t.gutter_text_active
                } else {
                    t.gutter_text
                };
                p.text(&number, &style, x, baseline, color);
            }
        }
        self.max_width = max_width;

        if let Some(thumb) = self.thumb() {
            let color = if self.hover_thumb || self.thumb_grab.is_some() {
                t.scrollbar_hover
            } else {
                t.scrollbar
            };
            p.rect(thumb, color);
        }
        p.list.pop_clip();
    }

    fn draw_chips(
        p: &mut Painter,
        layout: &LineLayout,
        style: &TextStyle,
        origin: f32,
        y: i32,
        line_h: i32,
    ) {
        let t = p.theme;
        let chips: Vec<(f32, Chip)> = layout
            .glyphs
            .iter()
            .filter(|g| g.font == FontId::CHIP)
            .filter_map(|g| Some((g.x, Chip::decode(char::from_u32(g.glyph)?)?)))
            .collect();
        for (gx, chip) in chips {
            let geo = p.text.chip_geometry(chip, style);
            let lm = p.metrics(&geo.label);
            let h = ((lm.ascent + lm.descent).round() as i32 + p.px(2.0)).min(line_h);
            let x = (origin + gx + geo.margin).round() as i32;
            let top = y + (line_h - h) / 2;
            let (bg, fg) = match chip {
                Chip::Byte(_) => (t.invalid_bg, t.invalid_text),
                Chip::Control(_) => (t.chip_bg, t.chip_text),
            };
            p.rect(Rect::new(x, top, geo.width.round() as i32, h), bg);
            let baseline = p.center_baseline(top, h, &geo.label);
            p.text(
                &chip.label(),
                &geo.label,
                x as f32 + geo.label_x,
                baseline,
                fg,
            );
        }
    }
}

/// The line containing `at`, with its line break: what a triple click selects.
fn line_range(doc: &Rope, at: usize) -> Range<usize> {
    let span = doc.line_span(doc.byte_to_line(at));
    span.start..span.next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::SUMI;
    use fude_buffer::Format;

    /// Input methods send empty updates freely (KWin answers every caret move with one).
    /// They mustn't pull a scrolled view back to the caret, or ask for a redraw.
    #[test]
    fn empty_ime_updates_leave_the_scroll_alone() {
        let text: String = (0..500).map(|i| format!("line {i}\n")).collect();
        let editor = Editor::new(Rope::from(text.as_bytes()), Format::default());
        let mut view = EditorView::new(editor);
        let mut p = Painter::new(&SUMI);
        let (area, now) = (Rect::new(0, 0, 800, 600), Instant::now());
        p.begin(1.0);
        view.draw(&mut p, area, now);
        let none = Modifiers::default();
        view.scroll(&mut p, 0.0, 30.0, ScrollUnit::Lines, none);
        let scrolled = view.scroll_position();
        assert!(
            view.top_line() > 50.0,
            "scrolled well past the caret: {scrolled:?}"
        );

        for ime in [
            Ime::Enabled,
            Ime::Preedit(String::new(), None),
            Ime::Commit(String::new()),
            Ime::Disabled,
        ] {
            assert!(!view.ime(ime, now), "nothing to redraw");
            p.begin(1.0);
            view.draw(&mut p, area, now);
            assert_eq!(view.scroll_position(), scrolled);
        }

        // Composing does bring the caret back, like typing.
        assert!(view.ime(Ime::Preedit("か".into(), None), now));
        assert!(
            !view.ime(Ime::Preedit("か".into(), None), now),
            "the same again"
        );
        p.begin(1.0);
        view.draw(&mut p, area, now);
        assert_eq!(view.top_line(), 0.0);
        assert!(
            view.ime(Ime::Preedit(String::new(), None), now),
            "composition ends"
        );
    }

    fn view_of(text: &str, wrap: bool) -> (EditorView, Painter, Rect) {
        let editor = Editor::new(Rope::from(text), Format::default());
        let mut view = EditorView::new(editor);
        view.wrap = wrap;
        let mut p = Painter::new(&SUMI);
        let area = Rect::new(0, 0, 800, 600);
        p.begin(1.0);
        view.draw(&mut p, area, Instant::now());
        (view, p, area)
    }

    fn key(view: &mut EditorView, p: &mut Painter, key: Key, ctrl: bool) {
        let input = KeyInput {
            key,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                ctrl,
                ..Modifiers::default()
            },
            text: None,
            base: None,
        };
        view.key(p, &input, Instant::now());
    }

    fn draw(view: &mut EditorView, p: &mut Painter, area: Rect) {
        p.begin(1.0);
        view.draw(p, area, Instant::now());
    }

    fn first_line_point(view: &EditorView, p: &mut Painter, prefix: &str) -> (f32, f32) {
        let g = view.geo;
        let x = (g.text.x + g.text_pad) as f32 + p.measure(prefix, &view.style());
        let y = (g.text.y + g.pad_top + g.line_h / 2) as f32;
        (x, y)
    }

    #[test]
    fn context_click_preserves_selections_and_targets_unselected_text() {
        let (mut view, mut p, _) = view_of("alpha beta gamma", false);
        let selections = Selections::new(vec![Selection::new(5, 0), Selection::new(11, 16)], 0);
        view.editor.set_selections(selections.clone());
        for prefix in ["", "alp", "alpha beta ga"] {
            let (x, y) = first_line_point(&view, &mut p, prefix);
            assert!(view.context_menu_at(&mut p, x, y, Instant::now()));
            assert_eq!(*view.editor.selections(), selections);
        }

        // The selection's end is outside its range, so this becomes one caret.
        let (x, y) = first_line_point(&view, &mut p, "alpha");
        assert!(view.context_menu_at(&mut p, x, y, Instant::now()));
        assert_eq!(
            *view.editor.selections(),
            Selections::single(Selection::cursor(5))
        );
    }

    #[test]
    fn context_click_keeps_existing_cursors_and_cancels_drag() {
        let (mut view, mut p, _) = view_of("alpha beta gamma", false);
        let (x, y) = first_line_point(&view, &mut p, "alpha b");
        view.mouse_down(&mut p, x, y, 1, Modifiers::default(), Instant::now());
        assert!(view.drag.is_some());
        let selections = Selections::new(vec![Selection::cursor(0), Selection::cursor(7)], 0);
        view.editor.set_selections(selections.clone());
        assert!(view.context_menu_at(&mut p, x, y, Instant::now()));
        assert!(view.drag.is_none());
        view.mouse_move(&mut p, x + 50.0, y);
        assert_eq!(*view.editor.selections(), selections);
    }

    #[test]
    fn context_click_ignores_scrollbar_and_outside_view() {
        let (mut view, mut p, _) = view_of("alpha beta gamma", false);
        view.editor.select(0..5);
        let selections = view.editor.selections().clone();
        let scrollbar = view.geo.scrollbar;
        for (x, y) in [
            (scrollbar.x as f32, scrollbar.y as f32 + 20.0),
            (-10.0, 20.0),
            (100.0, 700.0),
        ] {
            assert!(!view.context_menu_at(&mut p, x, y, Instant::now()));
            assert_eq!(*view.editor.selections(), selections);
        }

        // The gutter targets the line's start without selecting the whole line.
        assert!(view.context_menu_at(&mut p, 10.0, 45.0, Instant::now()));
        assert!(view.editor.selections().primary().is_empty());
    }

    #[test]
    fn context_click_on_secondary_selection_preserves_scrolled_view() {
        let text: String = (0..500).map(|i| format!("line {i}\n")).collect();
        let (mut view, mut p, area) = view_of(&text, false);
        view.scroll(&mut p, 0.0, 30.0, ScrollUnit::Lines, Modifiers::default());
        draw(&mut view, &mut p, area);
        let scroll = view.scroll_position();
        let (x, y) = first_line_point(&view, &mut p, "");
        let at = view.hit(&mut p, x, y);
        assert!(at > 0);
        let selections = Selections::new(vec![Selection::cursor(0), Selection::new(at, at + 4)], 0);
        view.editor.set_selections(selections.clone());
        view.changed(Instant::now());
        assert!(view.context_menu_at(&mut p, x, y, Instant::now()));
        draw(&mut view, &mut p, area);
        assert_eq!(*view.editor.selections(), selections);
        assert_eq!(view.scroll_position(), scroll);
    }

    #[test]
    fn keyboard_context_menu_uses_current_cursor_and_reveals_it() {
        let text = "alpha beta gamma\n".repeat(500);
        for wrap in [false, true] {
            let (mut view, mut p, area) = view_of(&text, wrap);
            view.editor.select(text.len() - 8..text.len() - 8);
            let (x, y) = view.context_menu_position(&mut p).unwrap();
            assert_eq!(
                view.hit(&mut p, x, y - 1.0),
                view.editor.selections().primary().head
            );
            assert!(
                view.geo.text.contains(x as i32, y as i32 - 1),
                "anchor ({x}, {y}), area {:?}, top {:?}",
                view.geo.text,
                view.top
            );
            draw(&mut view, &mut p, area);
            let caret = view.caret_rect().unwrap();
            assert!((x - caret.x as f32).abs() <= 1.0);
            assert!(y >= caret.y as f32);
        }
    }

    /// A line of 1 MB: drawing, moving and clicking anywhere in it only lays out pieces.
    #[test]
    fn long_lines_work_anywhere_in_them() {
        let line = "{\"key\":\"value\",\"n\":12345},".repeat(40_000);
        for wrap in [false, true] {
            let (mut view, mut p, area) = view_of(&format!("first\n{line}\nlast"), wrap);
            assert!(
                view.thumb().is_some(),
                "a long line needs a scrollbar (wrap {wrap})"
            );

            // To the end and back: the caret is on screen each time.
            key(&mut view, &mut p, Key::End, true);
            draw(&mut view, &mut p, area);
            let caret = view.caret_rect().expect("caret drawn");
            assert!(area.contains(caret.x, caret.y), "{caret:?}");
            assert!(
                view.top_line() > 1.5,
                "scrolled into the long line: {}",
                view.top_line()
            );
            key(&mut view, &mut p, Key::Home, true);
            draw(&mut view, &mut p, area);
            assert_eq!(view.top_line(), 0.0);

            // Down from the first line goes into the long one, a row (piece) at a time.
            key(&mut view, &mut p, Key::Down, false);
            key(&mut view, &mut p, Key::Down, false);
            draw(&mut view, &mut p, area);
            let head = view.editor.selections().primary().head;
            assert!(
                head > 6 + 16 && head < 6 + 2 * PIECE + 256,
                "second row: {head}"
            );
            key(&mut view, &mut p, Key::Up, false);
            key(&mut view, &mut p, Key::Up, false);
            assert!(view.editor.selections().primary().head <= 5);
            draw(&mut view, &mut p, area);

            // A click on the third row lands in the long line, past its first row.
            let caret = view.caret_rect().unwrap();
            let y = caret.y + 2 * view.geo.line_h + view.geo.line_h / 2;
            let at = view.hit(&mut p, caret.x as f32 + 30.0, y as f32);
            assert!(at > 6 + 16 && at < 6 + 3 * (PIECE + 256), "clicked {at}");

            // Scrolling stops with the last row at the top.
            view.scroll(&mut p, 0.0, 1e9, ScrollUnit::Pixels, Modifiers::default());
            draw(&mut view, &mut p, area);
            assert!(
                view.visible_lines().contains(&2),
                "{:?}",
                view.visible_lines()
            );
            assert_eq!(view.top_line(), 2.0);
        }
    }

    /// Without wrapping, the row a long line's piece is drawn in fills the screen's width
    /// even when laid out only as far as it shows.
    #[test]
    fn long_rows_draw_to_the_edge() {
        let line = "x".repeat(3 * PIECE);
        let (mut view, mut p, area) = view_of(&line, false);
        key(&mut view, &mut p, Key::End, false);
        draw(&mut view, &mut p, area);
        let caret = view.caret_rect().unwrap();
        assert!(
            area.contains(caret.x, caret.y),
            "the caret at the end shows: {caret:?}"
        );
        assert!(view.max_width > area.w as f32, "rows extend past the view");
    }
}

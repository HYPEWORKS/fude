//! The editor view: draws the visible part of a document and turns keys, mouse and IME
//! input into editor commands.
//!
//! Only visible lines are ever laid out, so a frame costs the same at any file size.
//! The caret blinks for a while after input, then stays solid, so an idle window wakes
//! no timers at all.

use std::ops::Range;
use std::time::{Duration, Instant};

use fude_buffer::{LineSpan, Rope};
use fude_editor::{Editor, Layout, Motion, Searcher, Selection, Selections, nearest, text as nav};
use fude_platform::{CursorShape, Ime, Key, KeyInput, Modifiers, ScrollUnit};
use fude_render::Rect;
use fude_text::{Chip, FontId, LineLayout, TextStyle, TextSystem};

use fude_syntax::Span;

use crate::display::DisplayLine;

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

/// Lines longer than this are cut off for display (long-line layout is phase 4 work).
const MAX_DISPLAY_BYTES: usize = 16 * 1024;
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
}

/// A mouse selection in progress: what was clicked, and by what unit it grows.
struct Drag {
    unit: Unit,
    origin: Range<usize>,
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

/// Lays out single lines for drawing, hit testing and vertical movement.
struct LineCtx<'a> {
    text: &'a mut TextSystem,
    style: TextStyle,
    s: &'a mut Scratch,
    /// Wrap width, if wrapping.
    wrap: Option<f32>,
}

impl LineCtx<'_> {
    fn prepare(&mut self, doc: &Rope, span: LineSpan, preedit: Option<(usize, &str)>) {
        self.s.bytes.clear();
        let end = span.end.min(span.start + MAX_DISPLAY_BYTES);
        doc.copy_range(span.start..end, &mut self.s.bytes);
        let preedit = preedit.map(|(at, text)| (at - span.start, text));
        self.s.display.build(&self.s.bytes, preedit);
        self.text
            .layout_line(&self.s.display.text, &self.style, &mut self.s.layout);
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
    /// of the line is a stop on the last row only; on other rows it's the next row's start.
    fn row_stops(&self, rows: &[Row], index: usize, line_start: usize) -> Vec<(usize, f32)> {
        let row = rows[index];
        let mut stops: Vec<(usize, f32)> = self
            .clusters()
            .into_iter()
            .filter(|&(c, _)| c >= row.start && c < row.end)
            .map(|(c, x)| (line_start + self.s.display.to_doc(c), row.place(x)))
            .collect();
        if index + 1 == rows.len() {
            stops.push((
                line_start + self.s.bytes.len(),
                row.place(self.s.layout.width),
            ));
        }
        if stops.is_empty() {
            stops.push((line_start + self.s.display.to_doc(row.start), row.indent));
        }
        stops
    }
}

impl Layout for LineCtx<'_> {
    fn x_of(&mut self, doc: &Rope, line: usize, at: usize) -> f32 {
        let span = doc.line_span(line);
        self.prepare(doc, span, None);
        let d = self.s.display.to_display(at - span.start);
        let rows = self.rows();
        rows[LineCtx::row_of(&rows, d)].place(self.x_at_display(d))
    }

    fn offset_at(&mut self, doc: &Rope, line: usize, x: f32) -> usize {
        let span = doc.line_span(line);
        self.prepare(doc, span, None);
        let rows = self.rows();
        nearest(&self.row_stops(&rows, 0, span.start), x)
    }

    fn vertical(
        &mut self,
        doc: &Rope,
        at: usize,
        goal_x: Option<f32>,
        rows: isize,
    ) -> (usize, Option<f32>) {
        let mut line = doc.byte_to_line(at);
        let mut span = doc.line_span(line);
        self.prepare(doc, span, None);
        let mut rs = self.rows();
        let d = self.s.display.to_display(at - span.start);
        let mut r = LineCtx::row_of(&rs, d);
        let goal = goal_x.unwrap_or_else(|| rs[r].place(self.x_at_display(d)));
        let mut left = rows;
        while left > 0 {
            if r + 1 < rs.len() {
                r += 1;
            } else if line + 1 < doc.line_count() {
                line += 1;
                span = doc.line_span(line);
                self.prepare(doc, span, None);
                rs = self.rows();
                r = 0;
            } else {
                return (doc.len(), None);
            }
            left -= 1;
        }
        while left < 0 {
            if r > 0 {
                r -= 1;
            } else if line > 0 {
                line -= 1;
                span = doc.line_span(line);
                self.prepare(doc, span, None);
                rs = self.rows();
                r = rs.len() - 1;
            } else {
                return (0, None);
            }
            left += 1;
        }
        (
            nearest(&self.row_stops(&rs, r, span.start), goal),
            Some(goal),
        )
    }
}

pub struct EditorView {
    pub editor: Editor,
    /// Logical font size; zoom changes it.
    font_size: f32,
    /// The Tab key inserts spaces up to the next tab stop.
    pub insert_spaces: bool,
    /// Matches of this search are highlighted in the visible lines.
    pub search: Option<Searcher>,
    /// Wrap long lines at the view's edge.
    pub wrap: bool,
    /// Syntax colors (possibly from a slightly older version while new ones are made).
    pub highlights: Option<Highlights>,
    /// The lines drawn in the last frame.
    visible_lines: Range<usize>,
    /// First visible line, fractional for smooth scrolling.
    scroll_line: f32,
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
            insert_spaces: false,
            search: None,
            wrap: false,
            highlights: None,
            visible_lines: 0..0,
            scroll_line: 0.0,
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
        }
    }

    /// How many rows `line` takes (1 without wrapping).
    fn line_rows(&mut self, text: &mut TextSystem, doc: &Rope, line: usize) -> usize {
        if !self.wrap {
            return 1;
        }
        let mut ctx = self.line_ctx(text);
        ctx.prepare(doc, doc.line_span(line), None);
        ctx.rows().len()
    }

    /// Scrolls by `delta` visual rows. The position is a line plus a fraction of that
    /// line's rows, so scrolling only lays out the lines it passes.
    fn scroll_rows(&mut self, text: &mut TextSystem, delta: f32) {
        if !self.wrap {
            self.scroll_lines(delta);
            return;
        }
        let doc = self.editor.text().clone();
        let last = doc.line_count() - 1;
        let mut line = (self.scroll_line.floor() as usize).min(last);
        let mut n = self.line_rows(text, &doc, line) as f32;
        let mut pos = (self.scroll_line - line as f32) * n + delta;
        while pos >= n && line < last {
            pos -= n;
            line += 1;
            n = self.line_rows(text, &doc, line) as f32;
        }
        while pos < 0.0 && line > 0 {
            line -= 1;
            n = self.line_rows(text, &doc, line) as f32;
            pos += n;
        }
        let pos = pos.clamp(0.0, (n - 1.0).max(0.0));
        self.scroll_line = line as f32 + pos / n;
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
        if !key.pressed {
            return false;
        }
        let m = key.modifiers;
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
            Key::Tab if !m.primary() && !m.shift && !m.alt => {
                if self.insert_spaces {
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

    pub fn ime(&mut self, ime: Ime, now: Instant) {
        match ime {
            Ime::Preedit(text, cursor) if !text.is_empty() => self.preedit = Some((text, cursor)),
            Ime::Commit(text) => {
                self.preedit = None;
                self.editor.insert(&text, now);
            }
            _ => self.preedit = None,
        }
        self.touched(now);
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

    fn scroll_lines(&mut self, lines: f32) {
        self.scroll_line = (self.scroll_line + lines).clamp(0.0, self.max_scroll());
    }

    fn scroll_px(&mut self, px: f32) {
        let limit = (self.max_width - (self.geo.text.w - self.geo.text_pad) as f32 / 2.0).max(0.0);
        self.scroll_x = (self.scroll_x + px).clamp(0.0, limit.max(self.scroll_x));
    }

    fn max_scroll(&self) -> f32 {
        (self.editor.text().line_count() as f32 - 1.0).max(0.0)
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

    /// The scroll position, to restore after a preview.
    pub fn scroll_position(&self) -> (f32, f32) {
        (self.scroll_line, self.scroll_x)
    }

    pub fn set_scroll_position(&mut self, (line, x): (f32, f32)) {
        self.scroll_line = line.clamp(0.0, self.max_scroll());
        self.scroll_x = x.max(0.0);
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
        let unit = if x < self.geo.text.x as f32 {
            Unit::Line
        } else {
            match clicks {
                2 => Unit::Word,
                3 => Unit::Line,
                _ => Unit::Char,
            }
        };
        let origin = match unit {
            Unit::Char => at..at,
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
            self.scroll_line = frac * self.max_scroll();
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
        let selection = match unit {
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

    /// The first visible line, the row within it, and the y of the first drawn row.
    fn top(&mut self, text: &mut TextSystem, doc: &Rope) -> (usize, usize, i32) {
        let g = self.geo;
        let first = (self.scroll_line.floor() as usize).min(doc.line_count() - 1);
        let n = self.line_rows(text, doc, first);
        let rows_in = (self.scroll_line - first as f32) * n as f32;
        let row = (rows_in.floor() as usize).min(n - 1);
        let sub = rows_in - row as f32;
        (
            first,
            row,
            g.text.y + g.pad_top - (sub * g.line_h as f32).round() as i32,
        )
    }

    /// The document offset under a point.
    fn hit(&mut self, p: &mut Painter, x: f32, y: f32) -> usize {
        let g = self.geo;
        let doc = self.editor.text().clone();
        let (first, first_row, y0) = self.top(&mut p.text, &doc);
        if (y as i32) < y0 && first == 0 && first_row == 0 {
            return 0;
        }
        let mut target = ((y - y0 as f32) / g.line_h as f32).floor().max(0.0) as usize;
        let lx = x - (g.text.x + g.text_pad) as f32 + self.scroll_x;
        let mut ctx = self.line_ctx(&mut p.text);
        for (i, span) in (first..).zip(doc.lines_from(first)) {
            ctx.prepare(&doc, span, None);
            let rows = ctx.rows();
            let skip = if i == first { first_row } else { 0 };
            let here = rows.len() - skip;
            if target < here {
                return nearest(&ctx.row_stops(&rows, skip + target, span.start), lx);
            }
            target -= here;
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
        let lines = self.editor.text().line_count() as f32;
        let visible = self.geo.visible;
        if lines <= visible.floor() {
            return None;
        }
        let track = self.track();
        let total = lines - 1.0 + visible;
        let h = ((track.h as f32 * visible / total) as i32)
            .max(self.geo.min_thumb)
            .min(track.h);
        let frac = self.scroll_line / self.max_scroll().max(1.0);
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

    fn scroll_to_cursor(&mut self, p: &mut Painter) {
        let doc = self.editor.text().clone();
        let head = self.editor.selections().primary().head;
        let line = doc.byte_to_line(head);
        let visible = self.geo.visible.floor().max(1.0);
        if self.wrap {
            self.scroll_to_cursor_wrapped(p, &doc, head, line, visible);
            return;
        }
        if std::mem::take(&mut self.center) {
            self.scroll_line = line as f32 - (visible / 2.0).floor();
        } else if (line as f32) < self.scroll_line {
            self.scroll_line = line as f32;
        } else if line as f32 + 1.0 > self.scroll_line + visible {
            self.scroll_line = line as f32 + 1.0 - visible;
        }
        self.scroll_line = self.scroll_line.clamp(0.0, self.max_scroll());

        let style = self.style();
        let column = p.metrics(&style).space_advance;
        let x = self.line_ctx(&mut p.text).x_of(&doc, line, head);
        let width = (self.geo.text.w - self.geo.text_pad) as f32;
        let margin = (column * 4.0).min(width / 3.0);
        if x < self.scroll_x + margin {
            self.scroll_x = (x - margin).max(0.0);
        } else if x > self.scroll_x + width - margin {
            self.scroll_x = x - width + margin;
        }
    }

    /// Keeps the cursor's row visible, counting rows rather than lines.
    fn scroll_to_cursor_wrapped(
        &mut self,
        p: &mut Painter,
        doc: &Rope,
        head: usize,
        line: usize,
        visible: f32,
    ) {
        self.scroll_x = 0.0;
        let mut ctx = self.line_ctx(&mut p.text);
        let span = doc.line_span(line);
        ctx.prepare(doc, span, None);
        let rows = ctx.rows();
        let row = LineCtx::row_of(&rows, ctx.s.display.to_display(head - span.start));
        let n = rows.len() as f32;
        let at = line as f32 + row as f32 / n;
        let center = std::mem::take(&mut self.center);
        if !center && at >= self.scroll_line {
            // Visible if the cursor's row is fewer than `visible` rows below the top.
            let (first, first_row, _) = self.top(&mut p.text, doc);
            let mut count = 0usize;
            let mut ctx = self.line_ctx(&mut p.text);
            for (i, span) in (first..=line).zip(doc.lines_from(first)) {
                ctx.prepare(doc, span, None);
                let n = if i == line { row + 1 } else { ctx.rows().len() };
                count += n - if i == first { first_row.min(n) } else { 0 };
                if count as f32 > visible {
                    break;
                }
            }
            if count as f32 <= visible {
                return;
            }
        }
        // Put the cursor's row `back` rows below the top.
        let back = if center {
            (visible / 2.0).floor()
        } else if at < self.scroll_line {
            0.0
        } else {
            visible - 1.0
        };
        self.scroll_line = at;
        self.scroll_rows(&mut p.text, -back);
    }

    pub fn draw(&mut self, p: &mut Painter, area: Rect, now: Instant) {
        self.update_geometry(p, area);
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

        // The visible lines and their rows, so matches can be found once for the range.
        let mut visible: Vec<(LineSpan, usize)> = Vec::new();
        {
            let mut rows_left = ((g.text.bottom() - y0) / g.line_h + 1).max(0) as usize;
            let mut ctx = self.line_ctx(&mut p.text);
            for (i, span) in (first..).zip(doc.lines_from(first)) {
                if rows_left == 0 {
                    break;
                }
                let n = if ctx.wrap.is_some() {
                    ctx.prepare(&doc, span, None);
                    ctx.rows().len()
                } else {
                    1
                };
                let shown = n - if i == first { first_row.min(n - 1) } else { 0 };
                rows_left = rows_left.saturating_sub(shown);
                visible.push((span, n));
            }
        }
        self.visible_lines = first..first + visible.len();
        let matches = match (&self.search, visible.first(), visible.last()) {
            (Some(s), Some((a, _)), Some((b, _))) => {
                s.matches_in(&doc, a.start..b.next.max(b.end + 1).min(doc.len()))
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
        let caret_h = ((m.ascent + m.descent) * 1.15).round() as i32;
        let gap = m.space_advance * 0.5;
        let preedit = self.preedit.clone();
        let wrap = self.wrap_width();
        let mut max_width = 0.0f32;
        let mut row_layout = LineLayout::default();
        self.caret = None;

        p.list.push_clip(area);
        let mut y = y0;
        for (i, (span, _)) in (first..).zip(visible) {
            let composing = preedit
                .as_ref()
                .filter(|_| i == cursor_line)
                .map(|(s, _)| (primary.head, s.as_str()));
            let mut ctx = LineCtx {
                text: &mut p.text,
                style,
                s: &mut self.scratch,
                wrap,
            };
            ctx.prepare(&doc, span, composing);
            let rows = ctx.rows();
            max_width = max_width.max(self.scratch.layout.width);
            // Syntax colors as display ranges, in order.
            let colored: Vec<(usize, usize, fude_render::Color)> = self
                .highlights
                .as_ref()
                .and_then(|h| h.line(i))
                .unwrap_or(&[])
                .iter()
                .map(|s| {
                    let len = span.end - span.start;
                    let d = &self.scratch.display;
                    (
                        d.to_display((s.start as usize).min(len)),
                        d.to_display((s.end as usize).min(len)),
                        t.style(s.style),
                    )
                })
                .collect();
            let skip = if i == first {
                first_row.min(rows.len() - 1)
            } else {
                0
            };
            let line_top = y;
            let line_rows = (rows.len() - skip) as i32;
            if i == cursor_line && primary.is_empty() {
                p.rect(
                    Rect::new(area.x, y, area.w, g.line_h * line_rows),
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
                    .filter(|m| m.start <= span.end && m.end > span.start)
                {
                    let a = ctx
                        .s
                        .display
                        .to_display(m.start.max(span.start) - span.start);
                    let b = ctx.s.display.to_display(m.end.min(span.end) - span.start);
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
                for s in sels.iter().filter(|s| !s.is_empty()) {
                    if s.end() < span.start
                        || s.start() > span.end
                        || s.end() == span.start && span.start != span.end
                    {
                        continue;
                    }
                    let a = ctx
                        .s
                        .display
                        .to_display(s.start().max(span.start) - span.start);
                    let b = ctx.s.display.to_display(s.end().min(span.end) - span.start);
                    let Some((x0, mut x1)) = extent(a, b) else {
                        continue;
                    };
                    if b > row.end && !last_row {
                        x1 = origin + row.place(ctx.x_at_display(row.end));
                    }
                    if last_row && s.end() > span.end && span.next > span.end {
                        x1 += gap;
                    }
                    if x1 > x0 {
                        p.list.rect(
                            Rect::new(x0.round() as i32, y, (x1 - x0).round() as i32, g.line_h),
                            selection_color,
                        );
                    }
                }
                // The row's glyphs, moved to the row's start.
                row_layout.glyphs.clear();
                row_layout.glyphs.extend(
                    ctx.s
                        .layout
                        .glyphs
                        .iter()
                        .filter(|gl| {
                            (gl.cluster as usize) >= row.start && (gl.cluster as usize) < row.end
                        })
                        .map(|gl| fude_text::PositionedGlyph {
                            x: row.place(gl.x),
                            ..*gl
                        }),
                );
                let visible_x = (g.text.x as f32, g.text.right() as f32);
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
                for s in sels.iter() {
                    let on_line = s.head >= span.start
                        && (s.head < span.next || (s.head == span.end && span.next == span.end));
                    if !on_line {
                        continue;
                    }
                    let ctx = LineCtx {
                        text: &mut p.text,
                        style,
                        s: &mut self.scratch,
                        wrap,
                    };
                    let mut display = ctx.s.display.to_display(s.head - span.start);
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
            if skip == 0 {
                let baseline = p.center_baseline(line_top, g.line_h, &style);
                let active = sels.iter().any(|s| doc.byte_to_line(s.head) == i);
                let number = (i + 1).to_string();
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

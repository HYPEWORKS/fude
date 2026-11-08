//! The editor view: draws the visible part of a document and turns keys, mouse and IME
//! input into editor commands.
//!
//! Only visible lines are ever laid out, so a frame costs the same at any file size.
//! The caret blinks for a while after input, then stays solid, so an idle window wakes
//! no timers at all.

use std::ops::Range;
use std::time::{Duration, Instant};

use fude_buffer::{LineSpan, Rope};
use fude_editor::{Editor, Layout, Motion, Selection, Selections, nearest, text as nav};
use fude_platform::{CursorShape, Ime, Key, KeyInput, Modifiers, ScrollUnit};
use fude_render::Rect;
use fude_text::{Chip, FontId, LineLayout, TextStyle, TextSystem};

use crate::display::DisplayLine;
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

/// Lays out single lines for drawing, hit testing and vertical movement.
struct LineCtx<'a> {
    text: &'a mut TextSystem,
    style: TextStyle,
    s: &'a mut Scratch,
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

    /// x of an offset relative to the prepared line's start.
    fn x_at(&self, rel: usize) -> f32 {
        self.x_at_display(self.s.display.to_display(rel))
    }

    /// Cluster boundaries of the prepared line, as (document offset, x).
    fn stops(&self, line_start: usize) -> Vec<(usize, f32)> {
        let mut stops = Vec::with_capacity(self.s.layout.glyphs.len() + 1);
        let mut last = None;
        for g in &self.s.layout.glyphs {
            if last != Some(g.cluster) {
                last = Some(g.cluster);
                stops.push((line_start + self.s.display.to_doc(g.cluster as usize), g.x));
            }
        }
        stops.push((line_start + self.s.bytes.len(), self.s.layout.width));
        stops
    }
}

impl Layout for LineCtx<'_> {
    fn x_of(&mut self, doc: &Rope, line: usize, at: usize) -> f32 {
        let span = doc.line_span(line);
        self.prepare(doc, span, None);
        self.x_at(at - span.start)
    }

    fn offset_at(&mut self, doc: &Rope, line: usize, x: f32) -> usize {
        let span = doc.line_span(line);
        self.prepare(doc, span, None);
        nearest(&self.stops(span.start), x)
    }
}

pub struct EditorView {
    pub editor: Editor,
    /// Logical font size; zoom changes it.
    font_size: f32,
    /// First visible line, fractional for smooth scrolling.
    scroll_line: f32,
    /// Horizontal scroll, physical pixels.
    scroll_x: f32,
    /// Scroll the primary cursor into view at the next draw.
    reveal: bool,
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
            scroll_line: 0.0,
            scroll_x: 0.0,
            reveal: false,
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
        TextStyle::new(FontId::MONO, self.geo.px)
    }

    fn line_ctx<'a>(&'a mut self, text: &'a mut TextSystem) -> LineCtx<'a> {
        LineCtx {
            text,
            style: TextStyle::new(FontId::MONO, self.geo.px),
            s: &mut self.scratch,
        }
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
            let lines = dy / self.geo.line_h as f32 * 0.5;
            self.scroll_line = (self.scroll_line + lines).clamp(0.0, self.max_scroll());
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
                Motion::PageUp(n) => self.scroll_lines(-(n as f32)),
                Motion::PageDown(n) => self.scroll_lines(n as f32),
                _ => {}
            }
            let mut ctx = LineCtx {
                text: &mut p.text,
                style: TextStyle::new(FontId::MONO, self.geo.px),
                s: &mut self.scratch,
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
            Key::Tab if !m.primary() && !m.shift && !m.alt => self.editor.insert("\t", now),
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
    pub fn scroll(&mut self, dx: f32, dy: f32, unit: ScrollUnit, modifiers: Modifiers) {
        let (dx, dy) = if modifiers.shift && dx == 0.0 {
            (dy, 0.0)
        } else {
            (dx, dy)
        };
        let column = self.scratch_space_advance();
        match unit {
            ScrollUnit::Lines => {
                self.scroll_lines(dy * 3.0);
                self.scroll_px(dx * 3.0 * column * 2.0);
            }
            ScrollUnit::Pixels => {
                self.scroll_lines(dy / self.geo.line_h as f32);
                self.scroll_px(dx);
            }
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

    pub fn zoom(&mut self, delta: f32) {
        self.font_size = (self.font_size + delta).clamp(6.0, 72.0);
        self.reveal = true;
    }

    pub fn reset_zoom(&mut self) {
        self.font_size = DEFAULT_FONT_SIZE;
        self.reveal = true;
    }

    /// Call after changing the editor from outside (undo, paste, select all).
    pub fn changed(&mut self, now: Instant) {
        self.touched(now);
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
                    self.scroll_lines(if yi < thumb.y { -page } else { page });
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

    /// The document offset under a point.
    fn hit(&mut self, p: &mut Painter, x: f32, y: f32) -> usize {
        let g = self.geo;
        let doc = self.editor.text().clone();
        let row = (y - (g.text.y + g.pad_top) as f32) / g.line_h as f32 + self.scroll_line;
        let last = doc.line_count() - 1;
        if row < 0.0 {
            return 0;
        }
        let line = (row.floor() as usize).min(last);
        if row.floor() as usize > last {
            return doc.len();
        }
        let lx = x - (g.text.x + g.text_pad) as f32 + self.scroll_x;
        self.line_ctx(&mut p.text).offset_at(&doc, line, lx)
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
        if (line as f32) < self.scroll_line {
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

    pub fn draw(&mut self, p: &mut Painter, area: Rect, now: Instant) {
        self.update_geometry(p, area);
        if std::mem::take(&mut self.reveal) {
            self.scroll_to_cursor(p);
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
        let first = (self.scroll_line.floor() as usize).min(doc.line_count() - 1);
        let frac = self.scroll_line - first as f32;
        let y0 = g.text.y + g.pad_top - (frac * g.line_h as f32).round() as i32;
        let rows = ((g.text.bottom() - y0) / g.line_h + 1).max(0) as usize;
        let spans: Vec<LineSpan> = doc.lines_from(first).take(rows).collect();
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
        let mut max_width = 0.0f32;
        self.caret = None;

        p.list.push_clip(area);
        for (i, span) in (first..).zip(spans) {
            let y = y0 + (i - first) as i32 * g.line_h;
            let baseline = p.center_baseline(y, g.line_h, &style);
            if i == cursor_line && primary.is_empty() {
                p.rect(Rect::new(area.x, y, area.w, g.line_h), t.line_highlight);
            }
            let composing = preedit
                .as_ref()
                .filter(|_| i == cursor_line)
                .map(|(s, _)| (primary.head, s.as_str()));
            let mut ctx = LineCtx {
                text: &mut p.text,
                style,
                s: &mut self.scratch,
            };
            ctx.prepare(&doc, span, composing);
            max_width = max_width.max(self.scratch.layout.width);

            p.list.push_clip(g.text);
            // Selections, extended a little past the line end when they include the break.
            for s in sels.iter().filter(|s| !s.is_empty()) {
                if s.end() < span.start
                    || s.start() > span.end
                    || s.end() == span.start && span.start != span.end
                {
                    continue;
                }
                let ctx = LineCtx {
                    text: &mut p.text,
                    style,
                    s: &mut self.scratch,
                };
                let a = s.start().max(span.start) - span.start;
                let b = s.end().min(span.end) - span.start;
                let x0 = origin + ctx.x_at(a);
                let mut x1 = origin + ctx.x_at(b);
                if s.end() > span.end && span.next > span.end {
                    x1 += gap;
                }
                p.rect(
                    Rect::new(x0.round() as i32, y, (x1 - x0).round() as i32, g.line_h),
                    selection_color,
                );
            }
            let visible_x = (g.text.x as f32, g.text.right() as f32);
            p.text.draw_line(
                &self.scratch.layout,
                &style,
                origin,
                baseline,
                t.text,
                visible_x,
                &mut p.list,
                &mut p.images,
            );
            self.draw_chips(p, &style, origin, y, g.line_h);
            if let Some(range) = self.scratch.display.preedit.clone() {
                let ctx = LineCtx {
                    text: &mut p.text,
                    style,
                    s: &mut self.scratch,
                };
                let (x0, x1) = (
                    origin + ctx.x_at_display(range.start),
                    origin + ctx.x_at_display(range.end),
                );
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
            // Carets.
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
                };
                let mut display = ctx.s.display.to_display(s.head - span.start);
                if *s == primary
                    && let (Some(range), Some((_, Some((c, _))))) =
                        (&ctx.s.display.preedit, &preedit)
                {
                    display = range.start + c;
                }
                let x = (origin + ctx.x_at_display(display)).round() as i32;
                let rect = Rect::new(x, y + (g.line_h - caret_h) / 2, caret_w, caret_h);
                if *s == primary {
                    self.caret = Some(rect);
                }
                if caret_on {
                    p.rect(rect, t.caret);
                }
            }
            p.list.pop_clip();

            // Line number.
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

    fn draw_chips(&mut self, p: &mut Painter, style: &TextStyle, origin: f32, y: i32, line_h: i32) {
        let t = p.theme;
        let chips: Vec<(f32, Chip)> = self
            .scratch
            .layout
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

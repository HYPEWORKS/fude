//! Read-only demo view, laid out like the real editor (tab strip, gutter, text, status
//! bar). It draws straight from a `fude-buffer` rope, touching only the visible lines.
//! The real editor view arrives in phase 2 with `fude-editor`.

use std::time::{Duration, Instant};

use fude_buffer::{Format, LineSpan, Rope};
use fude_render::{Canvas, DrawList, ImageStore, Rect, rasterize};
use fude_text::{FontId, LineLayout, LineMetrics, TextStyle, TextSystem};

use crate::theme::{SUMI, Theme};

/// Lines longer than this are cut off for display (long-line layout is phase 4 work).
const MAX_DISPLAY_BYTES: usize = 4096;

pub struct View {
    title: String,
    doc: Rope,
    format: Format,
    /// Raw bytes of the line being drawn.
    line_buf: Vec<u8>,
    text: TextSystem,
    images: ImageStore,
    list: DrawList,
    scratch: LineLayout,
    theme: &'static Theme,
    /// Logical font size; zoom changes it.
    font_size: f32,
    /// Scroll position in lines (fractional for smooth pixel scrolling).
    scroll: f32,
    /// Height of the text area in lines, from the last frame.
    page_lines: f32,
    last_frame: Duration,
}

pub const DEFAULT_FONT_SIZE: f32 = 14.0;

impl View {
    pub fn new(title: String, doc: Rope, format: Format) -> View {
        View {
            title,
            doc,
            format,
            line_buf: Vec::new(),
            text: TextSystem::new(),
            images: ImageStore::new(),
            list: DrawList::new(),
            scratch: LineLayout::default(),
            theme: &SUMI,
            font_size: DEFAULT_FONT_SIZE,
            scroll: 0.0,
            page_lines: 30.0,
            last_frame: Duration::ZERO,
        }
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn last_frame(&self) -> Duration {
        self.last_frame
    }

    pub fn line_count(&self) -> usize {
        self.doc.line_count()
    }

    /// Scrolls by `lines`, clamped to the document.
    pub fn scroll_by(&mut self, lines: f32) {
        let max = (self.line_count() as f32 - 1.0).max(0.0);
        self.scroll = (self.scroll + lines).clamp(0.0, max);
    }

    pub fn scroll_to(&mut self, line: f32) {
        self.scroll = 0.0;
        self.scroll_by(line);
    }

    pub fn page_lines(&self) -> f32 {
        self.page_lines
    }

    /// Scroll amount in lines for a pixel delta at `scale`.
    pub fn pixels_to_lines(&self, px: f32, scale: f32) -> f32 {
        px / self.line_height(scale)
    }

    pub fn zoom(&mut self, delta: f32) {
        self.font_size = (self.font_size + delta).clamp(6.0, 72.0);
    }

    pub fn reset_zoom(&mut self) {
        self.font_size = DEFAULT_FONT_SIZE;
    }

    fn style(&self, scale: f32) -> TextStyle {
        TextStyle::new(FontId::MONO, (self.font_size * scale).round())
    }

    fn line_height(&self, scale: f32) -> f32 {
        (self.style(scale).px * 1.5).round()
    }

    pub fn draw(&mut self, canvas: &mut Canvas<'_>, scale: f32) {
        let start = Instant::now();
        self.list.clear();
        let (w, h) = (canvas.width() as i32, canvas.height() as i32);
        let t = self.theme;
        let px = |v: f32| (v * scale).round() as i32;

        let style = self.style(scale);
        let m = self.text.metrics(style.font, style.px);
        let line_h = self.line_height(scale) as i32;
        let chrome_style = TextStyle::new(FontId::MONO, (13.0 * scale).round());
        let cm = self.text.metrics(chrome_style.font, chrome_style.px);

        // Tab strip.
        let tab_h = px(36.0);
        self.list.rect(Rect::new(0, 0, w, tab_h), t.chrome_bg);
        let title = self.title.clone();
        let tab_pad = px(16.0);
        let title_w = self.measure(&title, &chrome_style);
        let tab_w = (title_w.ceil() as i32 + tab_pad * 2).max(px(120.0));
        self.list.rect(Rect::new(0, 0, tab_w, tab_h), t.editor_bg);
        self.list.rect(Rect::new(0, 0, tab_w, px(2.0)), t.accent);
        self.list.rect(
            Rect::new(tab_w, tab_h - px(1.0), w - tab_w, px(1.0)),
            t.border,
        );
        let tab_baseline = center_baseline(0, tab_h, &cm);
        self.text_at(
            &title,
            &chrome_style,
            tab_pad as f32,
            tab_baseline,
            t.chrome_text,
            w,
        );

        // Status bar.
        let status_h = px(26.0);
        let status_y = h - status_h;
        self.list
            .rect(Rect::new(0, status_y, w, status_h), t.chrome_bg);
        self.list.rect(Rect::new(0, status_y, w, px(1.0)), t.border);
        let status_baseline = center_baseline(status_y, status_h, &cm);
        let line_no = self.scroll.floor() as usize + 1;
        let left = format!("Ln {line_no}, Col 1");
        self.text_at(
            &left,
            &chrome_style,
            tab_pad as f32,
            status_baseline,
            t.chrome_text,
            w,
        );
        let line_endings = if self.doc.line_ending_counts().is_mixed() {
            "Mixed"
        } else {
            self.format.line_ending.name()
        };
        let right = format!(
            "{} lines · {}   {}   {line_endings}   Plain Text   {:.2} ms",
            self.line_count(),
            human_bytes(self.doc.len()),
            self.format.encoding_label(),
            self.last_frame.as_secs_f64() * 1000.0
        );
        let right_w = self.measure(&right, &chrome_style);
        let right_x = w as f32 - tab_pad as f32 - right_w;
        self.text_at(
            &right,
            &chrome_style,
            right_x,
            status_baseline,
            t.chrome_text_muted,
            w,
        );

        // Editor area.
        let area = Rect::new(0, tab_h, w, (status_y - tab_h).max(0));
        self.list.rect(area, t.editor_bg);
        self.page_lines = area.h as f32 / line_h as f32;
        let digits = self.line_count().ilog10() as f32 + 1.0;
        let gutter_w = ((digits.max(3.0)) * m.space_advance).ceil() as i32 + px(24.0);
        let text_x = (gutter_w + px(8.0)) as f32;
        let top_pad = px(6.0);

        let first = self.scroll.floor() as usize;
        let frac = self.scroll - first as f32;
        let y0 = area.y + top_pad - (frac * line_h as f32).round() as i32;
        let cursor_line = first; // the demo cursor sits on the top visible line

        self.list.push_clip(area);
        let visible = ((area.bottom() - y0) / line_h.max(1) + 1).max(0) as usize;
        let spans: Vec<LineSpan> = self.doc.lines_from(first).take(visible).collect();
        let mut y = y0;
        for (i, span) in (first..).zip(spans) {
            let baseline = center_baseline(y, line_h, &m);
            if i == cursor_line {
                self.list.rect(Rect::new(0, y, w, line_h), t.line_highlight);
            }
            let num = (i + 1).to_string();
            let num_w = self.measure(&num, &style);
            let num_color = if i == cursor_line {
                t.gutter_text_active
            } else {
                t.gutter_text
            };
            let num_x = (gutter_w - px(12.0)) as f32 - num_w;
            self.text_at(&num, &style, num_x, baseline, num_color, w);

            // Invalid UTF-8 shows as U+FFFD for now; phase 2 draws it as byte chips.
            let mut raw = std::mem::take(&mut self.line_buf);
            raw.clear();
            let end = span.end.min(span.start + MAX_DISPLAY_BYTES);
            self.doc.copy_range(span.start..end, &mut raw);
            if end < span.end {
                raw.truncate(complete_utf8_prefix(&raw));
            }
            let shown = String::from_utf8_lossy(&raw);
            self.list
                .push_clip(Rect::new(gutter_w, area.y, w - gutter_w, area.h));
            self.text_at(&shown, &style, text_x, baseline, t.text, w);
            self.list.pop_clip();
            drop(shown);
            self.line_buf = raw;

            if i == cursor_line {
                let ch = (m.ascent + m.descent).round() as i32;
                self.list.rect(
                    Rect::new(
                        text_x as i32,
                        baseline - m.ascent.round() as i32,
                        px(2.0),
                        ch,
                    ),
                    t.accent,
                );
            }
            y += line_h;
        }
        self.list.pop_clip();

        rasterize(&self.list, &self.images, canvas, canvas.bounds());
        self.last_frame = start.elapsed();
    }

    fn measure(&mut self, s: &str, style: &TextStyle) -> f32 {
        let mut scratch = std::mem::take(&mut self.scratch);
        self.text.layout_line(s, style, &mut scratch);
        let w = scratch.width;
        self.scratch = scratch;
        w
    }

    fn text_at(
        &mut self,
        s: &str,
        style: &TextStyle,
        x: f32,
        baseline: i32,
        color: fude_render::Color,
        w: i32,
    ) {
        let mut scratch = std::mem::take(&mut self.scratch);
        self.text.layout_line(s, style, &mut scratch);
        self.text.draw_line(
            &scratch,
            style,
            x,
            baseline,
            color,
            (0.0, w as f32),
            &mut self.list,
            &mut self.images,
        );
        self.scratch = scratch;
    }

    /// Glyph cache and image store sizes, for diagnostics.
    pub fn cache_stats(&self) -> (usize, usize) {
        (self.text.cached_glyphs(), self.images.bytes())
    }
}

/// Baseline that vertically centers a line box of metrics `m` in `height` pixels at `top`.
fn center_baseline(top: i32, height: i32, m: &LineMetrics) -> i32 {
    let content = m.ascent + m.descent;
    top + ((height as f32 - content) / 2.0 + m.ascent).round() as i32
}

/// `bytes` without a UTF-8 sequence cut off at the end.
fn complete_utf8_prefix(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        _ => bytes.len(),
    }
}

fn human_bytes(n: usize) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub const WELCOME: &str = "\
fude 筆: Fast, Uncomplicated Document Editor
===========================================

This is the phase 1 build: a window, a CPU renderer, the text stack, and the
buffer (a rope with snapshots and undo, encoding detection, safe saving).
There is no editing yet: this view draws straight from the rope, read-only.

  Scroll           mouse wheel, trackpad, arrows, PageUp / PageDown, Home / End
  Zoom             Ctrl+= / Ctrl+- / Ctrl+0  (Cmd on macOS)
  Quit             Ctrl+Q  (Cmd+Q on macOS)

Open a file:  fude path/to/file.txt

Unicode:      café naïve Ærøskøbing  “quotes” • bullets → arrows ≠ ≤ ≥ ∞
Box drawing:  ┌──────┬──────┐
              │ sumi │ shu  │
              └──────┴──────┘
CJK:          筆  (shows as boxes until font fallback lands in phase 2)

fn main() {
\tlet greeting = \"Hello, fude!\";
\tfor (i, word) in greeting.split(' ').enumerate() {
\t\tprintln!(\"{i}: {word}\");   // tabs: 4 columns
\t}
}
";

/// Welcome text plus numbered filler lines so there is something to scroll.
pub fn welcome_text() -> String {
    let mut s = WELCOME.to_owned();
    for i in 1..=500 {
        s.push_str(&format!(
            "{i:>4}  The quick brown fox jumps over the lazy dog. 0123456789 ()[]{{}}<>!=->\n"
        ));
    }
    s
}

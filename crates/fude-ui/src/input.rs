//! A single-line text field, built on the same editor as documents, so it gets selection,
//! word motions, undo, the clipboard and IME like everything else.

use std::time::{Duration, Instant};

use fude_buffer::{Format, Rope};
use fude_editor::{Editor, MonoLayout, Motion, Selection, Selections, nearest, text as nav};
use fude_platform::{Ime, Key, KeyInput, Modifiers};
use fude_render::{Color, Rect};
use fude_text::{LineLayout, TextStyle};

use crate::painter::Painter;

const BLINK: Duration = Duration::from_millis(530);
const BLINK_FOR: Duration = Duration::from_secs(10);

/// What a key did to the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKey {
    /// The text changed.
    Changed,
    /// Only the cursor or selection moved.
    Moved,
    Ignored,
}

/// What an input method event did to a text field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImeChange {
    /// Nothing: input methods send empty updates freely (KWin answers every caret move
    /// with one).
    Nothing,
    /// The composition shown at the caret changed.
    Preedit,
    /// Text was committed into the field.
    Text,
}

pub struct TextInput {
    pub editor: Editor,
    scroll_x: f32,
    focused: bool,
    blink_start: Instant,
    /// Text rectangle from the last draw.
    rect: Rect,
    style: TextStyle,
    layout: LineLayout,
    preedit: Option<String>,
    dragging: bool,
    caret: Option<Rect>,
}

impl Default for TextInput {
    fn default() -> TextInput {
        TextInput::new()
    }
}

impl TextInput {
    pub fn new() -> TextInput {
        TextInput {
            editor: Editor::new(Rope::new(), Format::default()),
            scroll_x: 0.0,
            focused: false,
            blink_start: Instant::now(),
            rect: Rect::default(),
            style: TextStyle::new(fude_text::FontId::MONO, 13.0),
            layout: LineLayout::default(),
            preedit: None,
            dragging: false,
            caret: None,
        }
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.editor.text().to_vec()).into_owned()
    }

    /// Replaces the text (one undo step) and selects all of it.
    pub fn set_text(&mut self, text: &str) {
        let len = self.editor.text().len();
        self.editor
            .apply_edits(&[(0..len, text.as_bytes().to_vec())]);
        self.editor.select_all();
    }

    pub fn select_all(&mut self) {
        self.editor.select_all();
    }

    pub fn set_focused(&mut self, focused: bool, now: Instant) {
        self.focused = focused;
        self.blink_start = now;
        if !focused {
            self.preedit = None;
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let r = self.rect;
        Rect::new(r.x - 4, r.y - 4, r.w + 8, r.h + 8).contains(x as i32, y as i32)
    }

    pub fn caret_rect(&self) -> Option<Rect> {
        self.caret
    }

    pub fn next_timer(&self, now: Instant) -> Option<Instant> {
        if !self.focused {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.blink_start);
        if elapsed >= BLINK_FOR {
            return None;
        }
        let ticks = (elapsed.as_millis() / BLINK.as_millis()) as u32 + 1;
        Some(self.blink_start + BLINK * ticks)
    }

    fn touched(&mut self, now: Instant) {
        self.blink_start = now;
    }

    /// Typing and editing keys. Line breaks are never inserted.
    pub fn key(&mut self, key: &KeyInput, now: Instant) -> InputKey {
        if !key.pressed {
            return InputKey::Ignored;
        }
        let m = key.modifiers;
        let mac = cfg!(target_os = "macos");
        let word = if mac { m.alt } else { m.ctrl };
        let motion = match key.key {
            Key::Left if mac && m.logo => Some(Motion::DocStart),
            Key::Right if mac && m.logo => Some(Motion::DocEnd),
            Key::Left if word => Some(Motion::WordLeft),
            Key::Right if word => Some(Motion::WordRight),
            Key::Left => Some(Motion::Left),
            Key::Right => Some(Motion::Right),
            Key::Home => Some(Motion::DocStart),
            Key::End => Some(Motion::DocEnd),
            _ => None,
        };
        if let Some(motion) = motion {
            self.editor
                .move_cursors(motion, m.shift, &mut MonoLayout { tab_width: 4 });
            self.touched(now);
            return InputKey::Moved;
        }
        let before = self.editor.buffer().version();
        match key.key {
            Key::Backspace if word => self.editor.delete_word_left(now),
            Key::Backspace => self.editor.backspace(now),
            Key::Delete if word => self.editor.delete_word_right(now),
            Key::Delete => self.editor.delete(now),
            _ => {
                let Some(text) = &key.text else {
                    return InputKey::Ignored;
                };
                let shortcut = if mac { m.logo } else { m.ctrl && !m.alt };
                if shortcut || text.is_empty() || text.chars().any(char::is_control) {
                    return InputKey::Ignored;
                }
                self.editor.insert(text, now);
            }
        }
        self.touched(now);
        if self.editor.buffer().version() != before {
            InputKey::Changed
        } else {
            InputKey::Moved
        }
    }

    pub fn ime(&mut self, ime: Ime, now: Instant) -> ImeChange {
        match ime {
            Ime::Preedit(text, _) if !text.is_empty() => {
                if self.preedit.as_ref() == Some(&text) {
                    return ImeChange::Nothing;
                }
                self.preedit = Some(text);
                ImeChange::Preedit
            }
            Ime::Commit(text) if !text.is_empty() => {
                self.preedit = None;
                self.paste(&text, now);
                ImeChange::Text
            }
            _ if self.preedit.take().is_some() => ImeChange::Preedit,
            _ => ImeChange::Nothing,
        }
    }

    /// Inserts text, with line breaks turned into spaces.
    pub fn paste(&mut self, text: &str, now: Instant) {
        let single: String = text
            .trim_end_matches(['\r', '\n'])
            .chars()
            .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
            .collect();
        self.editor.insert(&single, now);
        self.touched(now);
    }

    // --- Mouse -------------------------------------------------------------------------

    fn offset_at(&mut self, p: &mut Painter, x: f32) -> usize {
        let text = self.text();
        p.text.layout_line(&text, &self.style, &mut self.layout);
        let mut stops: Vec<(usize, f32)> = Vec::new();
        let mut last = None;
        for g in &self.layout.glyphs {
            if last != Some(g.cluster) {
                last = Some(g.cluster);
                stops.push((g.cluster as usize, g.x));
            }
        }
        stops.push((text.len(), self.layout.width));
        nearest(&stops, x - self.rect.x as f32 + self.scroll_x)
    }

    pub fn mouse_down(
        &mut self,
        p: &mut Painter,
        x: f32,
        clicks: u8,
        modifiers: Modifiers,
        now: Instant,
    ) {
        let at = self.offset_at(p, x);
        let text = self.editor.text().clone();
        let selection = match clicks {
            2 => {
                let r = nav::word_range(&text, at);
                Selection::new(r.start, r.end)
            }
            3 => Selection::new(0, text.len()),
            _ if modifiers.shift => Selection::new(self.editor.selections().primary().anchor, at),
            _ => Selection::cursor(at),
        };
        self.editor.set_selections(Selections::single(selection));
        self.dragging = clicks == 1;
        self.touched(now);
    }

    pub fn mouse_move(&mut self, p: &mut Painter, x: f32) -> bool {
        if !self.dragging {
            return false;
        }
        let at = self.offset_at(p, x);
        let anchor = self.editor.selections().primary().anchor;
        self.editor
            .set_selections(Selections::single(Selection::new(anchor, at)));
        true
    }

    pub fn mouse_up(&mut self) {
        self.dragging = false;
    }

    // --- Drawing -----------------------------------------------------------------------

    /// Draws the field in `rect`: a box, the text (or `placeholder`), selection and caret.
    /// `error` outlines it in the error color.
    pub fn draw(
        &mut self,
        p: &mut Painter,
        rect: Rect,
        now: Instant,
        placeholder: &str,
        error: bool,
    ) {
        let t = p.theme;
        let border = if error {
            t.error
        } else if self.focused {
            t.accent
        } else {
            t.border
        };
        p.rect(rect, border);
        let inner = Rect::new(rect.x + 1, rect.y + 1, rect.w - 2, rect.h - 2);
        p.rect(inner, t.editor_bg);
        let pad = p.px(6.0);
        let text_rect = Rect::new(inner.x + pad, inner.y, (inner.w - 2 * pad).max(0), inner.h);
        self.rect = text_rect;
        self.style = p.style(13.0);
        let style = self.style;
        let baseline = p.center_baseline(text_rect.y, text_rect.h, &style);

        let text = self.text();
        let head = self.editor.selections().primary().head;
        let shown = match &self.preedit {
            Some(pre) => format!("{}{pre}{}", &text[..head], &text[head..]),
            None => text.clone(),
        };
        p.text.layout_line(&shown, &style, &mut self.layout);
        let x_at = |layout: &LineLayout, at: usize| {
            layout
                .glyphs
                .iter()
                .find(|g| g.cluster as usize >= at)
                .map_or(layout.width, |g| g.x)
        };
        let preedit_len = self.preedit.as_ref().map_or(0, String::len);
        let caret_x = x_at(&self.layout, head + preedit_len);
        // Keep the caret in view.
        let w = text_rect.w as f32;
        if caret_x - self.scroll_x > w {
            self.scroll_x = caret_x - w;
        } else if caret_x < self.scroll_x {
            self.scroll_x = caret_x;
        }
        self.scroll_x = self.scroll_x.min((self.layout.width - w).max(0.0)).max(0.0);
        let origin = text_rect.x as f32 - self.scroll_x;

        p.list.push_clip(text_rect);
        let sel = *self.editor.selections().primary();
        if !sel.is_empty() && self.preedit.is_none() {
            let (a, b) = (
                x_at(&self.layout, sel.start()),
                x_at(&self.layout, sel.end()),
            );
            let color = if self.focused {
                t.selection
            } else {
                t.selection_inactive
            };
            p.rect(
                Rect::new(
                    (origin + a) as i32,
                    text_rect.y + p.px(3.0),
                    (b - a).ceil() as i32,
                    text_rect.h - p.px(6.0),
                ),
                color,
            );
        }
        if shown.is_empty() {
            p.text(
                placeholder,
                &style,
                text_rect.x as f32,
                baseline,
                t.chrome_text_muted,
            );
        } else {
            let layout = std::mem::take(&mut self.layout);
            p.text.draw_line(
                &layout,
                &style,
                origin,
                baseline,
                t.text,
                (text_rect.x as f32, text_rect.right() as f32),
                &mut p.list,
                &mut p.images,
            );
            if self.preedit.is_some() {
                let (a, b) = (x_at(&layout, head), x_at(&layout, head + preedit_len));
                p.rect(
                    Rect::new(
                        (origin + a) as i32,
                        baseline + p.px(2.0),
                        (b - a) as i32,
                        p.px(1.0).max(1),
                    ),
                    t.preedit,
                );
            }
            self.layout = layout;
        }
        let m = p.metrics(&style);
        let h = ((m.ascent + m.descent) * 1.1).round() as i32;
        let caret = Rect::new(
            (origin + caret_x).round() as i32,
            text_rect.y + (text_rect.h - h) / 2,
            p.px(1.5).max(1),
            h,
        );
        self.caret = Some(caret);
        let elapsed = now.saturating_duration_since(self.blink_start);
        let on =
            elapsed >= BLINK_FOR || (elapsed.as_millis() / BLINK.as_millis()).is_multiple_of(2);
        if self.focused && on {
            p.rect(caret, t.caret);
        }
        p.list.pop_clip();
    }
}

/// A small labeled button. `on` draws it as active (toggles).
pub(crate) fn button(
    p: &mut Painter,
    rect: Rect,
    label: &str,
    on: bool,
    hover: bool,
    primary: bool,
) {
    let t = p.theme;
    let (bg, fg): (Color, Color) = if primary {
        (t.accent, t.accent_text)
    } else if on {
        (t.selection, t.text)
    } else {
        (t.button_bg, t.button_text)
    };
    p.rect(rect, bg);
    if hover {
        p.rect(rect, t.scrollbar);
    }
    if on {
        p.rect(
            Rect::new(rect.x, rect.bottom() - p.px(2.0), rect.w, p.px(2.0)),
            t.accent,
        );
    }
    let style = p.style(12.0);
    let w = p.measure(label, &style);
    let baseline = p.center_baseline(rect.y, rect.h, &style);
    p.text(
        label,
        &style,
        rect.x as f32 + (rect.w as f32 - w) / 2.0,
        baseline,
        fg,
    );
}

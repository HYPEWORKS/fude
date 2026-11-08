//! The find and replace bar, docked at the bottom of the editor.
//!
//! Keys: Enter / Shift+Enter (or F3 / Shift+F3) for the next and previous match, Escape
//! to close, Alt+C / Alt+W / Alt+R to toggle case, whole word and regex, Tab to move
//! between the fields, Enter in the replace field to replace, Ctrl+Alt+Enter to replace
//! all.

use std::time::Instant;

use fude_editor::Query;
use fude_platform::{Ime, Key, KeyInput, Modifiers};
use fude_render::Rect;

use crate::input::{ImeChange, InputKey, TextInput, button};
use crate::painter::Painter;

/// What the bar asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindAction {
    /// The query (text or options) changed: search again.
    QueryChanged,
    Next,
    Previous,
    ReplaceOne,
    ReplaceAll,
    Close,
    /// Only the bar changed (focus, cursor): redraw.
    Redraw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FindField {
    Find,
    Replace,
}

/// What the status area says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FindStatus {
    Empty,
    Counting,
    NoMatches,
    Error(String),
    /// `current` is the 1-based index of the selected match, when known.
    Count {
        current: Option<usize>,
        total: usize,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Button {
    ToggleReplace,
    Case,
    Word,
    Regex,
    Previous,
    Next,
    Close,
    Replace,
    ReplaceAll,
}

const ROW: f32 = 34.0;
const PAD: f32 = 6.0;

pub struct FindBar {
    pub find: TextInput,
    pub replace: TextInput,
    pub show_replace: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub focus: FindField,
    pub status: FindStatus,
    buttons: Vec<(Rect, Button)>,
    hover: Option<Button>,
    area: Rect,
}

impl Default for FindBar {
    fn default() -> FindBar {
        FindBar::new()
    }
}

impl FindBar {
    pub fn new() -> FindBar {
        FindBar {
            find: TextInput::new(),
            replace: TextInput::new(),
            show_replace: false,
            case_sensitive: false,
            whole_word: false,
            regex: false,
            focus: FindField::Find,
            status: FindStatus::Empty,
            buttons: Vec::new(),
            hover: None,
            area: Rect::default(),
        }
    }

    pub fn query(&self) -> Query {
        Query {
            text: self.find.text(),
            regex: self.regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
        }
    }

    pub fn replacement(&self) -> String {
        self.replace.text()
    }

    /// Logical height: one row, or two with the replace field.
    pub fn height(&self) -> f32 {
        let rows = if self.show_replace { 2.0 } else { 1.0 };
        rows * ROW + PAD
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x as i32, y as i32)
    }

    /// Opens (or refocuses) the bar on the find field, seeded with `seed` if given.
    pub fn open(&mut self, replace: bool, seed: Option<&str>, now: Instant) {
        self.show_replace |= replace;
        if let Some(seed) = seed {
            self.find.set_text(seed);
        }
        self.find.select_all();
        self.set_focus(FindField::Find, now);
    }

    pub fn set_focus(&mut self, field: FindField, now: Instant) {
        self.focus = field;
        self.find.set_focused(field == FindField::Find, now);
        self.replace.set_focused(field == FindField::Replace, now);
    }

    /// Unfocuses both fields (the editor has focus).
    pub fn blur(&mut self, now: Instant) {
        self.find.set_focused(false, now);
        self.replace.set_focused(false, now);
    }

    pub fn focused_input(&mut self) -> &mut TextInput {
        match self.focus {
            FindField::Find => &mut self.find,
            FindField::Replace => &mut self.replace,
        }
    }

    pub fn next_timer(&self, now: Instant) -> Option<Instant> {
        self.find.next_timer(now).or(self.replace.next_timer(now))
    }

    pub fn caret_rect(&self) -> Option<Rect> {
        match self.focus {
            FindField::Find => self.find.caret_rect(),
            FindField::Replace => self.replace.caret_rect(),
        }
    }

    pub fn key(&mut self, key: &KeyInput, now: Instant) -> Option<FindAction> {
        if !key.pressed {
            return None;
        }
        let m = key.modifiers;
        match &key.key {
            Key::Escape => return Some(FindAction::Close),
            Key::Enter if m.ctrl && m.alt => return Some(FindAction::ReplaceAll),
            Key::Enter if self.focus == FindField::Replace => return Some(FindAction::ReplaceOne),
            Key::Enter | Key::F(3) if m.shift => return Some(FindAction::Previous),
            Key::Enter | Key::F(3) => return Some(FindAction::Next),
            Key::Tab if self.show_replace => {
                let next = match self.focus {
                    FindField::Find => FindField::Replace,
                    FindField::Replace => FindField::Find,
                };
                self.set_focus(next, now);
                return Some(FindAction::Redraw);
            }
            Key::Character(c) if m.alt && !m.ctrl => {
                match c.to_lowercase().as_str() {
                    "c" => self.case_sensitive = !self.case_sensitive,
                    "w" => self.whole_word = !self.whole_word,
                    "r" => self.regex = !self.regex,
                    _ => return None,
                }
                return Some(FindAction::QueryChanged);
            }
            _ => {}
        }
        let field = self.focus;
        match self.focused_input().key(key, now) {
            InputKey::Changed if field == FindField::Find => Some(FindAction::QueryChanged),
            InputKey::Changed | InputKey::Moved => Some(FindAction::Redraw),
            InputKey::Ignored => None,
        }
    }

    pub fn ime(&mut self, ime: Ime, now: Instant) -> Option<FindAction> {
        let field = self.focus;
        match self.focused_input().ime(ime, now) {
            ImeChange::Nothing => None,
            ImeChange::Text if field == FindField::Find => Some(FindAction::QueryChanged),
            _ => Some(FindAction::Redraw),
        }
    }

    pub fn paste(&mut self, text: &str, now: Instant) -> FindAction {
        let field = self.focus;
        self.focused_input().paste(text, now);
        if field == FindField::Find {
            FindAction::QueryChanged
        } else {
            FindAction::Redraw
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
    ) -> Option<FindAction> {
        if !self.contains(x, y) {
            return None;
        }
        let hit = self
            .buttons
            .iter()
            .find(|(r, _)| r.contains(x as i32, y as i32))
            .map(|&(_, b)| b);
        if let Some(b) = hit {
            return Some(match b {
                Button::ToggleReplace => {
                    self.show_replace = !self.show_replace;
                    let field = if self.show_replace {
                        FindField::Replace
                    } else {
                        FindField::Find
                    };
                    self.set_focus(field, now);
                    FindAction::Redraw
                }
                Button::Case => {
                    self.case_sensitive = !self.case_sensitive;
                    FindAction::QueryChanged
                }
                Button::Word => {
                    self.whole_word = !self.whole_word;
                    FindAction::QueryChanged
                }
                Button::Regex => {
                    self.regex = !self.regex;
                    FindAction::QueryChanged
                }
                Button::Previous => FindAction::Previous,
                Button::Next => FindAction::Next,
                Button::Close => FindAction::Close,
                Button::Replace => FindAction::ReplaceOne,
                Button::ReplaceAll => FindAction::ReplaceAll,
            });
        }
        let field = if self.find.contains(x, y) {
            FindField::Find
        } else if self.show_replace && self.replace.contains(x, y) {
            FindField::Replace
        } else {
            return Some(FindAction::Redraw);
        };
        self.set_focus(field, now);
        self.focused_input()
            .mouse_down(p, x, clicks, modifiers, now);
        Some(FindAction::Redraw)
    }

    pub fn mouse_move(&mut self, p: &mut Painter, x: f32, y: f32) -> bool {
        let hover = self
            .buttons
            .iter()
            .find(|(r, _)| r.contains(x as i32, y as i32))
            .map(|&(_, b)| b);
        let changed = hover != self.hover;
        self.hover = hover;
        let a = self.find.mouse_move(p, x);
        let b = self.replace.mouse_move(p, x);
        changed || a || b
    }

    pub fn mouse_up(&mut self) {
        self.find.mouse_up();
        self.replace.mouse_up();
    }

    pub fn draw(&mut self, p: &mut Painter, area: Rect, now: Instant) {
        let t = p.theme;
        self.area = area;
        self.buttons.clear();
        p.rect(area, t.chrome_bg);
        p.rect(Rect::new(area.x, area.y, area.w, p.px(1.0)), t.border);
        let pad = p.px(PAD);
        let row_h = p.px(ROW);
        let field_h = row_h - pad;
        let small = p.px(28.0);
        let gap = p.px(4.0);
        let style = p.style(12.0);

        let row_y = |row: i32| area.y + pad + row * row_h;
        let toggle = Rect::new(area.x + pad, row_y(0), p.px(22.0), field_h);
        self.buttons.push((toggle, Button::ToggleReplace));
        let label = if self.show_replace { "▾" } else { "▸" };
        button(
            p,
            toggle,
            label,
            false,
            self.hover == Some(Button::ToggleReplace),
            false,
        );
        let left = toggle.right() + gap;

        // Row 1, right to left: close, next, previous, status, regex, word, case.
        let mut x = area.right() - pad;
        let mut place = |w: i32, b: Option<Button>, buttons: &mut Vec<(Rect, Button)>| {
            x -= w;
            let r = Rect::new(x, row_y(0), w, field_h);
            x -= gap;
            if let Some(b) = b {
                buttons.push((r, b));
            }
            r
        };
        let close = place(small, Some(Button::Close), &mut self.buttons);
        let next = place(small, Some(Button::Next), &mut self.buttons);
        let prev = place(small, Some(Button::Previous), &mut self.buttons);
        let status_w = p.px(120.0);
        let status = place(status_w, None, &mut self.buttons);
        let regex = place(small, Some(Button::Regex), &mut self.buttons);
        let word = place(small, Some(Button::Word), &mut self.buttons);
        let case = place(small, Some(Button::Case), &mut self.buttons);
        let field_right = x;
        let h = self.hover;
        button(p, close, "×", false, h == Some(Button::Close), false);
        button(p, next, "↓", false, h == Some(Button::Next), false);
        button(p, prev, "↑", false, h == Some(Button::Previous), false);
        button(p, regex, ".*", self.regex, h == Some(Button::Regex), false);
        button(
            p,
            word,
            "W",
            self.whole_word,
            h == Some(Button::Word),
            false,
        );
        button(
            p,
            case,
            "Aa",
            self.case_sensitive,
            h == Some(Button::Case),
            false,
        );

        let (text, error) = match &self.status {
            FindStatus::Empty => (String::new(), false),
            FindStatus::Counting => ("…".to_owned(), false),
            FindStatus::NoMatches => ("No results".to_owned(), true),
            FindStatus::Error(_) => ("Invalid regex".to_owned(), true),
            FindStatus::Count {
                current: Some(c),
                total,
            } => (format!("{c} of {total}"), false),
            FindStatus::Count {
                current: None,
                total,
            } => (format!("{total} found"), false),
        };
        let fitted = p.fit(&text, &style, status.w as f32);
        let w = p.measure(&fitted, &style);
        let baseline = p.center_baseline(status.y, status.h, &style);
        let color = if error { t.error } else { t.chrome_text_muted };
        p.text(
            &fitted,
            &style,
            status.x as f32 + (status.w as f32 - w) / 2.0,
            baseline,
            color,
        );

        let find_rect = Rect::new(
            left,
            row_y(0),
            (field_right - left).max(p.px(60.0)),
            field_h,
        );
        let is_error = matches!(self.status, FindStatus::Error(_));
        self.find.draw(p, find_rect, now, "Find", is_error);

        if self.show_replace {
            let mut x = area.right() - pad;
            let all_w = p.px(48.0);
            let one_w = p.px(72.0);
            x -= all_w;
            let all = Rect::new(x, row_y(1), all_w, field_h);
            x -= gap + one_w;
            let one = Rect::new(x, row_y(1), one_w, field_h);
            x -= gap;
            self.buttons.push((all, Button::ReplaceAll));
            self.buttons.push((one, Button::Replace));
            button(p, one, "Replace", false, h == Some(Button::Replace), false);
            button(p, all, "All", false, h == Some(Button::ReplaceAll), false);
            let replace_rect = Rect::new(left, row_y(1), (x - left).max(p.px(60.0)), field_h);
            self.replace.draw(p, replace_rect, now, "Replace", false);
        }
    }
}

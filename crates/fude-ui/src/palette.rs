//! The command palette: a fuzzy-filtered list of every command, and go to line (type
//! `:` and a line number, optionally `:column`).

use std::time::Instant;

use accesskit::{Action, ActionRequest, Live, NodeId, Role};
use fude_platform::{Ime, Key, KeyInput, Modifiers};
use fude_render::Rect;
use fude_text::LineLayout;

use crate::access::{self, AccessTree, Field, Part};
use crate::fuzzy;
use crate::input::{ImeChange, InputKey, TextInput};
use crate::painter::Painter;

pub struct PaletteItem {
    pub title: String,
    /// Shown on the right: a shortcut, a path.
    pub detail: String,
}

/// What the palette asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteAction {
    /// Run the item at this index (into the items given to `open`).
    Run(usize),
    /// Show this line (1-based) while typing, and its column if given.
    Preview {
        line: usize,
        column: Option<usize>,
    },
    /// Enter on a line number: go there.
    GoTo {
        line: usize,
        column: Option<usize>,
    },
    Close,
    Redraw,
}

const WIDTH: f32 = 620.0;
const ROW: f32 = 28.0;
const MAX_ROWS: usize = 12;

#[derive(Default)]
pub struct Palette {
    pub input: TextInput,
    items: Vec<PaletteItem>,
    /// Indices into `items` that match, best first, with their matched characters.
    matches: Vec<(usize, Vec<usize>)>,
    selected: usize,
    /// First visible row.
    top: usize,
    rows: Vec<Rect>,
    area: Rect,
    hover: Option<usize>,
    layout: LineLayout,
    /// What the list area says instead of commands (go to line, no matches), and where.
    note: Option<(String, Rect)>,
}

impl Palette {
    pub fn new() -> Palette {
        Palette::default()
    }

    /// Opens with `items` and the input set to `prefix` (":" for go to line).
    pub fn open(&mut self, items: Vec<PaletteItem>, prefix: &str, now: Instant) {
        self.items = items;
        self.input.set_text(prefix);
        let len = self.input.editor.text().len();
        self.input.editor.select(len..len);
        self.input.set_focused(true, now);
        self.filter();
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x as i32, y as i32)
    }

    pub fn caret_rect(&self) -> Option<Rect> {
        self.input.caret_rect()
    }

    pub fn next_timer(&self, now: Instant) -> Option<Instant> {
        self.input.next_timer(now)
    }

    /// Go to line mode: the text starts with `:`. Returns the line and column typed.
    fn line_target(&self) -> Option<(usize, Option<usize>)> {
        let text = self.input.text();
        let rest = text.strip_prefix(':')?;
        let mut parts = rest.trim().splitn(2, [':', ',']);
        let line = parts.next()?.trim().parse().ok()?;
        let column = parts.next().and_then(|c| c.trim().parse().ok());
        Some((line, column))
    }

    fn filter(&mut self) {
        let text = self.input.text();
        self.matches.clear();
        self.selected = 0;
        self.top = 0;
        if text.starts_with(':') {
            return;
        }
        let mut scored: Vec<(i32, usize, Vec<usize>)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| fuzzy::score(&text, &item.title).map(|(s, pos)| (s, i, pos)))
            .collect();
        // Best score first; ties keep the given order.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.matches = scored.into_iter().map(|(_, i, pos)| (i, pos)).collect();
    }

    fn after_edit(&mut self) -> PaletteAction {
        self.filter();
        match self.line_target() {
            Some((line, column)) => PaletteAction::Preview { line, column },
            None => PaletteAction::Redraw,
        }
    }

    pub fn key(&mut self, key: &KeyInput, now: Instant) -> Option<PaletteAction> {
        if !key.pressed {
            return None;
        }
        let n = self.matches.len();
        match key.key {
            Key::Escape => return Some(PaletteAction::Close),
            Key::Enter => {
                if let Some((line, column)) = self.line_target() {
                    return Some(PaletteAction::GoTo { line, column });
                }
                return Some(match self.matches.get(self.selected) {
                    Some(&(i, _)) => PaletteAction::Run(i),
                    None => PaletteAction::Close,
                });
            }
            Key::Down if n > 0 => {
                self.select((self.selected + 1) % n);
                return Some(PaletteAction::Redraw);
            }
            Key::Up if n > 0 => {
                self.select((self.selected + n - 1) % n);
                return Some(PaletteAction::Redraw);
            }
            Key::PageDown if n > 0 => {
                self.select((self.selected + MAX_ROWS).min(n - 1));
                return Some(PaletteAction::Redraw);
            }
            Key::PageUp if n > 0 => {
                self.select(self.selected.saturating_sub(MAX_ROWS));
                return Some(PaletteAction::Redraw);
            }
            _ => {}
        }
        match self.input.key(key, now) {
            InputKey::Changed => Some(self.after_edit()),
            InputKey::Moved => Some(PaletteAction::Redraw),
            InputKey::Ignored => None,
        }
    }

    pub fn ime(&mut self, ime: Ime, now: Instant) -> Option<PaletteAction> {
        match self.input.ime(ime, now) {
            ImeChange::Nothing => None,
            ImeChange::Preedit => Some(PaletteAction::Redraw),
            ImeChange::Text => Some(self.after_edit()),
        }
    }

    pub fn paste(&mut self, text: &str, now: Instant) -> PaletteAction {
        self.input.paste(text, now);
        self.after_edit()
    }

    fn select(&mut self, i: usize) {
        self.selected = i;
        if i < self.top {
            self.top = i;
        } else if i >= self.top + MAX_ROWS {
            self.top = i + 1 - MAX_ROWS;
        }
    }

    pub fn scroll(&mut self, rows: i32) {
        let max = self.matches.len().saturating_sub(MAX_ROWS);
        self.top = (self.top as i32 + rows).clamp(0, max as i32) as usize;
    }

    pub fn mouse_down(
        &mut self,
        p: &mut Painter,
        x: f32,
        y: f32,
        clicks: u8,
        modifiers: Modifiers,
        now: Instant,
    ) -> Option<PaletteAction> {
        if !self.contains(x, y) {
            return Some(PaletteAction::Close);
        }
        if let Some(row) = self
            .rows
            .iter()
            .position(|r| r.contains(x as i32, y as i32))
        {
            let i = self.top + row;
            return self
                .matches
                .get(i)
                .map(|&(item, _)| PaletteAction::Run(item));
        }
        if self.input.contains(x, y) {
            self.input.mouse_down(p, x, clicks, modifiers, now);
        }
        Some(PaletteAction::Redraw)
    }

    pub fn mouse_move(&mut self, p: &mut Painter, x: f32, y: f32) -> bool {
        let hover = self
            .rows
            .iter()
            .position(|r| r.contains(x as i32, y as i32));
        let changed = hover != self.hover;
        self.hover = hover;
        self.input.mouse_move(p, x) || changed
    }

    pub fn mouse_up(&mut self) {
        self.input.mouse_up();
    }

    /// Describes the palette for assistive technologies: a dialog with the field, and
    /// the commands on screen as a list whose selected item is the field's active
    /// descendant (so moving through the list reads each item while typing stays in
    /// the field).
    pub fn access(&mut self, p: &mut Painter, tree: &mut AccessTree) -> NodeId {
        let placeholder = "Type a command, or : and a line number";
        let input = self
            .input
            .access(p, tree, Field::Palette, "Command", placeholder, false);
        let mut children = vec![input];
        if let Some((text, rect)) = &self.note {
            let mut note = access::text_node(text, *rect);
            note.set_live(Live::Polite);
            children.push(tree.push(Part::PaletteList, note));
        } else {
            let mut items = Vec::new();
            for (row, rect) in self.rows.iter().enumerate() {
                let i = self.top + row;
                let Some((item, _)) = self.matches.get(i) else {
                    break;
                };
                let item = &self.items[*item];
                let mut node = access::clickable(Role::ListBoxOption, &item.title, *rect);
                node.add_action(Action::Focus);
                node.set_selected(i == self.selected);
                node.set_position_in_set(i);
                if !item.detail.is_empty() {
                    node.set_keyboard_shortcut(item.detail.as_str());
                }
                items.push(tree.push(Part::PaletteItem(i), node));
            }
            let area = match (self.rows.first(), self.rows.last()) {
                (Some(a), Some(b)) => Rect::new(a.x, a.y, a.w, b.bottom() - a.y),
                _ => Rect::default(),
            };
            let mut list = access::labeled(Role::ListBox, "Commands", area);
            list.set_size_of_set(self.matches.len());
            list.set_children(items);
            let list = tree.push(Part::PaletteList, list);
            children.push(list);
            if let Some(field) = tree.node_mut(input) {
                field.set_controls(vec![list]);
                if self.matches.get(self.selected).is_some() {
                    field.set_active_descendant(Part::PaletteItem(self.selected).id());
                }
            }
        }
        let mut dialog = access::labeled(Role::Dialog, "Command palette", self.area);
        dialog.set_children(children);
        tree.push(Part::Palette, dialog)
    }

    /// Does what an assistive technology asks of a part of the palette.
    pub fn access_action(
        &mut self,
        part: Part,
        request: &ActionRequest,
        now: Instant,
    ) -> Option<PaletteAction> {
        match part {
            Part::PaletteItem(i) => {
                let &(item, _) = self.matches.get(i)?;
                match request.action {
                    Action::Click => Some(PaletteAction::Run(item)),
                    Action::Focus | Action::ScrollIntoView => {
                        self.select(i);
                        Some(PaletteAction::Redraw)
                    }
                    _ => None,
                }
            }
            Part::Field(Field::Palette) => {
                match self.input.access_action(Field::Palette, request, now) {
                    InputKey::Changed => Some(self.after_edit()),
                    InputKey::Moved => Some(PaletteAction::Redraw),
                    InputKey::Ignored => None,
                }
            }
            _ => None,
        }
    }

    pub fn draw(&mut self, p: &mut Painter, window: Rect, top: i32, now: Instant) {
        let t = p.theme;
        let pad = p.px(8.0);
        let width = p.px(WIDTH).min(window.w - p.px(32.0)).max(p.px(200.0));
        let row_h = p.px(ROW);
        let input_h = p.px(32.0);
        let going_to_line = self.line_target().is_some() || self.input.text().starts_with(':');
        let rows = if going_to_line {
            1
        } else {
            self.matches.len().clamp(1, MAX_ROWS)
        };
        let height = pad + input_h + pad + rows as i32 * row_h + pad;
        let x = window.x + (window.w - width) / 2;
        let y = top + p.px(8.0);
        self.area = Rect::new(x, y, width, height);
        p.rect(Rect::new(x - 1, y - 1, width + 2, height + 2), t.border);
        p.rect(self.area, t.dialog_bg);
        let placeholder = "Type a command, or : and a line number";
        self.input.draw(
            p,
            Rect::new(x + pad, y + pad, width - 2 * pad, input_h),
            now,
            placeholder,
            false,
        );

        let style = p.style(13.0);
        let list_y = y + pad + input_h + pad;
        self.rows.clear();
        self.note = None;
        let note_rect = Rect::new(x + pad / 2, list_y, width - pad, row_h);
        if going_to_line {
            let hint = match self.line_target() {
                Some((line, Some(col))) => format!("Go to line {line}, column {col}"),
                Some((line, None)) => format!("Go to line {line}"),
                None => "Type a line number, and optionally :column".to_owned(),
            };
            let baseline = p.center_baseline(list_y, row_h, &style);
            p.text(
                &hint,
                &style,
                (x + 2 * pad) as f32,
                baseline,
                t.chrome_text_muted,
            );
            self.note = Some((hint, note_rect));
            return;
        }
        if self.matches.is_empty() {
            let baseline = p.center_baseline(list_y, row_h, &style);
            let note = "No matching commands";
            p.text(
                note,
                &style,
                (x + 2 * pad) as f32,
                baseline,
                t.chrome_text_muted,
            );
            self.note = Some((note.to_owned(), note_rect));
            return;
        }
        let visible: Vec<(usize, (usize, Vec<usize>))> = self
            .matches
            .iter()
            .cloned()
            .enumerate()
            .skip(self.top)
            .take(MAX_ROWS)
            .collect();
        for (row, (i, (item, positions))) in visible.into_iter().enumerate() {
            let r = Rect::new(x + pad / 2, list_y + row as i32 * row_h, width - pad, row_h);
            self.rows.push(r);
            if i == self.selected {
                p.rect(r, t.selection);
            } else if self.hover == Some(row) {
                p.rect(r, t.line_highlight);
            }
            let baseline = p.center_baseline(r.y, r.h, &style);
            let title = self.items[item].title.clone();
            let detail = self.items[item].detail.clone();
            let tx = (r.x + pad) as f32;
            p.text(&title, &style, tx, baseline, t.text);
            // Matched characters again, in the accent color, over the plain ones.
            if !positions.is_empty() {
                p.text.layout_line(&title, &style, &mut self.layout);
                let byte_positions: Vec<usize> = title
                    .char_indices()
                    .enumerate()
                    .filter(|(ci, _)| positions.contains(ci))
                    .map(|(_, (b, _))| b)
                    .collect();
                self.layout
                    .glyphs
                    .retain(|g| byte_positions.contains(&(g.cluster as usize)));
                let layout = std::mem::take(&mut self.layout);
                p.text.draw_line(
                    &layout,
                    &style,
                    tx,
                    baseline,
                    t.accent,
                    (f32::MIN, f32::MAX),
                    &mut p.list,
                    &mut p.images,
                );
                self.layout = layout;
            }
            let dw = p.measure(&detail, &style);
            p.text(
                &detail,
                &style,
                (r.right() - pad) as f32 - dw,
                baseline,
                t.chrome_text_muted,
            );
        }
    }
}

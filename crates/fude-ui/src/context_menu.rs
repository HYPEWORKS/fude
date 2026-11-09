//! A transient command menu, positioned at the pointer and kept inside the window.

use fude_platform::{Key, KeyInput};
use fude_render::Rect;

use crate::{Action, Painter};

#[derive(Clone, Debug)]
pub struct ContextMenuItem {
    /// `None` draws a separator.
    pub action: Option<Action>,
    pub label: String,
    pub shortcut: String,
    pub enabled: bool,
}

impl ContextMenuItem {
    pub fn command(
        action: Action,
        label: impl Into<String>,
        shortcut: impl Into<String>,
        enabled: bool,
    ) -> Self {
        Self {
            action: Some(action),
            label: label.into(),
            shortcut: shortcut.into(),
            enabled,
        }
    }

    pub fn separator() -> Self {
        Self {
            action: None,
            label: String::new(),
            shortcut: String::new(),
            enabled: false,
        }
    }

    fn selectable(&self) -> bool {
        self.enabled && self.action.is_some()
    }
}

#[derive(Default)]
pub struct ContextMenu {
    open: bool,
    items: Vec<ContextMenuItem>,
    area: Rect,
    /// Rows in content coordinates; scrolling is applied for drawing and hit testing.
    rows: Vec<Rect>,
    selected: Option<usize>,
    scroll: i32,
    content_height: i32,
    pad: i32,
    border: i32,
    shortcut_width: i32,
}

impl ContextMenu {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(
        &mut self,
        p: &mut Painter,
        window: Rect,
        x: f32,
        y: f32,
        items: Vec<ContextMenuItem>,
    ) {
        self.items = items;
        self.pad = p.px(12.0).max(1);
        self.border = p.px(1.0).max(1);
        let style = p.style(13.0);
        let mut label_width = 0;
        self.shortcut_width = 0;
        for item in &self.items {
            label_width = label_width.max(p.measure(&item.label, &style).ceil() as i32);
            self.shortcut_width = self
                .shortcut_width
                .max(p.measure(&item.shortcut, &style).ceil() as i32);
        }
        let gap = if self.shortcut_width > 0 {
            2 * self.pad
        } else {
            0
        };
        let width = (label_width + self.shortcut_width + gap + 2 * self.pad + 2 * self.border)
            .max(p.px(200.0));
        self.place(window, x, y, width, p.px(28.0).max(1), p.px(9.0).max(1));
    }

    fn place(&mut self, window: Rect, x: f32, y: f32, width: i32, row: i32, separator: i32) {
        self.rows.clear();
        let mut top = 0;
        for item in &self.items {
            let height = if item.action.is_some() {
                row
            } else {
                separator
            };
            self.rows.push(Rect::new(0, top, 0, height));
            top += height;
        }
        self.content_height = top;
        let inset = self.inset();
        let width = width.min(window.w.max(0));
        let height = (top + 2 * inset).min(window.h.max(0));
        let x = (x.round() as i32).clamp(window.x, window.right() - width);
        let y = (y.round() as i32).clamp(window.y, window.bottom() - height);
        self.area = Rect::new(x, y, width, height);
        self.open = !self.area.is_empty() && !self.items.is_empty();
        self.selected = None;
        self.scroll = 0;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn close(&mut self) {
        self.open = false;
        self.selected = None;
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.open && self.area.contains(x as i32, y as i32)
    }

    fn inset(&self) -> i32 {
        self.border + self.pad / 2
    }

    fn viewport(&self) -> Rect {
        let inset = self.inset();
        Rect::new(
            self.area.x + self.border,
            self.area.y + inset,
            (self.area.w - 2 * self.border).max(0),
            (self.area.h - 2 * inset).max(0),
        )
    }

    fn row_rect(&self, index: usize) -> Rect {
        let viewport = self.viewport();
        let row = self.rows[index];
        Rect::new(
            viewport.x,
            viewport.y + row.y - self.scroll,
            viewport.w,
            row.h,
        )
    }

    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        if !self.contains(x, y) || !self.viewport().contains(x as i32, y as i32) {
            return None;
        }
        self.items.iter().enumerate().find_map(|(i, item)| {
            (item.selectable() && self.row_rect(i).contains(x as i32, y as i32)).then_some(i)
        })
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        if !self.open {
            return false;
        }
        let selected = self.hit(x, y);
        let changed = self.selected != selected;
        self.selected = selected;
        changed
    }

    pub fn mouse_down(&mut self, x: f32, y: f32) -> Option<Action> {
        if !self.open {
            return None;
        }
        if !self.contains(x, y) {
            self.close();
            return None;
        }
        let index = self.hit(x, y)?;
        let action = self.items[index].action;
        self.close();
        action
    }

    /// Scrolls a menu that is taller than the window. Returns whether it changed.
    pub fn scroll(&mut self, rows: i32) -> bool {
        if !self.open {
            return false;
        }
        let step = self
            .items
            .iter()
            .position(|item| item.action.is_some())
            .map_or(1, |i| self.rows[i].h);
        let max = (self.content_height - self.viewport().h).max(0);
        let scroll = self
            .scroll
            .saturating_add(rows.saturating_mul(step))
            .clamp(0, max);
        let changed = scroll != self.scroll;
        if changed {
            self.scroll = scroll;
            self.selected = None;
        }
        changed
    }

    fn select(&mut self, index: usize) {
        self.selected = Some(index);
        let row = self.rows[index];
        let height = self.viewport().h;
        if row.y < self.scroll {
            self.scroll = row.y;
        } else if row.bottom() > self.scroll + height {
            self.scroll = (row.bottom() - height).max(0);
        }
    }

    fn step(&mut self, forward: bool) {
        let n = self.items.len();
        if n == 0 {
            return;
        }
        let start = self.selected.unwrap_or(if forward { n - 1 } else { 0 });
        for distance in 1..=n {
            let i = if forward {
                (start + distance) % n
            } else {
                (start + n - distance) % n
            };
            if self.items[i].selectable() {
                self.select(i);
                break;
            }
        }
    }

    /// The caller consumes keys while the menu is open, including unhandled keys.
    pub fn key(&mut self, key: &KeyInput) -> Option<Action> {
        if !self.open || !key.pressed {
            return None;
        }
        match &key.key {
            Key::Escape | Key::Tab => self.close(),
            Key::Down => self.step(true),
            Key::Up => self.step(false),
            Key::Home | Key::End => {
                let index = if key.key == Key::Home {
                    self.items.iter().position(ContextMenuItem::selectable)
                } else {
                    self.items.iter().rposition(ContextMenuItem::selectable)
                };
                if let Some(index) = index {
                    self.select(index);
                }
            }
            Key::Enter => return self.activate(),
            Key::Character(s) if s == " " => return self.activate(),
            _ => {}
        }
        None
    }

    fn activate(&mut self) -> Option<Action> {
        let index = self.selected?;
        let action = self.items[index].action;
        self.close();
        action
    }

    pub fn draw(&mut self, p: &mut Painter) {
        if !self.open {
            return;
        }
        let t = p.theme;
        p.rect(self.area, t.border);
        let interior = Rect::new(
            self.area.x + self.border,
            self.area.y + self.border,
            (self.area.w - 2 * self.border).max(0),
            (self.area.h - 2 * self.border).max(0),
        );
        p.rect(interior, t.dialog_bg);
        let viewport = self.viewport();
        p.list.push_clip(viewport);
        let style = p.style(13.0);
        for (i, item) in self.items.iter().enumerate() {
            let row = self.row_rect(i);
            if row.intersect(viewport).is_empty() {
                continue;
            }
            if item.action.is_none() {
                p.rect(
                    Rect::new(
                        row.x + self.pad,
                        row.y + row.h / 2,
                        row.w - 2 * self.pad,
                        self.border,
                    ),
                    t.border,
                );
                continue;
            }
            if self.selected == Some(i) {
                p.rect(row, t.selection);
            }
            let color = if item.enabled {
                t.text
            } else {
                t.chrome_text_muted
            };
            let baseline = p.center_baseline(row.y, row.h, &style);
            let available = (row.w - 2 * self.pad).max(0);
            let shortcut_width = self.shortcut_width.min(available / 2);
            let gap = if shortcut_width > 0 { self.pad } else { 0 };
            let label = p.fit(
                &item.label,
                &style,
                (available - shortcut_width - gap).max(0) as f32,
            );
            p.text(&label, &style, (row.x + self.pad) as f32, baseline, color);
            if !item.shortcut.is_empty() && shortcut_width > 0 {
                let shortcut = p.fit(&item.shortcut, &style, shortcut_width as f32);
                let width = p.measure(&shortcut, &style);
                p.text(
                    &shortcut,
                    &style,
                    (row.right() - self.pad) as f32 - width,
                    baseline,
                    t.chrome_text_muted,
                );
            }
        }
        p.list.pop_clip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fude_platform::Modifiers;

    fn menu(window: Rect) -> ContextMenu {
        let mut menu = ContextMenu {
            items: vec![
                ContextMenuItem::command(Action::Undo, "Undo", "Ctrl+Z", false),
                ContextMenuItem::separator(),
                ContextMenuItem::command(Action::Copy, "Copy", "Ctrl+C", true),
                ContextMenuItem::command(Action::Paste, "Paste", "Ctrl+V", true),
            ],
            pad: 12,
            border: 1,
            ..ContextMenu::default()
        };
        menu.place(window, 480.0, 380.0, 240, 28, 9);
        menu
    }

    fn key(menu: &mut ContextMenu, key: Key) -> Option<Action> {
        menu.key(&KeyInput {
            key,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
            text: None,
            base: None,
        })
    }

    #[test]
    fn placement_stays_inside_window_and_hits_before_drawing() {
        let mut menu = menu(Rect::new(0, 0, 500, 400));
        assert_eq!(menu.area.right(), 500);
        assert_eq!(menu.area.bottom(), 400);
        assert_eq!(menu.selected, None);
        let row = menu.row_rect(2);
        assert_eq!(
            menu.mouse_down((row.x + 10) as f32, (row.y + 10) as f32),
            Some(Action::Copy)
        );
        assert!(!menu.is_open());
    }

    #[test]
    fn disabled_rows_and_separators_do_not_run_or_close() {
        let mut menu = menu(Rect::new(0, 0, 500, 400));
        for i in [0, 1] {
            let row = menu.row_rect(i);
            assert_eq!(
                menu.mouse_down((row.x + 10) as f32, (row.y + 1) as f32),
                None
            );
            assert!(menu.is_open());
        }
        assert_eq!(menu.mouse_down(0.0, 0.0), None);
        assert!(!menu.is_open());
    }

    #[test]
    fn keyboard_skips_disabled_rows_and_separators_and_wraps() {
        let mut menu = menu(Rect::new(0, 0, 500, 400));
        key(&mut menu, Key::Down);
        assert_eq!(menu.selected, Some(2));
        key(&mut menu, Key::Up);
        assert_eq!(menu.selected, Some(3));
        key(&mut menu, Key::Home);
        assert_eq!(menu.selected, Some(2));
        key(&mut menu, Key::End);
        assert_eq!(menu.selected, Some(3));
        assert_eq!(key(&mut menu, Key::Enter), Some(Action::Paste));
        assert!(!menu.is_open());
    }

    #[test]
    fn escape_dismisses_without_running_and_key_release_is_ignored() {
        let mut menu = menu(Rect::new(0, 0, 500, 400));
        menu.key(&KeyInput {
            key: Key::Down,
            pressed: false,
            repeat: false,
            modifiers: Modifiers::default(),
            text: None,
            base: None,
        });
        assert_eq!(menu.selected, None);
        key(&mut menu, Key::Down);
        assert_eq!(key(&mut menu, Key::Escape), None);
        assert!(!menu.is_open());
        assert_eq!(key(&mut menu, Key::Enter), None);
    }

    #[test]
    fn small_windows_clip_and_scroll_to_keyboard_selection() {
        let window = Rect::new(20, 30, 120, 70);
        let mut menu = menu(window);
        assert_eq!(menu.area, window);
        key(&mut menu, Key::End);
        let row = menu.row_rect(3);
        assert!(row.y >= menu.viewport().y);
        assert!(row.bottom() <= menu.viewport().bottom());
        assert_eq!(
            menu.mouse_down((row.x + 1) as f32, (row.y + 1) as f32),
            Some(Action::Paste)
        );
    }

    #[test]
    fn wheel_scrolling_changes_hit_testing() {
        let mut menu = menu(Rect::new(0, 0, 200, 70));
        assert!(menu.scroll(10));
        let row = menu.row_rect(3);
        assert_eq!(menu.hit((row.x + 1) as f32, (row.y + 1) as f32), Some(3));
        assert!(!menu.scroll(1));
        assert!(menu.scroll(-10));
    }
}

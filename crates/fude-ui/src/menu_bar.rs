//! The application menus: what's in them ([`menus`]), and the menu bar drawn in the
//! window on Windows, Linux and BSD. (macOS shows the same menus in its own menu bar.)
//!
//! Click a title to open its menu, then move along the bar to open the others. From the
//! keyboard, F10 opens the first menu and Alt with a title's first letter opens that
//! one; the arrows move through the items and across the menus, Enter runs, Escape
//! closes.

use accesskit::{Action as AkAction, ActionRequest, HasPopup, NodeId, Role};
use fude_buffer::LineEnding;
use fude_platform::{Key, KeyInput};
use fude_render::Rect;
use fude_tools::Tool;

use crate::access::{self, AccessTree, Part};
use crate::{Action, ContextMenu, ContextMenuItem, Painter};

/// Logical height of the bar.
pub const MENU_BAR_HEIGHT: f32 = 28.0;
const FONT: f32 = 13.0;
const PAD: f32 = 10.0;

/// One of the application's menus.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub title: &'static str,
    pub items: Vec<MenuEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    Command { action: Action, label: String },
    Separator,
}

/// The menus, left to right, for this platform. `external_tools` are the names of the
/// tools in the settings, which end the Tools menu.
pub fn menus(external_tools: &[&str]) -> Vec<Menu> {
    use MenuEntry::Separator;
    let mac = cfg!(target_os = "macos");
    let c = |action: Action, label: &str| MenuEntry::Command {
        action,
        label: label.to_owned(),
    };
    let quit = if mac {
        "Quit fude"
    } else if cfg!(windows) {
        "Exit"
    } else {
        "Quit"
    };
    let mut tools = Vec::new();
    for group in TOOL_GROUPS {
        if !tools.is_empty() {
            tools.push(Separator);
        }
        tools.extend(group.iter().map(|&t| c(Action::Tool(t), t.title())));
    }
    tools.extend([
        Separator,
        c(
            Action::LineEndings(LineEnding::Lf),
            "Convert Line Endings to LF",
        ),
        c(
            Action::LineEndings(LineEnding::CrLf),
            "Convert Line Endings to CRLF",
        ),
        c(
            Action::LineEndings(LineEnding::Cr),
            "Convert Line Endings to CR",
        ),
    ]);
    if !external_tools.is_empty() {
        tools.push(Separator);
        tools.extend(
            external_tools
                .iter()
                .enumerate()
                .map(|(i, name)| c(Action::External(i as u16), name)),
        );
    }
    vec![
        Menu {
            title: "File",
            items: vec![
                c(Action::New, "New Tab"),
                c(Action::Open, "Open…"),
                Separator,
                c(Action::Save, "Save"),
                c(Action::SaveAs, "Save As…"),
                Separator,
                c(Action::Close, "Close Tab"),
                Separator,
                c(
                    Action::OpenSettings,
                    if mac { "Settings…" } else { "Settings" },
                ),
                Separator,
                c(Action::Quit, quit),
            ],
        },
        Menu {
            title: "Edit",
            items: vec![
                c(Action::Undo, "Undo"),
                c(Action::Redo, "Redo"),
                Separator,
                c(Action::Cut, "Cut"),
                c(Action::Copy, "Copy"),
                c(Action::Paste, "Paste"),
                Separator,
                c(Action::Find, "Find…"),
                c(Action::Replace, "Replace…"),
                c(Action::FindNext, "Find Next"),
                c(Action::FindPrevious, "Find Previous"),
                Separator,
                c(Action::ToggleComment, "Toggle Line Comment"),
                c(Action::ToggleBlockComment, "Toggle Block Comment"),
            ],
        },
        Menu {
            title: "Selection",
            items: vec![
                c(Action::SelectAll, "Select All"),
                Separator,
                c(Action::SelectNextOccurrence, "Add Next Occurrence"),
                c(Action::SelectAllOccurrences, "Select All Occurrences"),
                c(Action::SplitIntoLines, "Split into Lines"),
                c(Action::AddCursorAbove, "Add Cursor Above"),
                c(Action::AddCursorBelow, "Add Cursor Below"),
                Separator,
                c(Action::MoveLineUp, "Move Line Up"),
                c(Action::MoveLineDown, "Move Line Down"),
                c(Action::DuplicateLine, "Duplicate Line"),
                c(Action::DeleteLine, "Delete Line"),
                c(Action::JoinLines, "Join Lines"),
                c(Action::InsertLineAbove, "Insert Line Above"),
                c(Action::InsertLineBelow, "Insert Line Below"),
                Separator,
                c(Action::Indent, "Indent"),
                c(Action::Outdent, "Outdent"),
            ],
        },
        Menu {
            title: "View",
            items: vec![
                c(Action::CommandPalette, "Command Palette…"),
                Separator,
                c(Action::ToggleWrap, "Word Wrap"),
                Separator,
                c(Action::ZoomIn, "Zoom In"),
                c(Action::ZoomOut, "Zoom Out"),
                c(Action::ZoomReset, "Reset Zoom"),
            ],
        },
        Menu {
            title: "Go",
            items: vec![
                c(Action::GoToLine, "Go to Line…"),
                c(Action::GoToBracket, "Go to Matching Bracket"),
                Separator,
                c(Action::NextTab, "Next Tab"),
                c(Action::PreviousTab, "Previous Tab"),
            ],
        },
        Menu {
            title: "Tools",
            items: tools,
        },
        Menu {
            title: "Help",
            items: vec![c(Action::About, "About fude")],
        },
    ]
}

/// The built-in tools in the Tools menu, a group between separators each.
const TOOL_GROUPS: [&[Tool]; 6] = [
    &[
        Tool::SortLines,
        Tool::SortLinesDescending,
        Tool::SortLinesIgnoringCase,
        Tool::SortLinesNaturally,
        Tool::RemoveDuplicateLines,
        Tool::ReverseLines,
        Tool::TrimTrailingWhitespace,
    ],
    &[Tool::IndentWithSpaces, Tool::IndentWithTabs],
    &[Tool::UpperCase, Tool::LowerCase, Tool::TitleCase],
    &[
        Tool::Base64Encode,
        Tool::Base64Decode,
        Tool::UrlEncode,
        Tool::UrlDecode,
        Tool::HtmlEncode,
        Tool::HtmlDecode,
    ],
    &[Tool::FormatJson, Tool::MinifyJson, Tool::FormatXml],
    &[Tool::Sha256, Tool::Sha1, Tool::Md5],
];

/// What an input did to the menu bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuBarEvent {
    /// Open this menu: the caller supplies its items to [`MenuBar::open`].
    Open(usize),
    /// An item was chosen; the menu has closed.
    Run(Action),
    Redraw,
    Ignored,
}

#[derive(Default)]
pub struct MenuBar {
    titles: Vec<String>,
    area: Rect,
    /// Title rectangles from the last draw.
    rects: Vec<Rect>,
    /// The title whose menu is open.
    open: Option<usize>,
    hover: Option<usize>,
    /// Opened from the keyboard: the first item is selected, and access keys are
    /// underlined.
    keyboard: bool,
    menu: ContextMenu,
}

impl MenuBar {
    pub fn new<S: Into<String>>(titles: impl IntoIterator<Item = S>) -> MenuBar {
        MenuBar {
            titles: titles.into_iter().map(Into::into).collect(),
            ..MenuBar::default()
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some() && self.menu.is_open()
    }

    /// The index of the open menu.
    pub fn open_index(&self) -> Option<usize> {
        self.open.filter(|_| self.menu.is_open())
    }

    /// Over the bar itself.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x as i32, y as i32)
    }

    /// Over the open menu.
    pub fn menu_contains(&self, x: f32, y: f32) -> bool {
        self.is_open() && self.menu.contains(x, y)
    }

    pub fn close(&mut self) {
        self.open = None;
        self.keyboard = false;
        self.menu.close();
    }

    fn title_at(&self, x: f32, y: f32) -> Option<usize> {
        if !self.contains(x, y) {
            return None;
        }
        self.rects
            .iter()
            .position(|r| r.contains(x as i32, y as i32))
    }

    /// Shows menu `index` below its title, kept inside `window` under the bar.
    pub fn open(
        &mut self,
        p: &mut Painter,
        window: Rect,
        index: usize,
        items: Vec<ContextMenuItem>,
    ) {
        let Some(&title) = self.rects.get(index) else {
            return;
        };
        let top = self.area.bottom();
        let below = Rect::new(window.x, top, window.w, (window.bottom() - top).max(0));
        self.menu.open(p, below, title.x as f32, top as f32, items);
        if !self.menu.is_open() {
            self.close();
            return;
        }
        self.open = Some(index);
        if self.keyboard {
            self.menu.select_first();
        }
    }

    /// F10, or Alt and a menu's first letter, opens a menu from the keyboard.
    pub fn activation(&mut self, key: &KeyInput) -> MenuBarEvent {
        if !key.pressed || key.repeat || self.titles.is_empty() {
            return MenuBarEvent::Ignored;
        }
        let m = key.modifiers;
        let index = if key.key == Key::F(10) && !(m.shift || m.ctrl || m.alt || m.logo) {
            Some(0)
        } else {
            self.access_key(key)
        };
        match index {
            Some(i) => {
                self.keyboard = true;
                MenuBarEvent::Open(i)
            }
            None => MenuBarEvent::Ignored,
        }
    }

    /// Alt and the first letter of a title.
    fn access_key(&self, key: &KeyInput) -> Option<usize> {
        let m = key.modifiers;
        if !m.alt || m.ctrl || m.logo || m.shift {
            return None;
        }
        let c = key.shortcut_char()?.to_lowercase();
        self.titles.iter().position(|title| {
            title
                .chars()
                .next()
                .is_some_and(|first| first.to_lowercase().eq(c.chars()))
        })
    }

    /// Keys while a menu is open. The caller sends every key here first, and consumes
    /// them all.
    pub fn key(&mut self, key: &KeyInput) -> MenuBarEvent {
        let Some(current) = self.open_index() else {
            return MenuBarEvent::Ignored;
        };
        if !key.pressed {
            return MenuBarEvent::Ignored;
        }
        let n = self.titles.len();
        match key.key {
            Key::Left | Key::Right => {
                self.keyboard = true;
                let next = if key.key == Key::Left {
                    current + n - 1
                } else {
                    current + 1
                };
                return MenuBarEvent::Open(next % n);
            }
            Key::F(10) => {
                self.close();
                return MenuBarEvent::Redraw;
            }
            _ => {}
        }
        if let Some(i) = self.access_key(key) {
            self.keyboard = true;
            return if i == current {
                MenuBarEvent::Ignored
            } else {
                MenuBarEvent::Open(i)
            };
        }
        let action = self.menu.key(key);
        if !self.menu.is_open() {
            self.close();
        }
        match action {
            Some(action) => MenuBarEvent::Run(action),
            None => MenuBarEvent::Redraw,
        }
    }

    /// A press anywhere while a menu is open, or on the bar. A press outside the open
    /// menu closes it (and does nothing else).
    pub fn mouse_down(&mut self, x: f32, y: f32) -> MenuBarEvent {
        if let Some(i) = self.title_at(x, y) {
            if self.open_index() == Some(i) {
                self.close();
                return MenuBarEvent::Redraw;
            }
            self.keyboard = false;
            return MenuBarEvent::Open(i);
        }
        if !self.is_open() {
            return MenuBarEvent::Ignored;
        }
        if !self.menu.contains(x, y) {
            self.close();
            return MenuBarEvent::Redraw;
        }
        match self.menu.mouse_down(x, y) {
            Some(action) => {
                self.close();
                MenuBarEvent::Run(action)
            }
            None => MenuBarEvent::Redraw,
        }
    }

    /// With a menu open, moving onto another title opens that one instead.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> MenuBarEvent {
        let title = self.title_at(x, y);
        if let Some(current) = self.open_index() {
            if let Some(i) = title
                && i != current
            {
                return MenuBarEvent::Open(i);
            }
            return if self.menu.mouse_move(x, y) {
                MenuBarEvent::Redraw
            } else {
                MenuBarEvent::Ignored
            };
        }
        if std::mem::replace(&mut self.hover, title) != title {
            MenuBarEvent::Redraw
        } else {
            MenuBarEvent::Ignored
        }
    }

    pub fn mouse_leave(&mut self) -> bool {
        let hover = self.hover.take().is_some();
        self.menu.mouse_move(-1.0, -1.0) || hover
    }

    /// Scrolls an open menu that is taller than the window.
    pub fn scroll(&mut self, rows: i32) -> bool {
        self.menu.scroll(rows)
    }

    /// Draws the bar. The open menu is drawn later, above everything, by
    /// [`MenuBar::draw_menu`].
    pub fn draw(&mut self, p: &mut Painter, area: Rect) {
        self.area = area;
        self.rects.clear();
        if area.is_empty() {
            return;
        }
        let t = p.theme;
        p.rect(area, t.chrome_bg);
        let style = p.style(FONT);
        let pad = p.px(PAD);
        let inset = p.px(3.0);
        let baseline = p.center_baseline(area.y, area.h, &style);
        let open = self.open_index();
        let mut x = area.x + p.px(4.0);
        for (i, title) in self.titles.iter().enumerate() {
            let w = p.measure(title, &style).ceil() as i32 + 2 * pad;
            let r = Rect::new(x, area.y, w, area.h);
            let highlight = Rect::new(r.x, r.y + inset, r.w, (r.h - 2 * inset).max(0));
            if open == Some(i) {
                p.rect(highlight, t.selection);
            } else if open.is_none() && self.hover == Some(i) {
                p.rect(highlight, t.line_highlight);
            }
            p.text(title, &style, (x + pad) as f32, baseline, t.chrome_text);
            if self.keyboard
                && let Some(first) = title.chars().next()
            {
                let first_w = p.measure(first.encode_utf8(&mut [0; 4]), &style);
                p.rect(
                    Rect::new(
                        x + pad,
                        baseline + p.px(2.0),
                        first_w.round() as i32,
                        p.px(1.0).max(1),
                    ),
                    t.chrome_text,
                );
            }
            self.rects.push(r);
            x += w;
        }
    }

    pub fn draw_menu(&mut self, p: &mut Painter) {
        self.menu.draw(p);
    }

    /// Describes the bar for assistive technologies: the titles, and under the open one,
    /// its menu.
    pub fn access(&self, tree: &mut AccessTree) -> NodeId {
        let open = self.open_index();
        let mut titles = Vec::new();
        for (i, (title, rect)) in self.titles.iter().zip(&self.rects).enumerate() {
            let mut node = access::clickable(Role::MenuItem, title, *rect);
            node.set_has_popup(HasPopup::Menu);
            node.set_expanded(open == Some(i));
            if let Some(first) = title.chars().next() {
                node.set_access_key(first.to_string());
            }
            if open == Some(i)
                && let Some(menu) = self.menu.access(tree, title)
            {
                node.set_children(vec![menu]);
            }
            titles.push(tree.push(Part::MenuTitle(i), node));
        }
        let mut bar = access::labeled(Role::MenuBar, "Menu bar", self.area);
        bar.set_children(titles);
        tree.push(Part::MenuBar, bar)
    }

    /// What has focus in the bar: the open menu's selected item, or the menu.
    pub fn access_focus(&self) -> Option<Part> {
        self.open_index().and(self.menu.access_focus())
    }

    /// Does what an assistive technology asks: opens or closes a menu (as from the
    /// keyboard, so its first item is selected), or runs or selects an item.
    pub fn access_action(&mut self, part: Part, request: &ActionRequest) -> MenuBarEvent {
        match part {
            Part::MenuTitle(i) if i < self.titles.len() => match request.action {
                AkAction::Click | AkAction::Expand if self.open_index() != Some(i) => {
                    self.keyboard = true;
                    MenuBarEvent::Open(i)
                }
                AkAction::Click | AkAction::Collapse if self.open_index() == Some(i) => {
                    self.close();
                    MenuBarEvent::Redraw
                }
                _ => MenuBarEvent::Ignored,
            },
            Part::MenuItem(_) if self.is_open() => {
                let action = self.menu.access_action(part, request);
                if !self.menu.is_open() {
                    self.close();
                }
                match action {
                    Some(action) => MenuBarEvent::Run(action),
                    None => MenuBarEvent::Redraw,
                }
            }
            _ => MenuBarEvent::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SUMI, commands};
    use fude_platform::Modifiers;

    fn bar() -> (MenuBar, Painter) {
        let titles: Vec<&str> = menus(&[]).iter().map(|m| m.title).collect();
        let mut bar = MenuBar::new(titles);
        let mut p = Painter::new(&SUMI);
        p.begin(1.0);
        bar.draw(&mut p, Rect::new(0, 0, 800, 28));
        (bar, p)
    }

    fn items() -> Vec<ContextMenuItem> {
        vec![
            ContextMenuItem::command(Action::Undo, "Undo", "Ctrl+Z", false),
            ContextMenuItem::separator(),
            ContextMenuItem::command(Action::Copy, "Copy", "Ctrl+C", true),
        ]
    }

    fn press(key: Key, modifiers: Modifiers) -> KeyInput {
        let base = match &key {
            Key::Character(c) => Some(c.clone()),
            _ => None,
        };
        KeyInput {
            key,
            pressed: true,
            repeat: false,
            modifiers,
            text: None,
            base,
        }
    }

    fn center(r: Rect) -> (f32, f32) {
        ((r.x + r.w / 2) as f32, (r.y + r.h / 2) as f32)
    }

    const WINDOW: Rect = Rect {
        x: 0,
        y: 0,
        w: 800,
        h: 600,
    };

    #[test]
    fn every_command_is_in_a_menu_once() {
        let menus = menus(&["jq"]);
        let actions: Vec<Action> = menus
            .iter()
            .flat_map(|m| &m.items)
            .filter_map(|item| match item {
                MenuEntry::Command { action, .. } => Some(*action),
                MenuEntry::Separator => None,
            })
            .collect();
        for (action, title, _) in commands() {
            let n = actions.iter().filter(|&&a| a == action).count();
            assert_eq!(n, 1, "{title} is in {n} menus");
        }
        assert!(actions.contains(&Action::External(0)));
        // Separators only between items.
        for menu in &menus {
            assert!(!matches!(menu.items.first(), Some(MenuEntry::Separator)));
            assert!(!matches!(menu.items.last(), Some(MenuEntry::Separator)));
            assert!(
                !menu
                    .items
                    .windows(2)
                    .any(|w| w.iter().all(|i| *i == MenuEntry::Separator))
            );
        }
    }

    #[test]
    fn access_keys_are_unique_first_letters() {
        let menus = menus(&[]);
        let mut letters: Vec<char> = menus
            .iter()
            .map(|m| m.title.chars().next().unwrap().to_ascii_lowercase())
            .collect();
        letters.sort_unstable();
        letters.dedup();
        assert_eq!(letters.len(), menus.len());
    }

    #[test]
    fn clicking_opens_toggles_and_moving_switches_menus() {
        let (mut bar, mut p) = bar();
        let (x0, y0) = center(bar.rects[0]);
        assert_eq!(bar.mouse_down(x0, y0), MenuBarEvent::Open(0));
        bar.open(&mut p, WINDOW, 0, items());
        assert!(bar.is_open());
        // Below the bar, at the title, nothing selected when opened by the mouse.
        assert_eq!(bar.menu.selected_index(), None);
        assert!(bar.menu_contains(x0, 40.0));
        assert!(!bar.menu_contains(x0, 20.0));

        let (x1, y1) = center(bar.rects[1]);
        assert_eq!(bar.mouse_move(x1, y1), MenuBarEvent::Open(1));
        bar.open(&mut p, WINDOW, 1, items());
        assert_eq!(bar.open_index(), Some(1));
        assert_eq!(bar.mouse_down(x1, y1), MenuBarEvent::Redraw);
        assert!(!bar.is_open());
        assert_eq!(bar.mouse_down(x1, 300.0), MenuBarEvent::Ignored);
    }

    #[test]
    fn clicks_in_the_menu_run_enabled_items_and_outside_close_it() {
        let (mut bar, mut p) = bar();
        bar.open(&mut p, WINDOW, 0, items());
        let undo = bar.menu.row_area(0);
        let (x, y) = center(undo);
        assert_eq!(bar.mouse_down(x, y), MenuBarEvent::Redraw);
        assert!(bar.is_open(), "a disabled item does nothing");
        let (x, y) = center(bar.menu.row_area(2));
        assert_eq!(bar.mouse_down(x, y), MenuBarEvent::Run(Action::Copy));
        assert!(!bar.is_open());

        bar.open(&mut p, WINDOW, 0, items());
        assert_eq!(bar.mouse_down(700.0, 500.0), MenuBarEvent::Redraw);
        assert!(!bar.is_open());
    }

    #[test]
    fn keyboard_opens_moves_across_and_runs() {
        let (mut bar, mut p) = bar();
        let none = Modifiers::default();
        let alt = Modifiers { alt: true, ..none };
        assert_eq!(
            bar.activation(&press(Key::Character("e".into()), none)),
            MenuBarEvent::Ignored
        );
        assert_eq!(
            bar.activation(&press(Key::Character("e".into()), alt)),
            MenuBarEvent::Open(1)
        );
        assert_eq!(
            bar.activation(&press(Key::F(10), none)),
            MenuBarEvent::Open(0)
        );
        bar.open(&mut p, WINDOW, 0, items());
        // Opened from the keyboard: the first item that can run is selected.
        assert_eq!(bar.menu.selected_index(), Some(2));
        assert_eq!(bar.key(&press(Key::Left, none)), MenuBarEvent::Open(6));
        bar.open(&mut p, WINDOW, 6, items());
        assert_eq!(bar.key(&press(Key::Right, none)), MenuBarEvent::Open(0));
        bar.open(&mut p, WINDOW, 0, items());
        assert_eq!(
            bar.key(&press(Key::Character("h".into()), alt)),
            MenuBarEvent::Open(6)
        );
        assert_eq!(
            bar.key(&press(Key::Enter, none)),
            MenuBarEvent::Run(Action::Copy)
        );
        assert!(!bar.is_open());

        bar.activation(&press(Key::F(10), none));
        bar.open(&mut p, WINDOW, 0, items());
        assert_eq!(bar.key(&press(Key::Escape, none)), MenuBarEvent::Redraw);
        assert!(!bar.is_open());
        bar.activation(&press(Key::F(10), none));
        bar.open(&mut p, WINDOW, 0, items());
        assert_eq!(bar.key(&press(Key::F(10), none)), MenuBarEvent::Redraw);
        assert!(!bar.is_open());
    }

    #[test]
    fn a_tall_menu_stays_below_the_bar() {
        let (mut bar, mut p) = bar();
        let tall: Vec<ContextMenuItem> = (0..60)
            .map(|_| ContextMenuItem::command(Action::Copy, "Copy", "", true))
            .collect();
        bar.open(&mut p, Rect::new(0, 0, 800, 400), 5, tall);
        let area = bar.menu.area();
        assert_eq!(area.y, 28);
        assert_eq!(area.bottom(), 400);
    }
}

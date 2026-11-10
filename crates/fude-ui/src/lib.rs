//! fude's UI: the editor view, the window chrome and dialogs, drawn into a
//! `fude-render` draw list with one theme.

mod chrome;
mod context_menu;
mod display;
mod find;
mod fuzzy;
mod input;
mod keymap;
mod menu_bar;
mod painter;
mod palette;
mod pieces;
mod prompt;
mod tabs;
mod theme;
mod view;

pub use chrome::{STATUS_BAR_HEIGHT, StatusMessage, status_bar};
pub use context_menu::{ContextMenu, ContextMenuItem};
pub use find::{FindAction, FindBar, FindField, FindStatus};
pub use input::{ImeChange, InputKey, TextInput};
pub use keymap::{Action, Shortcut, action_for, commands, shortcut, shortcut_label};
pub use menu_bar::{MENU_BAR_HEIGHT, Menu, MenuBar, MenuBarEvent, MenuEntry, menus};
pub use painter::Painter;
pub use palette::{Palette, PaletteAction, PaletteItem};
pub use prompt::{Prompt, PromptInput};
pub use tabs::{TAB_STRIP_HEIGHT, TabEvent, TabInfo, TabStrip};
pub use theme::{SUMI, Theme, WASHI};
pub use view::{DEFAULT_FONT_SIZE, EditorView, Highlights, ScrollPosition};

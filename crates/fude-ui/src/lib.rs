//! fude's UI: the editor view, the window chrome and dialogs, drawn into a
//! `fude-render` draw list with one theme.

mod chrome;
mod display;
mod find;
mod fuzzy;
mod input;
mod keymap;
mod painter;
mod palette;
mod prompt;
mod tabs;
mod theme;
mod view;

pub use chrome::{STATUS_BAR_HEIGHT, StatusMessage, status_bar};
pub use find::{FindAction, FindBar, FindField, FindStatus};
pub use input::{InputKey, TextInput};
pub use keymap::{Action, action_for, commands};
pub use painter::Painter;
pub use palette::{Palette, PaletteAction, PaletteItem};
pub use prompt::{Prompt, PromptInput};
pub use tabs::{TAB_STRIP_HEIGHT, TabEvent, TabInfo, TabStrip};
pub use theme::{SUMI, Theme, WASHI};
pub use view::{DEFAULT_FONT_SIZE, EditorView, Highlights};

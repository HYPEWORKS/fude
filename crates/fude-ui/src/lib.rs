//! fude's UI: the editor view, the window chrome and dialogs, drawn into a
//! `fude-render` draw list with one theme.

mod chrome;
mod display;
mod keymap;
mod painter;
mod prompt;
mod theme;
mod view;

pub use chrome::{STATUS_BAR_HEIGHT, StatusMessage, TAB_STRIP_HEIGHT, status_bar, tab_strip};
pub use keymap::{Action, action_for};
pub use painter::Painter;
pub use prompt::{Prompt, PromptInput};
pub use theme::{SUMI, Theme};
pub use view::{DEFAULT_FONT_SIZE, EditorView};

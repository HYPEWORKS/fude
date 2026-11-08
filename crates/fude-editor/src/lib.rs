//! Editing, independent of any UI: selections, cursor movement and editing commands on
//! top of a `fude-buffer` buffer. The view supplies horizontal geometry through
//! [`Layout`], so everything here is testable without fonts or a window.

mod editor;
mod layout;
pub mod search;
mod selection;
pub mod text;

pub use editor::{Editor, Motion, Settings};
pub use layout::{Layout, MonoLayout, nearest};
pub use search::{Query, Searcher};
pub use selection::{Selection, Selections};
pub use text::WordStop;

#[cfg(test)]
mod tests;

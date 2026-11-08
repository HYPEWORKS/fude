//! Shortcuts for actions beyond moving and typing (which [`crate::EditorView`] handles).
//! Every action has a stable name, ready for the command palette and user keymaps.

use fude_platform::{Key, KeyInput};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    New,
    Open,
    Save,
    SaveAs,
    Close,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    ZoomIn,
    ZoomOut,
    ZoomReset,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::New => "file.new",
            Action::Open => "file.open",
            Action::Save => "file.save",
            Action::SaveAs => "file.save_as",
            Action::Close => "file.close",
            Action::Quit => "app.quit",
            Action::Undo => "edit.undo",
            Action::Redo => "edit.redo",
            Action::Cut => "edit.cut",
            Action::Copy => "edit.copy",
            Action::Paste => "edit.paste",
            Action::SelectAll => "edit.select_all",
            Action::ZoomIn => "view.zoom_in",
            Action::ZoomOut => "view.zoom_out",
            Action::ZoomReset => "view.zoom_reset",
        }
    }
}

/// The action a key press triggers, if any: Ctrl+key, or Cmd+key on macOS.
pub fn action_for(key: &KeyInput) -> Option<Action> {
    if !key.pressed {
        return None;
    }
    let m = key.modifiers;
    if !m.primary() {
        // Shift+Delete / Shift+Insert / Ctrl+Insert, the old clipboard keys.
        return match (&key.key, m.shift, m.ctrl) {
            (Key::Delete, true, false) => Some(Action::Cut),
            (Key::Insert, true, false) => Some(Action::Paste),
            _ => None,
        };
    }
    let Key::Character(c) = &key.key else {
        return match key.key {
            Key::Insert => Some(Action::Copy),
            _ => None,
        };
    };
    let shift = m.shift;
    Some(match c.to_lowercase().as_str() {
        "n" => Action::New,
        "o" => Action::Open,
        "s" if shift => Action::SaveAs,
        "s" => Action::Save,
        "w" => Action::Close,
        "q" => Action::Quit,
        "z" if shift => Action::Redo,
        "z" => Action::Undo,
        "y" if !cfg!(target_os = "macos") => Action::Redo,
        "x" => Action::Cut,
        "c" => Action::Copy,
        "v" => Action::Paste,
        "a" => Action::SelectAll,
        "=" | "+" => Action::ZoomIn,
        "-" | "_" => Action::ZoomOut,
        "0" => Action::ZoomReset,
        _ => return None,
    })
}

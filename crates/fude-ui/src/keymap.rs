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
    NextTab,
    PreviousTab,
    /// Ctrl+1 to Ctrl+9: the nth tab (9 is always the last).
    GoToTab(u8),
    OpenSettings,
    Find,
    Replace,
    FindNext,
    FindPrevious,
    CommandPalette,
    GoToLine,
    ToggleWrap,
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
            Action::NextTab => "view.next_tab",
            Action::PreviousTab => "view.previous_tab",
            Action::GoToTab(_) => "view.go_to_tab",
            Action::OpenSettings => "app.settings",
            Action::Find => "find.find",
            Action::Replace => "find.replace",
            Action::FindNext => "find.next",
            Action::FindPrevious => "find.previous",
            Action::CommandPalette => "app.command_palette",
            Action::GoToLine => "edit.go_to_line",
            Action::ToggleWrap => "view.toggle_wrap",
        }
    }
}

/// The action a key press triggers, if any: Ctrl+key, or Cmd+key on macOS.
pub fn action_for(key: &KeyInput) -> Option<Action> {
    if !key.pressed {
        return None;
    }
    let m = key.modifiers;
    if key.key == Key::F(1) {
        return Some(Action::CommandPalette);
    }
    if key.key == Key::F(3) {
        return Some(if m.shift {
            Action::FindPrevious
        } else {
            Action::FindNext
        });
    }
    // Ctrl+Tab and Ctrl+PageUp/PageDown switch tabs on every platform.
    if m.ctrl {
        match key.key {
            Key::Tab if m.shift => return Some(Action::PreviousTab),
            Key::Tab => return Some(Action::NextTab),
            Key::PageDown => return Some(Action::NextTab),
            Key::PageUp => return Some(Action::PreviousTab),
            _ => {}
        }
    }
    // Alt+Z toggles word wrap, as in VS Code.
    if m.alt && !m.ctrl && !m.logo && key.key == Key::Character("z".into()) {
        return Some(Action::ToggleWrap);
    }
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
        "," => Action::OpenSettings,
        // Replace is Ctrl+H, or Cmd+Option+F on macOS (where Cmd+H hides the app).
        "f" if m.alt => Action::Replace,
        "f" => Action::Find,
        "h" if !cfg!(target_os = "macos") => Action::Replace,
        "p" if shift => Action::CommandPalette,
        // Go to line: Ctrl+G, or Cmd+L on macOS, where Cmd+G finds the next match.
        "g" if !cfg!(target_os = "macos") => Action::GoToLine,
        "l" if cfg!(target_os = "macos") => Action::GoToLine,
        "g" if shift => Action::FindPrevious,
        "g" => Action::FindNext,
        d if d.len() == 1 && ('1'..='9').contains(&d.chars().next().unwrap_or('0')) => {
            Action::GoToTab(d.as_bytes()[0] - b'0')
        }
        _ => return None,
    })
}

/// Every command for the palette: its action, a title, and its shortcut on this platform.
pub fn commands() -> Vec<(Action, &'static str, String)> {
    let mac = cfg!(target_os = "macos");
    let key = |k: &str| {
        if mac {
            format!("⌘{}", k.replace("Shift+", "⇧").replace("Alt+", "⌥"))
        } else {
            format!("Ctrl+{k}")
        }
    };
    vec![
        (Action::New, "File: New", key("N")),
        (Action::Open, "File: Open…", key("O")),
        (Action::Save, "File: Save", key("S")),
        (Action::SaveAs, "File: Save As…", key("Shift+S")),
        (Action::Close, "File: Close Tab", key("W")),
        (Action::Quit, "Quit", key("Q")),
        (Action::Undo, "Edit: Undo", key("Z")),
        (
            Action::Redo,
            "Edit: Redo",
            if mac { key("Shift+Z") } else { key("Y") },
        ),
        (Action::Cut, "Edit: Cut", key("X")),
        (Action::Copy, "Edit: Copy", key("C")),
        (Action::Paste, "Edit: Paste", key("V")),
        (Action::SelectAll, "Edit: Select All", key("A")),
        (
            Action::GoToLine,
            "Go to Line…",
            if mac { key("L") } else { key("G") },
        ),
        (Action::Find, "Find…", key("F")),
        (
            Action::Replace,
            "Replace…",
            if mac { key("Alt+F") } else { key("H") },
        ),
        (Action::FindNext, "Find Next", "F3".to_owned()),
        (Action::FindPrevious, "Find Previous", "Shift+F3".to_owned()),
        (Action::NextTab, "View: Next Tab", "Ctrl+Tab".to_owned()),
        (
            Action::PreviousTab,
            "View: Previous Tab",
            "Ctrl+Shift+Tab".to_owned(),
        ),
        (
            Action::ToggleWrap,
            "View: Toggle Word Wrap",
            if mac {
                "⌥Z".to_owned()
            } else {
                "Alt+Z".to_owned()
            },
        ),
        (Action::ZoomIn, "View: Zoom In", key("=")),
        (Action::ZoomOut, "View: Zoom Out", key("-")),
        (Action::ZoomReset, "View: Reset Zoom", key("0")),
        (Action::OpenSettings, "Preferences: Open Settings", key(",")),
    ]
}

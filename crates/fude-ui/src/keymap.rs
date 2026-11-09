//! Shortcuts for actions beyond moving and typing (which [`crate::EditorView`] handles).
//! Every action has a stable name, ready for the command palette and user keymaps.

use fude_buffer::LineEnding;
use fude_platform::{Key, KeyInput};
use fude_tools::Tool;

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
    SelectNextOccurrence,
    SelectAllOccurrences,
    SplitIntoLines,
    AddCursorAbove,
    AddCursorBelow,
    GoToBracket,
    DuplicateLine,
    MoveLineUp,
    MoveLineDown,
    DeleteLine,
    JoinLines,
    InsertLineBelow,
    InsertLineAbove,
    Indent,
    Outdent,
    ToggleComment,
    ToggleBlockComment,
    /// A built-in text tool.
    Tool(Tool),
    /// Converts every line break.
    LineEndings(LineEnding),
    /// An external tool from the settings, by its index there.
    External(u16),
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
            Action::SelectNextOccurrence => "selection.add_next_occurrence",
            Action::SelectAllOccurrences => "selection.all_occurrences",
            Action::SplitIntoLines => "selection.split_into_lines",
            Action::AddCursorAbove => "selection.add_cursor_above",
            Action::AddCursorBelow => "selection.add_cursor_below",
            Action::GoToBracket => "edit.go_to_bracket",
            Action::DuplicateLine => "line.duplicate",
            Action::MoveLineUp => "line.move_up",
            Action::MoveLineDown => "line.move_down",
            Action::DeleteLine => "line.delete",
            Action::JoinLines => "line.join",
            Action::InsertLineBelow => "line.insert_below",
            Action::InsertLineAbove => "line.insert_above",
            Action::Indent => "line.indent",
            Action::Outdent => "line.outdent",
            Action::ToggleComment => "edit.toggle_comment",
            Action::ToggleBlockComment => "edit.toggle_block_comment",
            Action::Tool(tool) => tool.name(),
            Action::LineEndings(LineEnding::Lf) => "format.line_endings_lf",
            Action::LineEndings(LineEnding::CrLf) => "format.line_endings_crlf",
            Action::LineEndings(LineEnding::Cr) => "format.line_endings_cr",
            Action::External(_) => "tools.external",
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
    let mac = cfg!(target_os = "macos");
    match key.key {
        // Alt+Up/Down move lines; Ctrl+Alt+Up/Down (Cmd+Option on macOS) add cursors.
        Key::Up | Key::Down => {
            let down = key.key == Key::Down;
            let add_cursor = if mac {
                m.logo && m.alt
            } else {
                m.ctrl && m.alt
            };
            if add_cursor && !m.shift {
                return Some(if down {
                    Action::AddCursorBelow
                } else {
                    Action::AddCursorAbove
                });
            }
            if m.alt && !m.ctrl && !m.logo && !m.shift {
                return Some(if down {
                    Action::MoveLineDown
                } else {
                    Action::MoveLineUp
                });
            }
        }
        // Ctrl+Enter opens a line below, Ctrl+Shift+Enter above.
        Key::Enter if m.primary() && !m.alt => {
            return Some(if m.shift {
                Action::InsertLineAbove
            } else {
                Action::InsertLineBelow
            });
        }
        _ => {}
    }
    // Alt+Z toggles word wrap (as in VS Code), Shift+Alt+A block comments, Shift+Alt+I
    // splits the selection into lines.
    if m.alt && !m.ctrl && !m.logo {
        let c = key.shortcut_char().map(str::to_lowercase);
        match (c.as_deref(), m.shift) {
            (Some("z"), false) => return Some(Action::ToggleWrap),
            (Some("a"), true) => return Some(Action::ToggleBlockComment),
            (Some("i"), true) => return Some(Action::SplitIntoLines),
            _ => {}
        }
    }
    if !m.primary() {
        // Shift+Delete / Shift+Insert / Ctrl+Insert, the old clipboard keys.
        return match (&key.key, m.shift, m.ctrl) {
            (Key::Delete, true, false) => Some(Action::Cut),
            (Key::Insert, true, false) => Some(Action::Paste),
            _ => None,
        };
    }
    let Some(c) = key.shortcut_char() else {
        return match key.key {
            Key::Insert => Some(Action::Copy),
            _ => None,
        };
    };
    let shift = m.shift;
    Some(match c.to_lowercase().as_str() {
        "d" if shift => Action::DuplicateLine,
        "d" => Action::SelectNextOccurrence,
        "l" if shift => Action::SelectAllOccurrences,
        "k" if shift => Action::DeleteLine,
        "j" => Action::JoinLines,
        "/" => Action::ToggleComment,
        "]" => Action::Indent,
        "[" => Action::Outdent,
        "\\" | "|" if shift => Action::GoToBracket,
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
pub fn commands() -> Vec<(Action, String, String)> {
    let mac = cfg!(target_os = "macos");
    let key = |k: &str| {
        if mac {
            format!("⌘{}", k.replace("Shift+", "⇧").replace("Alt+", "⌥"))
        } else {
            format!("Ctrl+{k}")
        }
    };
    let alt = |k: &str| {
        if mac {
            format!("⌥{}", k.replace("Shift+", "⇧"))
        } else {
            format!("Alt+{k}")
        }
    };
    let ctrl_alt = |k: &str| {
        if mac {
            format!("⌥⌘{k}")
        } else {
            format!("Ctrl+Alt+{k}")
        }
    };
    let mut out: Vec<(Action, String, String)> = vec![
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
        (Action::SelectAll, "Selection: Select All", key("A")),
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
        (
            Action::SelectNextOccurrence,
            "Selection: Add Next Occurrence",
            key("D"),
        ),
        (
            Action::SelectAllOccurrences,
            "Selection: Select All Occurrences",
            key("Shift+L"),
        ),
        (
            Action::SplitIntoLines,
            "Selection: Split into Lines",
            alt("Shift+I"),
        ),
        (
            Action::AddCursorAbove,
            "Selection: Add Cursor Above",
            ctrl_alt("↑"),
        ),
        (
            Action::AddCursorBelow,
            "Selection: Add Cursor Below",
            ctrl_alt("↓"),
        ),
        (
            Action::GoToBracket,
            "Go to Matching Bracket",
            key("Shift+\\"),
        ),
        (Action::DuplicateLine, "Line: Duplicate", key("Shift+D")),
        (Action::MoveLineUp, "Line: Move Up", alt("↑")),
        (Action::MoveLineDown, "Line: Move Down", alt("↓")),
        (Action::DeleteLine, "Line: Delete", key("Shift+K")),
        (Action::JoinLines, "Line: Join", key("J")),
        (Action::InsertLineBelow, "Line: Insert Below", key("Enter")),
        (
            Action::InsertLineAbove,
            "Line: Insert Above",
            key("Shift+Enter"),
        ),
        (Action::Indent, "Line: Indent", key("]")),
        (Action::Outdent, "Line: Outdent", key("[")),
        (Action::ToggleComment, "Toggle Line Comment", key("/")),
        (
            Action::ToggleBlockComment,
            "Toggle Block Comment",
            alt("Shift+A"),
        ),
        (
            Action::LineEndings(LineEnding::Lf),
            "Line Endings: Convert to LF (Unix, macOS)",
            String::new(),
        ),
        (
            Action::LineEndings(LineEnding::CrLf),
            "Line Endings: Convert to CRLF (Windows)",
            String::new(),
        ),
        (
            Action::LineEndings(LineEnding::Cr),
            "Line Endings: Convert to CR (Classic Mac)",
            String::new(),
        ),
    ]
    .into_iter()
    .map(|(a, t, k)| (a, t.to_owned(), k))
    .collect();
    out.extend(Tool::ALL.iter().map(|&t| {
        (
            Action::Tool(t),
            format!("Tools: {}", t.title()),
            String::new(),
        )
    }));
    out
}

/// A key combination from the settings, such as `"ctrl+alt+j"` or `"shift+f5"`. `mod` is
/// Ctrl, or Cmd on macOS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    ctrl: bool,
    alt: bool,
    shift: bool,
    logo: bool,
    key: ShortcutKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ShortcutKey {
    /// A character, lower case.
    Char(String),
    Named(Key),
}

impl Shortcut {
    pub fn parse(s: &str) -> Result<Shortcut, String> {
        let mac = cfg!(target_os = "macos");
        let mut out = Shortcut {
            ctrl: false,
            alt: false,
            shift: false,
            logo: false,
            key: ShortcutKey::Named(Key::Other),
        };
        let parts: Vec<String> = s.split('+').map(|p| p.trim().to_lowercase()).collect();
        let Some((last, mods)) = parts.split_last() else {
            return Err("an empty key".into());
        };
        for m in mods {
            match m.as_str() {
                "ctrl" | "control" => out.ctrl = true,
                "alt" | "option" | "opt" => out.alt = true,
                "shift" => out.shift = true,
                "cmd" | "command" | "super" | "win" | "meta" | "logo" => out.logo = true,
                "mod" | "primary" if mac => out.logo = true,
                "mod" | "primary" => out.ctrl = true,
                other => return Err(format!("unknown modifier {other:?} in {s:?}")),
            }
        }
        out.key = match last.as_str() {
            "enter" | "return" => ShortcutKey::Named(Key::Enter),
            "tab" => ShortcutKey::Named(Key::Tab),
            "space" => ShortcutKey::Char(" ".into()),
            "escape" | "esc" => ShortcutKey::Named(Key::Escape),
            "backspace" => ShortcutKey::Named(Key::Backspace),
            "delete" | "del" => ShortcutKey::Named(Key::Delete),
            "insert" | "ins" => ShortcutKey::Named(Key::Insert),
            "home" => ShortcutKey::Named(Key::Home),
            "end" => ShortcutKey::Named(Key::End),
            "pageup" => ShortcutKey::Named(Key::PageUp),
            "pagedown" => ShortcutKey::Named(Key::PageDown),
            "up" => ShortcutKey::Named(Key::Up),
            "down" => ShortcutKey::Named(Key::Down),
            "left" => ShortcutKey::Named(Key::Left),
            "right" => ShortcutKey::Named(Key::Right),
            f if f.len() > 1
                && f.starts_with('f')
                && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) =>
            {
                ShortcutKey::Named(Key::F(f[1..].parse().expect("checked")))
            }
            c if c.chars().count() == 1 => ShortcutKey::Char(c.to_owned()),
            other => return Err(format!("unknown key {other:?} in {s:?}")),
        };
        Ok(out)
    }

    pub fn matches(&self, key: &KeyInput) -> bool {
        let m = key.modifiers;
        if !key.pressed
            || (m.ctrl, m.alt, m.shift, m.logo) != (self.ctrl, self.alt, self.shift, self.logo)
        {
            return false;
        }
        match &self.key {
            ShortcutKey::Named(k) => key.key == *k,
            ShortcutKey::Char(c) => key.shortcut_char().is_some_and(|s| s.to_lowercase() == *c),
        }
    }

    /// For the command palette: "Ctrl+Alt+J", or "⌃⌥J" on macOS.
    pub fn label(&self) -> String {
        let mac = cfg!(target_os = "macos");
        let key = match &self.key {
            ShortcutKey::Char(c) if c == " " => "Space".to_owned(),
            ShortcutKey::Char(c) => c.to_uppercase(),
            ShortcutKey::Named(Key::F(n)) => format!("F{n}"),
            ShortcutKey::Named(k) => format!("{k:?}"),
        };
        let mods = [
            (self.ctrl, "Ctrl+", "⌃"),
            (self.alt, "Alt+", "⌥"),
            (self.shift, "Shift+", "⇧"),
            (self.logo, if mac { "Cmd+" } else { "Super+" }, "⌘"),
        ];
        let mut out: String = mods
            .iter()
            .filter(|m| m.0)
            .map(|m| if mac { m.2 } else { m.1 })
            .collect();
        out.push_str(&key);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fude_platform::Modifiers;

    fn press(key: Key, base: Option<&str>, m: Modifiers) -> KeyInput {
        KeyInput {
            key,
            pressed: true,
            repeat: false,
            modifiers: m,
            text: None,
            base: base.map(str::to_owned),
        }
    }

    #[test]
    fn shortcuts_parse_and_match() {
        let s = Shortcut::parse("Ctrl+Alt+J").unwrap();
        let m = Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::default()
        };
        assert!(s.matches(&press(Key::Character("j".into()), Some("j"), m)));
        assert!(!s.matches(&press(
            Key::Character("j".into()),
            Some("j"),
            Modifiers::default()
        )));
        let f5 = Shortcut::parse("shift+f5").unwrap();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert!(f5.matches(&press(Key::F(5), None, shift)));
        assert!(Shortcut::parse("hyper+x").is_err());
        assert!(Shortcut::parse("ctrl+banana").is_err());
        if !cfg!(target_os = "macos") {
            assert_eq!(s.label(), "Ctrl+Alt+J");
        }
    }

    #[test]
    fn new_editing_shortcuts() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        let ctrl_shift = Modifiers {
            shift: true,
            ..ctrl
        };
        let alt = Modifiers {
            alt: true,
            ..Modifiers::default()
        };
        if cfg!(target_os = "macos") {
            return;
        }
        let ch = |c: &str, shown: &str, m| press(Key::Character(shown.into()), Some(c), m);
        assert_eq!(
            action_for(&ch("d", "d", ctrl)),
            Some(Action::SelectNextOccurrence)
        );
        assert_eq!(
            action_for(&ch("d", "D", ctrl_shift)),
            Some(Action::DuplicateLine)
        );
        // Shift turns \ into | on US layouts; shortcuts go by the unshifted key.
        assert_eq!(
            action_for(&ch("\\", "|", ctrl_shift)),
            Some(Action::GoToBracket)
        );
        assert_eq!(action_for(&ch("/", "/", ctrl)), Some(Action::ToggleComment));
        assert_eq!(
            action_for(&press(Key::Up, None, alt)),
            Some(Action::MoveLineUp)
        );
        let ctrl_alt = Modifiers { alt: true, ..ctrl };
        assert_eq!(
            action_for(&press(Key::Down, None, ctrl_alt)),
            Some(Action::AddCursorBelow)
        );
        let shift_alt = Modifiers { shift: true, ..alt };
        assert_eq!(
            action_for(&ch("a", "A", shift_alt)),
            Some(Action::ToggleBlockComment)
        );
        assert_eq!(
            action_for(&press(Key::Up, None, shift_alt)),
            None,
            "column selection, in the view"
        );
    }
}

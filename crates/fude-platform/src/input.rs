use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    /// Cmd on macOS, the Windows key elsewhere.
    pub logo: bool,
}

impl Modifiers {
    /// The platform's primary shortcut modifier: Cmd on macOS, Ctrl elsewhere.
    pub fn primary(self) -> bool {
        if cfg!(target_os = "macos") {
            self.logo
        } else {
            self.ctrl
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Key {
    /// A key that produces a character, as laid out by the user's keyboard layout.
    Character(String),
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    /// F1 to F12.
    F(u8),
    Other,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct KeyInput {
    pub key: Key,
    pub pressed: bool,
    pub repeat: bool,
    pub modifiers: Modifiers,
    /// Text this key press would insert, if any.
    pub text: Option<String>,
    /// For a character key, the character it makes without modifiers (Shift, Option,
    /// AltGr) in the current layout: what shortcuts match, so Ctrl+Shift+\ is "\" and
    /// Option+Z on macOS is "z".
    pub base: Option<String>,
}

impl KeyInput {
    /// The key's character for shortcuts (see [`KeyInput::base`]).
    pub fn shortcut_char(&self) -> Option<&str> {
        match (&self.base, &self.key) {
            (Some(b), _) => Some(b),
            (None, Key::Character(c)) => Some(c),
            _ => None,
        }
    }
}

pub(crate) fn modifiers(m: ModifiersState) -> Modifiers {
    Modifiers {
        shift: m.shift_key(),
        ctrl: m.control_key(),
        alt: m.alt_key(),
        logo: m.super_key(),
    }
}

pub(crate) fn key_input(ev: &KeyEvent, modifiers: Modifiers) -> KeyInput {
    let key = match &ev.logical_key {
        WKey::Character(s) => Key::Character(s.to_string()),
        WKey::Named(named) => match named {
            NamedKey::Space => Key::Character(" ".into()),
            NamedKey::Enter => Key::Enter,
            NamedKey::Tab => Key::Tab,
            NamedKey::Escape => Key::Escape,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Insert => Key::Insert,
            NamedKey::F1 => Key::F(1),
            NamedKey::F2 => Key::F(2),
            NamedKey::F3 => Key::F(3),
            NamedKey::F4 => Key::F(4),
            NamedKey::F5 => Key::F(5),
            NamedKey::F6 => Key::F(6),
            NamedKey::F7 => Key::F(7),
            NamedKey::F8 => Key::F(8),
            NamedKey::F9 => Key::F(9),
            NamedKey::F10 => Key::F(10),
            NamedKey::F11 => Key::F(11),
            NamedKey::F12 => Key::F(12),
            _ => Key::Other,
        },
        _ => Key::Other,
    };
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    let base = match ev.key_without_modifiers() {
        WKey::Character(s) => Some(s.to_string()),
        _ => None,
    };
    KeyInput {
        key,
        pressed: ev.state == ElementState::Pressed,
        repeat: ev.repeat,
        modifiers,
        text: ev.text.as_ref().map(|t| t.to_string()),
        base,
    }
}

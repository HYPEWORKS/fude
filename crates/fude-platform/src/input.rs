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
            _ => Key::Other,
        },
        _ => Key::Other,
    };
    KeyInput {
        key,
        pressed: ev.state == ElementState::Pressed,
        repeat: ev.repeat,
        modifiers,
        text: ev.text.as_ref().map(|t| t.to_string()),
    }
}

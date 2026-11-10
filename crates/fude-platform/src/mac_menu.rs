//! The macOS menu bar, through `muda`.
//!
//! The app describes its menus once ([`MenuBarSpec`]); this adds what every Mac app has:
//! the application menu (About, Settings, Services, Hide, Quit) and the Window menu.
//! Chosen items arrive as [`crate::Event::Menu`]. Shortcuts become key equivalents, so
//! the menu bar handles them before the window sees the key; a disabled item lets its
//! key through to the window, where the app's own keymap handles it the same way.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::Sender;

use muda::accelerator::{Accelerator, Code, Modifiers as Mods};
use muda::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use winit::event_loop::EventLoopProxy;

use crate::{Key, MenuBarSpec, MenuCommand, MenuItemSpec, MenuRole, Modifiers};

/// Sends chosen items' ids to `events` and wakes the event loop. Set once per process.
pub(crate) fn listen(proxy: EventLoopProxy<()>, events: Sender<u32>) {
    let proxy = Mutex::new(proxy);
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let Ok(id) = event.id.0.parse::<u32>() else {
            return;
        };
        let _ = events.send(id);
        if let Ok(proxy) = proxy.lock() {
            let _ = proxy.send_event(());
        }
    }));
}

enum Item {
    Plain(MenuItem),
    Check(CheckMenuItem),
}

pub(crate) struct NativeMenu {
    /// Installed in the application while it lives.
    _menu: Menu,
    items: HashMap<u32, Item>,
}

impl NativeMenu {
    pub(crate) fn new(spec: &MenuBarSpec) -> NativeMenu {
        let menu = Menu::new();
        let mut items = HashMap::new();
        let role = |role: MenuRole| {
            spec.menus
                .iter()
                .flat_map(|m| &m.items)
                .find_map(|item| match item {
                    MenuItemSpec::Command(c) if c.role == role => Some(c),
                    _ => None,
                })
        };

        let app = Submenu::new(&spec.app_name, true);
        if let Some(about) = role(MenuRole::About) {
            add(&app, &*command(about, &mut items));
            add(&app, &PredefinedMenuItem::separator());
        }
        if let Some(settings) = role(MenuRole::Settings) {
            add(&app, &*command(settings, &mut items));
            add(&app, &PredefinedMenuItem::separator());
        }
        add(&app, &PredefinedMenuItem::services(None));
        add(&app, &PredefinedMenuItem::separator());
        add(&app, &PredefinedMenuItem::hide(None));
        add(&app, &PredefinedMenuItem::hide_others(None));
        add(&app, &PredefinedMenuItem::show_all(None));
        if let Some(quit) = role(MenuRole::Quit) {
            add(&app, &PredefinedMenuItem::separator());
            // The app's own Quit, not terminate:, so quitting saves the session.
            add(&app, &*command(quit, &mut items));
        }
        add(&menu, &app);

        let window = Submenu::new("Window", true);
        add(&window, &PredefinedMenuItem::minimize(None));
        add(&window, &PredefinedMenuItem::maximize(Some("Zoom")));
        add(&window, &PredefinedMenuItem::separator());
        add(&window, &PredefinedMenuItem::bring_all_to_front(None));

        let mut help = None;
        for spec_menu in &spec.menus {
            let submenu = Submenu::new(&spec_menu.title, true);
            // Without the items that moved to the application menu, and without the
            // separators that leaves at the ends or doubled.
            let mut pending_separator = false;
            let mut any = false;
            for item in &spec_menu.items {
                match item {
                    MenuItemSpec::Separator => pending_separator = any,
                    MenuItemSpec::Command(c) if c.role != MenuRole::Normal => {}
                    MenuItemSpec::Command(c) => {
                        if std::mem::take(&mut pending_separator) {
                            add(&submenu, &PredefinedMenuItem::separator());
                        }
                        add(&submenu, &*command(c, &mut items));
                        any = true;
                    }
                }
            }
            if !any {
                continue;
            }
            if spec_menu.title == "Help" {
                help = Some(submenu);
            } else {
                add(&menu, &submenu);
            }
        }
        add(&menu, &window);
        if let Some(help) = &help {
            add(&menu, help);
        }
        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        if let Some(help) = &help {
            help.set_as_help_menu_for_nsapp();
        }
        NativeMenu { _menu: menu, items }
    }

    pub(crate) fn set_state(&self, id: u32, enabled: bool, checked: Option<bool>) {
        match self.items.get(&id) {
            Some(Item::Plain(item)) => item.set_enabled(enabled),
            Some(Item::Check(item)) => {
                item.set_enabled(enabled);
                if let Some(checked) = checked {
                    item.set_checked(checked);
                }
            }
            None => {}
        }
    }
}

trait Container {
    fn append(&self, item: &dyn IsMenuItem) -> muda::Result<()>;
}

impl Container for Menu {
    fn append(&self, item: &dyn IsMenuItem) -> muda::Result<()> {
        Menu::append(self, item)
    }
}

impl Container for Submenu {
    fn append(&self, item: &dyn IsMenuItem) -> muda::Result<()> {
        Submenu::append(self, item)
    }
}

fn add(to: &impl Container, item: &dyn IsMenuItem) {
    if let Err(e) = to.append(item) {
        eprintln!("fude: couldn't add a menu item: {e}");
    }
}

/// Makes the item for a command and remembers it by id, for [`NativeMenu::set_state`].
fn command(c: &MenuCommand, items: &mut HashMap<u32, Item>) -> Box<dyn IsMenuItem> {
    // An ampersand marks a mnemonic in muda's labels.
    let label = c.label.replace('&', "&&");
    let accelerator = c.shortcut.as_ref().and_then(|(k, m)| accelerator(k, *m));
    let (item, boxed): (Item, Box<dyn IsMenuItem>) = match c.checked {
        Some(checked) => {
            let item = CheckMenuItem::with_id(c.id, label, c.enabled, checked, accelerator);
            (Item::Check(item.clone()), Box::new(item))
        }
        None => {
            let item = MenuItem::with_id(c.id, label, c.enabled, accelerator);
            (Item::Plain(item.clone()), Box::new(item))
        }
    };
    items.insert(c.id, item);
    boxed
}

fn accelerator(key: &Key, m: Modifiers) -> Option<Accelerator> {
    const LETTERS: [Code; 26] = [
        Code::KeyA,
        Code::KeyB,
        Code::KeyC,
        Code::KeyD,
        Code::KeyE,
        Code::KeyF,
        Code::KeyG,
        Code::KeyH,
        Code::KeyI,
        Code::KeyJ,
        Code::KeyK,
        Code::KeyL,
        Code::KeyM,
        Code::KeyN,
        Code::KeyO,
        Code::KeyP,
        Code::KeyQ,
        Code::KeyR,
        Code::KeyS,
        Code::KeyT,
        Code::KeyU,
        Code::KeyV,
        Code::KeyW,
        Code::KeyX,
        Code::KeyY,
        Code::KeyZ,
    ];
    const DIGITS: [Code; 10] = [
        Code::Digit0,
        Code::Digit1,
        Code::Digit2,
        Code::Digit3,
        Code::Digit4,
        Code::Digit5,
        Code::Digit6,
        Code::Digit7,
        Code::Digit8,
        Code::Digit9,
    ];
    const F_KEYS: [Code; 12] = [
        Code::F1,
        Code::F2,
        Code::F3,
        Code::F4,
        Code::F5,
        Code::F6,
        Code::F7,
        Code::F8,
        Code::F9,
        Code::F10,
        Code::F11,
        Code::F12,
    ];
    let code = match key {
        Key::Character(s) => {
            let mut chars = s.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            match c.to_ascii_lowercase() {
                c @ 'a'..='z' => LETTERS[(c as u8 - b'a') as usize],
                c @ '0'..='9' => DIGITS[(c as u8 - b'0') as usize],
                '=' => Code::Equal,
                '-' => Code::Minus,
                ',' => Code::Comma,
                '.' => Code::Period,
                '/' => Code::Slash,
                '\\' => Code::Backslash,
                '[' => Code::BracketLeft,
                ']' => Code::BracketRight,
                ';' => Code::Semicolon,
                '\'' => Code::Quote,
                '`' => Code::Backquote,
                ' ' => Code::Space,
                _ => return None,
            }
        }
        Key::Enter => Code::Enter,
        Key::Tab => Code::Tab,
        Key::Escape => Code::Escape,
        Key::Backspace => Code::Backspace,
        Key::Delete => Code::Delete,
        Key::Left => Code::ArrowLeft,
        Key::Right => Code::ArrowRight,
        Key::Up => Code::ArrowUp,
        Key::Down => Code::ArrowDown,
        Key::Home => Code::Home,
        Key::End => Code::End,
        Key::PageUp => Code::PageUp,
        Key::PageDown => Code::PageDown,
        Key::Insert => Code::Insert,
        Key::F(n @ 1..=12) => F_KEYS[usize::from(*n) - 1],
        _ => return None,
    };
    let mut mods = Mods::empty();
    mods.set(Mods::SHIFT, m.shift);
    mods.set(Mods::CONTROL, m.ctrl);
    mods.set(Mods::ALT, m.alt);
    mods.set(Mods::META, m.logo);
    Some(Accelerator::new(mods, code))
}

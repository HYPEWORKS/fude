//! Windowing, input and OS integration.
//!
//! This is the only crate that knows about `winit`, `softbuffer`, the clipboard and file
//! dialogs. Everything above it sees fude's own [`Event`] types and draws into a
//! [`Canvas`], so the windowing layer can be replaced per OS later without touching the
//! editor (`ARCHITECTURE.md` §6).
//!
//! The loop is event-driven: it sleeps until input arrives, a timer the app set comes
//! due ([`Ctx::set_timer`]), or another thread calls [`Waker::wake`].

mod input;
#[cfg(target_os = "macos")]
mod mac_menu;
mod memory;
#[cfg(all(unix, not(target_os = "macos")))]
mod wayland_dnd;

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{
    ElementState, MouseButton as WButton, MouseScrollDelta, StartCause, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Cursor, CursorIcon, Window, WindowId};

pub use fude_render::Canvas;
pub use input::{Key, KeyInput, Modifiers};
pub use memory::{MemoryUsage, memory_usage};

/// Initial window configuration.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: String,
    /// Inner size in logical pixels.
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ScrollUnit {
    Lines,
    /// Physical pixels (trackpads, precise wheels).
    Pixels,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Other,
}

/// Input method (IME) composition, for languages typed in several keystrokes.
#[derive(Clone, PartialEq, Debug)]
pub enum Ime {
    Enabled,
    /// Text being composed, not yet in the document, and the byte range of the IME's
    /// cursor in it. Empty text ends the composition.
    Preedit(String, Option<(usize, usize)>),
    /// Composed text to insert.
    Commit(String),
    Disabled,
}

/// The OS's light or dark appearance.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OsTheme {
    Light,
    Dark,
}

fn os_theme(theme: winit::window::Theme) -> OsTheme {
    match theme {
        winit::window::Theme::Light => OsTheme::Light,
        winit::window::Theme::Dark => OsTheme::Dark,
    }
}

/// Positions are physical pixels from the window's top left.
#[derive(Clone, PartialEq, Debug)]
pub enum Event {
    CloseRequested,
    /// New inner size in physical pixels.
    Resized {
        width: u32,
        height: u32,
    },
    ScaleChanged(f64),
    Focused(bool),
    Key(KeyInput),
    Ime(Ime),
    /// Positive `dy` scrolls the content down (towards the end of the document).
    Scroll {
        dx: f32,
        dy: f32,
        unit: ScrollUnit,
        modifiers: Modifiers,
    },
    MouseMove {
        x: f32,
        y: f32,
        modifiers: Modifiers,
    },
    /// `clicks` counts rapid clicks in one place: 1, 2 (double), 3 (triple), then 1 again.
    MouseDown {
        x: f32,
        y: f32,
        button: MouseButton,
        clicks: u8,
        modifiers: Modifiers,
    },
    MouseUp {
        x: f32,
        y: f32,
        button: MouseButton,
        modifiers: Modifiers,
    },
    MouseLeave,
    /// The time passed to [`Ctx::set_timer`] has come.
    Timer,
    /// Another thread called [`Waker::wake`].
    Wake,
    ThemeChanged(OsTheme),
    /// A file was dropped on the window.
    DroppedFile(PathBuf),
    /// Files are being dragged over the window (true), or the drag left or ended (false).
    FileHover(bool),
    /// A native menu item was chosen, by its [`MenuCommand::id`].
    Menu(u32),
}

/// Whether menus are the OS's own menu bar (macOS), set with [`Ctx::set_menu_bar`],
/// rather than drawn in the window.
pub const NATIVE_MENU_BAR: bool = cfg!(target_os = "macos");

/// A native menu bar's contents: the menus, left to right. The platform adds its own
/// items (on macOS the application and Window menus) and moves the commands with a
/// [`MenuRole`] to where it keeps them.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuBarSpec {
    pub app_name: String,
    pub menus: Vec<MenuSpec>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuSpec {
    pub title: String,
    pub items: Vec<MenuItemSpec>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuItemSpec {
    Command(MenuCommand),
    Separator,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuCommand {
    /// Delivered as [`Event::Menu`].
    pub id: u32,
    pub label: String,
    pub shortcut: Option<(Key, Modifiers)>,
    pub enabled: bool,
    /// `Some` for a command that is on or off.
    pub checked: Option<bool>,
    pub role: MenuRole,
}

/// Commands some platforms put in a place of their own (on macOS, the application menu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuRole {
    Normal,
    About,
    Settings,
    Quit,
}

/// Mouse pointer shapes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorShape {
    Default,
    Text,
    Pointer,
}

/// Startup milestones reported to [`App::startup_mark`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StartupMark {
    EventLoopCreated,
    WindowCreated,
    SurfaceCreated,
}

/// Wakes the event loop from another thread, delivering [`Event::Wake`].
#[derive(Clone)]
pub struct Waker(EventLoopProxy<()>);

impl Waker {
    pub fn wake(&self) {
        // Fails only if the loop has exited, when there's nothing left to wake.
        let _ = self.0.send_event(());
    }
}

/// A file dialog's options.
#[derive(Clone, Debug, Default)]
pub struct FileDialog<'a> {
    pub title: &'a str,
    pub directory: Option<&'a Path>,
    /// Suggested name (save dialogs).
    pub file_name: Option<&'a str>,
}

/// State the platform keeps for the app between events.
struct Shared {
    proxy: EventLoopProxy<()>,
    timer: Option<Instant>,
    cursor: CursorShape,
    /// Created on first use, so startup doesn't pay for it.
    clipboard: Option<arboard::Clipboard>,
    /// Native menu items chosen, by id (sent from the menu's handler, then a wake).
    menu_events: (std::sync::mpsc::Sender<u32>, std::sync::mpsc::Receiver<u32>),
    #[cfg(target_os = "macos")]
    menu: Option<mac_menu::NativeMenu>,
}

/// Lets an [`App`] talk back to the platform.
pub struct Ctx<'a> {
    event_loop: &'a ActiveEventLoop,
    window: &'a Window,
    shared: &'a mut Shared,
}

impl Ctx<'_> {
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub fn exit(&self) {
        self.event_loop.exit();
    }

    pub fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    pub fn set_title(&self, title: &str) {
        self.window.set_title(title);
    }

    /// The OS's light or dark appearance, where the platform reports it.
    pub fn os_theme(&self) -> Option<OsTheme> {
        self.window.theme().map(os_theme)
    }

    /// Brings the window to the front (for example when another launch hands it a file).
    pub fn focus_window(&self) {
        self.window.set_minimized(false);
        self.window.focus_window();
    }

    /// Delivers [`Event::Timer`] at `at` (replacing any earlier request), or cancels.
    pub fn set_timer(&mut self, at: Option<Instant>) {
        self.shared.timer = at;
    }

    pub fn waker(&self) -> Waker {
        Waker(self.shared.proxy.clone())
    }

    pub fn set_cursor(&mut self, shape: CursorShape) {
        if self.shared.cursor == shape {
            return;
        }
        self.shared.cursor = shape;
        let icon = match shape {
            CursorShape::Default => CursorIcon::Default,
            CursorShape::Text => CursorIcon::Text,
            CursorShape::Pointer => CursorIcon::Pointer,
        };
        self.window.set_cursor(Cursor::Icon(icon));
    }

    /// Where the text cursor is (physical pixels), so the IME places its candidate
    /// window next to it.
    pub fn set_ime_area(&self, x: f32, y: f32, width: f32, height: f32) {
        self.window.set_ime_cursor_area(
            PhysicalPosition::new(x, y),
            PhysicalSize::new(width.max(1.0), height.max(1.0)),
        );
    }

    pub fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard()?.get_text().ok()
    }

    pub fn set_clipboard_text(&mut self, text: String) {
        if let Some(clipboard) = self.clipboard()
            && let Err(e) = clipboard.set_text(text)
        {
            eprintln!("fude: couldn't set the clipboard: {e}");
        }
    }

    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.shared.clipboard.is_none() {
            match arboard::Clipboard::new() {
                Ok(c) => self.shared.clipboard = Some(c),
                Err(e) => {
                    eprintln!("fude: no clipboard: {e}");
                    return None;
                }
            }
        }
        self.shared.clipboard.as_mut()
    }

    /// Replaces the native menu bar (where there is one: [`NATIVE_MENU_BAR`]).
    pub fn set_menu_bar(&mut self, spec: &MenuBarSpec) {
        #[cfg(target_os = "macos")]
        {
            if self.shared.menu.is_none() {
                mac_menu::listen(self.shared.proxy.clone(), self.shared.menu_events.0.clone());
            }
            self.shared.menu = Some(mac_menu::NativeMenu::new(spec));
        }
        #[cfg(not(target_os = "macos"))]
        let _ = spec;
    }

    /// Enables or disables, and checks or unchecks, an item of the native menu bar.
    pub fn set_menu_item_state(&mut self, id: u32, enabled: bool, checked: Option<bool>) {
        #[cfg(target_os = "macos")]
        if let Some(menu) = &self.shared.menu {
            menu.set_state(id, enabled, checked);
        }
        #[cfg(not(target_os = "macos"))]
        let _ = (id, enabled, checked);
    }

    /// Shows the system's open dialog. Blocks until it closes.
    pub fn open_file_dialog(&self, options: &FileDialog<'_>) -> Option<PathBuf> {
        self.dialog(options).pick_file()
    }

    /// Shows the system's save dialog. Blocks until it closes.
    pub fn save_file_dialog(&self, options: &FileDialog<'_>) -> Option<PathBuf> {
        self.dialog(options).save_file()
    }

    fn dialog(&self, options: &FileDialog<'_>) -> rfd::FileDialog {
        let mut dialog = rfd::FileDialog::new()
            .set_title(options.title)
            .set_parent(self.window);
        if let Some(dir) = options.directory {
            dialog = dialog.set_directory(dir);
        }
        if let Some(name) = options.file_name {
            dialog = dialog.set_file_name(name);
        }
        dialog
    }
}

/// The application driven by [`run`].
pub trait App {
    fn event(&mut self, ctx: &mut Ctx<'_>, event: Event);

    /// Draws a frame. `scale` is the window's scale factor (physical / logical pixels).
    fn draw(&mut self, canvas: &mut Canvas<'_>, scale: f32);

    /// Called after a frame has been handed to the OS for display.
    fn presented(&mut self, _ctx: &mut Ctx<'_>) {}

    fn startup_mark(&mut self, _mark: StartupMark) {}
}

#[derive(Debug)]
pub struct PlatformError(String);

impl std::fmt::Display for PlatformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PlatformError {}

fn err(context: &str, e: impl std::fmt::Display) -> PlatformError {
    PlatformError(format!("{context}: {e}"))
}

/// Opens a window and runs `app` until it calls [`Ctx::exit`].
pub fn run<A: App>(options: WindowOptions, app: A) -> Result<(), PlatformError> {
    let mut builder = EventLoop::with_user_event();
    // The app sets its own menu bar with its first event.
    #[cfg(target_os = "macos")]
    winit::platform::macos::EventLoopBuilderExtMacOS::with_default_menu(&mut builder, false);
    let event_loop = builder.build().map_err(|e| err("creating event loop", e))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut runner = Runner {
        #[cfg(all(unix, not(target_os = "macos")))]
        dnd: None,
        drops: std::sync::mpsc::channel(),
        app,
        options,
        win: None,
        error: None,
        modifiers: Modifiers::default(),
        mouse: (0.0, 0.0),
        last_click: None,
        shared: Shared {
            proxy: event_loop.create_proxy(),
            timer: None,
            cursor: CursorShape::Default,
            clipboard: None,
            menu_events: std::sync::mpsc::channel(),
            #[cfg(target_os = "macos")]
            menu: None,
        },
    };
    runner.app.startup_mark(StartupMark::EventLoopCreated);
    event_loop
        .run_app(&mut runner)
        .map_err(|e| err("running event loop", e))?;
    match runner.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

struct WinState {
    // Field order is drop order: the surface must go before the context and window.
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    _context: softbuffer::Context<Rc<Window>>,
    window: Rc<Window>,
    surface_size: (u32, u32),
}

/// Rapid clicks within this time and distance count as a double or triple click.
const MULTI_CLICK_TIME: Duration = Duration::from_millis(500);
const MULTI_CLICK_DISTANCE: f64 = 4.0;

struct Click {
    at: Instant,
    pos: (f64, f64),
    button: MouseButton,
    count: u8,
}

struct Runner<A> {
    /// Wayland drag and drop (winit has none there). Dropped before the event loop ends.
    #[cfg(all(unix, not(target_os = "macos")))]
    dnd: Option<wayland_dnd::WaylandDrop>,
    /// Paths dropped on the window under Wayland, read on a helper thread.
    drops: (
        std::sync::mpsc::Sender<Vec<PathBuf>>,
        std::sync::mpsc::Receiver<Vec<PathBuf>>,
    ),
    app: A,
    options: WindowOptions,
    win: Option<WinState>,
    error: Option<PlatformError>,
    modifiers: Modifiers,
    /// Last mouse position, physical pixels.
    mouse: (f64, f64),
    last_click: Option<Click>,
    shared: Shared,
}

impl<A: App> Runner<A> {
    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<WinState, PlatformError> {
        let attrs = Window::default_attributes()
            .with_title(self.options.title.clone())
            .with_inner_size(LogicalSize::new(self.options.width, self.options.height))
            .with_min_inner_size(LogicalSize::new(320.0, 200.0));
        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| err("creating window", e))?,
        );
        self.app.startup_mark(StartupMark::WindowCreated);
        window.set_ime_allowed(true);
        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| err("creating draw context", e))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| err("creating draw surface", e))?;
        self.app.startup_mark(StartupMark::SurfaceCreated);
        Ok(WinState {
            surface,
            _context: context,
            window,
            surface_size: (0, 0),
        })
    }

    /// Listens for file drops under Wayland, sharing winit's connection.
    #[cfg(all(unix, not(target_os = "macos")))]
    fn start_wayland_dnd(&mut self, window: &Window) {
        use winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
        let Ok(handle) = window.display_handle() else {
            return;
        };
        let RawDisplayHandle::Wayland(wayland) = handle.as_raw() else {
            return;
        };
        let proxy = self.shared.proxy.clone();
        let proxy = std::sync::Mutex::new(proxy);
        let wake = move || {
            if let Ok(p) = proxy.lock() {
                let _ = p.send_event(());
            }
        };
        // SAFETY: the display is the event loop's own and lives until the loop ends; the
        // drop handler is dropped in `exiting`, before that.
        #[allow(unsafe_code)]
        let dnd = unsafe {
            wayland_dnd::WaylandDrop::new(wayland.display.as_ptr(), self.drops.0.clone(), wake)
        };
        self.dnd = dnd;
    }

    fn dispatch(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        let Some(win) = &self.win else { return };
        let window = win.window.clone();
        let mut ctx = Ctx {
            event_loop,
            window: &window,
            shared: &mut self.shared,
        };
        self.app.event(&mut ctx, event);
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> Result<(), PlatformError> {
        let Some(win) = &mut self.win else {
            return Ok(());
        };
        let size = win.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return Ok(()); // minimized
        };
        if win.surface_size != (w.get(), h.get()) {
            win.surface
                .resize(w, h)
                .map_err(|e| err("resizing surface", e))?;
            win.surface_size = (w.get(), h.get());
        }
        let scale = win.window.scale_factor() as f32;
        let mut buffer = win
            .surface
            .buffer_mut()
            .map_err(|e| err("acquiring buffer", e))?;
        {
            let mut canvas = Canvas::new(&mut buffer, w.get() as usize, h.get() as usize);
            self.app.draw(&mut canvas, scale);
        }
        win.window.pre_present_notify();
        buffer.present().map_err(|e| err("presenting", e))?;
        let window = win.window.clone();
        self.app.presented(&mut Ctx {
            event_loop,
            window: &window,
            shared: &mut self.shared,
        });
        Ok(())
    }

    fn mouse_button(&mut self, event_loop: &ActiveEventLoop, state: ElementState, button: WButton) {
        let button = match button {
            WButton::Left => MouseButton::Left,
            WButton::Middle => MouseButton::Middle,
            WButton::Right => MouseButton::Right,
            _ => MouseButton::Other,
        };
        let (x, y) = (self.mouse.0 as f32, self.mouse.1 as f32);
        let modifiers = self.modifiers;
        let event = match state {
            ElementState::Pressed => {
                let now = Instant::now();
                let count = match &self.last_click {
                    Some(c)
                        if c.button == button
                            && now.duration_since(c.at) < MULTI_CLICK_TIME
                            && (c.pos.0 - self.mouse.0).abs() <= MULTI_CLICK_DISTANCE
                            && (c.pos.1 - self.mouse.1).abs() <= MULTI_CLICK_DISTANCE =>
                    {
                        c.count % 3 + 1
                    }
                    _ => 1,
                };
                self.last_click = Some(Click {
                    at: now,
                    pos: self.mouse,
                    button,
                    count,
                });
                Event::MouseDown {
                    x,
                    y,
                    button,
                    clicks: count,
                    modifiers,
                }
            }
            ElementState::Released => Event::MouseUp {
                x,
                y,
                button,
                modifiers,
            },
        };
        self.dispatch(event_loop, event);
    }

    /// Delivers the timer if it's due.
    fn check_timer(&mut self, event_loop: &ActiveEventLoop) {
        if self.shared.timer.is_some_and(|t| t <= Instant::now()) {
            self.shared.timer = None;
            self.dispatch(event_loop, Event::Timer);
        }
    }
}

impl<A: App> ApplicationHandler for Runner<A> {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        if let StartCause::ResumeTimeReached { .. } = cause {
            self.check_timer(event_loop);
        }
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.win.is_some() {
            return;
        }
        match self.create_window(event_loop) {
            Ok(win) => {
                win.window.request_redraw();
                let theme = win.window.theme();
                #[cfg(all(unix, not(target_os = "macos")))]
                self.start_wayland_dnd(&win.window);
                self.win = Some(win);
                // Before the first frame, so it's drawn in the right theme.
                if let Some(theme) = theme {
                    self.dispatch(event_loop, Event::ThemeChanged(os_theme(theme)));
                }
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: ()) {
        while let Ok(paths) = self.drops.1.try_recv() {
            for path in paths {
                self.dispatch(event_loop, Event::DroppedFile(path));
            }
        }
        while let Ok(id) = self.shared.menu_events.1.try_recv() {
            self.dispatch(event_loop, Event::Menu(id));
        }
        self.dispatch(event_loop, Event::Wake);
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // Our queue on winit's Wayland connection must go before the connection does.
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            self.dnd = None;
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw(event_loop) {
                    self.error = Some(e);
                    event_loop.exit();
                }
            }
            WindowEvent::CloseRequested => self.dispatch(event_loop, Event::CloseRequested),
            WindowEvent::Resized(size) => {
                self.dispatch(
                    event_loop,
                    Event::Resized {
                        width: size.width,
                        height: size.height,
                    },
                );
                if let Some(win) = &self.win {
                    win.window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.dispatch(event_loop, Event::ScaleChanged(scale_factor));
            }
            WindowEvent::Focused(focused) => self.dispatch(event_loop, Event::Focused(focused)),
            WindowEvent::ThemeChanged(theme) => {
                self.dispatch(event_loop, Event::ThemeChanged(os_theme(theme)));
            }
            WindowEvent::DroppedFile(path) => {
                self.dispatch(event_loop, Event::FileHover(false));
                self.dispatch(event_loop, Event::DroppedFile(path));
            }
            WindowEvent::HoveredFile(_) => self.dispatch(event_loop, Event::FileHover(true)),
            WindowEvent::HoveredFileCancelled => self.dispatch(event_loop, Event::FileHover(false)),
            WindowEvent::ModifiersChanged(m) => self.modifiers = input::modifiers(m.state()),
            WindowEvent::KeyboardInput { event, .. } => {
                let key = input::key_input(&event, self.modifiers);
                self.dispatch(event_loop, Event::Key(key));
            }
            WindowEvent::Ime(ime) => {
                let ime = match ime {
                    winit::event::Ime::Enabled => Ime::Enabled,
                    winit::event::Ime::Preedit(text, cursor) => Ime::Preedit(text, cursor),
                    winit::event::Ime::Commit(text) => Ime::Commit(text),
                    winit::event::Ime::Disabled => Ime::Disabled,
                };
                self.dispatch(event_loop, Event::Ime(ime));
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse = (position.x, position.y);
                let modifiers = self.modifiers;
                self.dispatch(
                    event_loop,
                    Event::MouseMove {
                        x: position.x as f32,
                        y: position.y as f32,
                        modifiers,
                    },
                );
            }
            WindowEvent::CursorLeft { .. } => self.dispatch(event_loop, Event::MouseLeave),
            WindowEvent::MouseInput { state, button, .. } => {
                self.mouse_button(event_loop, state, button);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // winit reports positive y when content should move down (scroll up);
                // fude's convention is positive = towards the end of the document.
                let modifiers = self.modifiers;
                let event = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Event::Scroll {
                        dx: -x,
                        dy: -y,
                        unit: ScrollUnit::Lines,
                        modifiers,
                    },
                    MouseScrollDelta::PixelDelta(p) => Event::Scroll {
                        dx: -p.x as f32,
                        dy: -p.y as f32,
                        unit: ScrollUnit::Pixels,
                        modifiers,
                    },
                };
                self.dispatch(event_loop, event);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(dnd) = &mut self.dnd {
            for wayland_dnd::DropEvent::Hover(on) in dnd.dispatch() {
                self.dispatch(event_loop, Event::FileHover(on));
            }
        }
        self.check_timer(event_loop);
        event_loop.set_control_flow(match self.shared.timer {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
}

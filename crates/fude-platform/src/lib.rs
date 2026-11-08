//! Windowing, input and OS integration.
//!
//! This is the only crate that knows about `winit` and `softbuffer`. Everything above it
//! sees fude's own [`Event`] types and draws into a [`Canvas`], so the windowing layer
//! can be replaced per OS later without touching the editor (`ARCHITECTURE.md` §6).

mod input;
mod memory;

use std::num::NonZeroU32;
use std::rc::Rc;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

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
    /// Positive `dy` scrolls the content down (towards the end of the document).
    Scroll {
        dx: f32,
        dy: f32,
        unit: ScrollUnit,
    },
}

/// Startup milestones reported to [`App::startup_mark`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StartupMark {
    EventLoopCreated,
    WindowCreated,
    SurfaceCreated,
}

/// Lets an [`App`] talk back to the platform.
pub struct Ctx<'a> {
    event_loop: &'a ActiveEventLoop,
    window: &'a Window,
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
    let event_loop = EventLoop::new().map_err(|e| err("creating event loop", e))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut runner = Runner {
        app,
        options,
        win: None,
        error: None,
        modifiers: Modifiers::default(),
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

struct Runner<A> {
    app: A,
    options: WindowOptions,
    win: Option<WinState>,
    error: Option<PlatformError>,
    modifiers: Modifiers,
}

impl<A: App> Runner<A> {
    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<WinState, PlatformError> {
        let attrs = Window::default_attributes()
            .with_title(self.options.title.clone())
            .with_inner_size(LogicalSize::new(self.options.width, self.options.height));
        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .map_err(|e| err("creating window", e))?,
        );
        self.app.startup_mark(StartupMark::WindowCreated);
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

    fn dispatch(&mut self, event_loop: &ActiveEventLoop, event: Event) {
        let Some(win) = &self.win else { return };
        let window = win.window.clone();
        let mut ctx = Ctx {
            event_loop,
            window: &window,
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
        });
        Ok(())
    }
}

impl<A: App> ApplicationHandler for Runner<A> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.win.is_some() {
            return;
        }
        match self.create_window(event_loop) {
            Ok(win) => {
                win.window.request_redraw();
                self.win = Some(win);
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
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
            WindowEvent::ModifiersChanged(m) => self.modifiers = input::modifiers(m.state()),
            WindowEvent::KeyboardInput { event, .. } => {
                let key = input::key_input(&event, self.modifiers);
                self.dispatch(event_loop, Event::Key(key));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // winit reports positive y when content should move down (scroll up);
                // fude's convention is positive = towards the end of the document.
                let event = match delta {
                    MouseScrollDelta::LineDelta(x, y) => Event::Scroll {
                        dx: -x,
                        dy: -y,
                        unit: ScrollUnit::Lines,
                    },
                    MouseScrollDelta::PixelDelta(p) => Event::Scroll {
                        dx: -p.x as f32,
                        dy: -p.y as f32,
                        unit: ScrollUnit::Pixels,
                    },
                };
                self.dispatch(event_loop, event);
            }
            _ => {}
        }
    }
}

//! Accessibility: the window's AccessKit adapter, which answers the OS's accessibility API
//! (UI Automation, NSAccessibility, AT-SPI) from the tree the app builds.
//!
//! The adapter's handlers run on whatever thread the OS calls in on (on Linux, AccessKit's
//! own), so they only pass requests to the event loop through a channel and wake it. The
//! app builds a tree only while an assistive technology is listening: until one asks for
//! the first tree, [`accesskit_winit::Adapter::update_if_active`] never calls its closure.

use std::sync::mpsc::{Receiver, Sender, channel};

use accesskit::{ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::Window;

/// What the OS asked for.
pub(crate) enum Request {
    /// An assistive technology started listening: send the whole tree.
    Tree,
    /// Do something to a node (focus it, click it, select text).
    Action(ActionRequest),
}

/// Passes requests to the event loop and wakes it.
#[derive(Clone)]
struct Handler {
    tx: Sender<Request>,
    proxy: EventLoopProxy<()>,
}

impl Handler {
    fn send(&self, request: Request) {
        // Fails only once the loop has exited.
        if self.tx.send(request).is_ok() {
            let _ = self.proxy.send_event(());
        }
    }
}

impl ActivationHandler for Handler {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        // The tree is built on the event loop's thread, from the last frame.
        self.send(Request::Tree);
        None
    }
}

impl ActionHandler for Handler {
    fn do_action(&mut self, request: ActionRequest) {
        self.send(Request::Action(request));
    }
}

impl DeactivationHandler for Handler {
    fn deactivate_accessibility(&mut self) {}
}

pub(crate) struct Access {
    adapter: accesskit_winit::Adapter,
    rx: Receiver<Request>,
}

impl Access {
    /// Must be created before the window is first shown.
    pub(crate) fn new(
        event_loop: &ActiveEventLoop,
        window: &Window,
        proxy: EventLoopProxy<()>,
    ) -> Access {
        let (tx, rx) = channel();
        let handler = Handler { tx, proxy };
        let adapter = accesskit_winit::Adapter::with_direct_handlers(
            event_loop,
            window,
            handler.clone(),
            handler.clone(),
            handler,
        );
        Access { adapter, rx }
    }

    /// Keeps the adapter up to date with the window's focus, position and size.
    pub(crate) fn window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) {
        self.adapter.process_event(window, event);
    }

    pub(crate) fn try_recv(&self) -> Option<Request> {
        self.rx.try_recv().ok()
    }

    /// Sends the tree `build` makes, if an assistive technology is listening.
    pub(crate) fn update(&mut self, build: impl FnOnce() -> TreeUpdate) {
        self.adapter.update_if_active(build);
    }
}

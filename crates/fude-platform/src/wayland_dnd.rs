//! Dropping files on the window under Wayland, which winit doesn't support.
//!
//! This opens an event queue of its own on winit's Wayland connection and binds the
//! seat's data device (core protocol). A drag that offers `text/uri-list` (what file
//! managers send) is accepted as a copy; on drop, the list is read on a helper thread
//! and the paths come back through a channel, waking the event loop.

// Sharing winit's display needs one unsafe call; see `WaylandDrop::new`.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Read;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::Sender;

use wayland_backend::client::{Backend, ObjectId};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};

const URI_LIST: &str = "text/uri-list";

/// Something that happened to a drag over the window.
#[derive(Debug, PartialEq, Eq)]
pub enum DropEvent {
    /// Files are being dragged over the window (true) or left it (false).
    Hover(bool),
}

struct State {
    conn: Connection,
    manager: Option<WlDataDeviceManager>,
    seats: Vec<WlSeat>,
    /// Mime types each live offer has announced.
    offers: HashMap<ObjectId, Vec<String>>,
    /// The offer being dragged over the window, if it has files.
    current: Option<WlDataOffer>,
    events: Vec<DropEvent>,
    dropped: Sender<Vec<PathBuf>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

pub struct WaylandDrop {
    queue: EventQueue<State>,
    state: State,
    _devices: Vec<WlDataDevice>,
}

impl WaylandDrop {
    /// Starts listening for drops on the display winit is using. `dropped` receives the
    /// paths of each drop; `wake` is called after sending.
    ///
    /// # Safety
    /// `display` must be the live `wl_display` of the window's event loop, and the result
    /// must be dropped before that display is closed (before the event loop ends).
    pub unsafe fn new(
        display: *mut c_void,
        dropped: Sender<Vec<PathBuf>>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Option<WaylandDrop> {
        // SAFETY: the caller guarantees the display outlives this connection.
        let backend = unsafe { Backend::from_foreign_display(display.cast()) };
        let conn = Connection::from_backend(backend);
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let _registry = conn.display().get_registry(&qh, ());
        let mut state = State {
            conn: conn.clone(),
            manager: None,
            seats: Vec::new(),
            offers: HashMap::new(),
            current: None,
            events: Vec::new(),
            dropped,
            wake: Arc::new(wake),
        };
        queue.roundtrip(&mut state).ok()?;
        let manager = state.manager.clone()?;
        let devices: Vec<WlDataDevice> = state
            .seats
            .iter()
            .map(|seat| manager.get_data_device(seat, &qh, ()))
            .collect();
        conn.flush().ok()?;
        Some(WaylandDrop {
            queue,
            state,
            _devices: devices,
        })
    }

    /// Handles queued Wayland events; returns hover changes for the app.
    pub fn dispatch(&mut self) -> Vec<DropEvent> {
        let _ = self.queue.dispatch_pending(&mut self.state);
        let _ = self.state.conn.flush();
        std::mem::take(&mut self.state.events)
    }
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut State,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<State>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_data_device_manager" => {
                    state.manager = Some(registry.bind(name, version.min(3), qh, ()));
                }
                "wl_seat" => state
                    .seats
                    .push(registry.bind(name, version.min(1), qh, ())),
                _ => {}
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut State,
        _: &WlSeat,
        _: <WlSeat as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<WlDataDeviceManager, ()> for State {
    fn event(
        _: &mut State,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
    }
}

impl Dispatch<WlDataOffer, ()> for State {
    fn event(
        state: &mut State,
        offer: &WlDataOffer,
        event: wl_data_offer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state.offers.entry(offer.id()).or_default().push(mime_type);
        }
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        state: &mut State,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<State>,
    ) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                state.offers.insert(id.id(), Vec::new());
            }
            wl_data_device::Event::Enter { serial, id, .. } => {
                let Some(offer) = id else { return };
                let has_files = state
                    .offers
                    .get(&offer.id())
                    .is_some_and(|m| m.iter().any(|m| m == URI_LIST));
                if has_files {
                    offer.accept(serial, Some(URI_LIST.to_owned()));
                    if offer.version() >= 3 {
                        offer.set_actions(DndAction::Copy, DndAction::Copy);
                    }
                    state.current = Some(offer);
                    state.events.push(DropEvent::Hover(true));
                } else {
                    offer.accept(serial, None);
                }
            }
            wl_data_device::Event::Leave => {
                if let Some(offer) = state.current.take() {
                    state.offers.remove(&offer.id());
                    offer.destroy();
                    state.events.push(DropEvent::Hover(false));
                }
            }
            wl_data_device::Event::Drop => {
                let Some(offer) = state.current.take() else {
                    return;
                };
                state.offers.remove(&offer.id());
                state.events.push(DropEvent::Hover(false));
                let Ok((mut reader, writer)) = std::io::pipe() else {
                    offer.destroy();
                    return;
                };
                offer.receive(URI_LIST.to_owned(), writer.as_fd());
                drop(writer);
                let _ = state.conn.flush();
                // The source writes the list after seeing the request; read it off the
                // UI thread, then tell the source we're done.
                let (conn, tx, wake) = (
                    state.conn.clone(),
                    state.dropped.clone(),
                    state.wake.clone(),
                );
                std::thread::spawn(move || {
                    let mut list = Vec::new();
                    let _ = reader.read_to_end(&mut list);
                    if offer.version() >= 3 {
                        offer.finish();
                    }
                    offer.destroy();
                    let _ = conn.flush();
                    let paths = parse_uri_list(&list);
                    if !paths.is_empty() && tx.send(paths).is_ok() {
                        wake();
                    }
                });
            }
            wl_data_device::Event::Selection { id: Some(offer) } => {
                // Clipboard offers are arboard's business, not ours.
                state.offers.remove(&offer.id());
                offer.destroy();
            }
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}

/// Local file paths from a `text/uri-list`: one URI per line, `#` comments, percent
/// encoding.
pub(crate) fn parse_uri_list(list: &[u8]) -> Vec<PathBuf> {
    list.split(|&b| b == b'\n')
        .map(|line| line.trim_suffix(b"\r"))
        .filter(|line| !line.is_empty() && !line.starts_with(b"#"))
        .filter_map(|line| line.strip_prefix(b"file://"))
        .filter_map(|rest| {
            // file://host/path: only the local host (empty or "localhost") is ours.
            let slash = rest.iter().position(|&b| b == b'/')?;
            let (host, path) = rest.split_at(slash);
            if !host.is_empty() && host != b"localhost" {
                return None;
            }
            let bytes = percent_decode(path)?;
            use std::os::unix::ffi::OsStringExt;
            Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
        })
        .collect()
}

fn percent_decode(s: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' {
            let hex = std::str::from_utf8(s.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_lists() {
        let list = b"# from a file manager\r\nfile:///home/me/My%20Notes.txt\r\nfile://localhost/tmp/a.rs\nhttps://example.com/x\nfile://other-host/y\nfile:///tmp/%E6%97%A5.txt\n";
        assert_eq!(
            parse_uri_list(list),
            [
                PathBuf::from("/home/me/My Notes.txt"),
                PathBuf::from("/tmp/a.rs"),
                PathBuf::from("/tmp/日.txt"),
            ]
        );
    }
}

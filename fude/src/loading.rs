//! Loading files on worker threads, so opening a big file never freezes the window.
//!
//! Each load runs on a thread of its own and reports through one channel: a preview of
//! the start of the file (shown read-only right away), progress, and the result. Opening
//! waits a moment for small files, so they appear whole without a flash of "Loading".

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fude_buffer::{Encoding, FileStamp, Format, LoadObserver, Loaded, Rope};
use fude_platform::Waker;

use crate::app::Fude;
use crate::doc::{Doc, DocId};

/// How long opening waits for a file before showing its preview.
const SETTLE: Duration = Duration::from_millis(20);
/// Progress reports at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(50);

pub enum LoadEvent {
    Preview(Loaded),
    Progress(u64),
    Done(io::Result<Loaded>),
}

/// What the loaded text is for.
#[derive(Clone, Debug)]
pub enum LoadKind {
    /// A file being opened (or a session tab being restored).
    Open,
    /// A session backup: the document's unsaved text, in its own format.
    Backup {
        format: Format,
        path: Option<PathBuf>,
    },
    /// The file changed on disk: its new text replaces the old as one undo step.
    Reload,
}

/// A document's load in progress.
pub struct Loading {
    pub kind: LoadKind,
    pub read: u64,
    pub total: u64,
    cancel: Arc<AtomicBool>,
    /// Cursor and first line to restore when done (a session tab).
    pub restore: Option<(usize, f64)>,
    /// The length of the preview shown, if one is.
    preview: Option<usize>,
}

impl Loading {
    /// Percent read, for the status bar.
    pub fn percent(&self) -> u64 {
        (self.read * 100)
            .checked_div(self.total)
            .unwrap_or(0)
            .min(100)
    }
}

impl Drop for Loading {
    fn drop(&mut self) {
        // A load no one waits for any more (the tab was closed) stops at its next read.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The channel loads report through, and a way to wake the event loop.
pub struct Loads {
    tx: std::sync::mpsc::Sender<(DocId, LoadEvent)>,
    rx: std::sync::mpsc::Receiver<(DocId, LoadEvent)>,
    waker: Arc<Mutex<Option<Waker>>>,
}

impl Default for Loads {
    fn default() -> Loads {
        let (tx, rx) = std::sync::mpsc::channel();
        Loads {
            tx,
            rx,
            waker: Arc::default(),
        }
    }
}

impl Loads {
    pub fn set_waker(&self, waker: Waker) {
        if let Ok(mut w) = self.waker.lock() {
            *w = Some(waker);
        }
    }
}

/// Reports a load's progress from its thread.
struct Report {
    id: DocId,
    tx: std::sync::mpsc::Sender<(DocId, LoadEvent)>,
    waker: Arc<Mutex<Option<Waker>>>,
    cancel: Arc<AtomicBool>,
    last: Instant,
}

impl Report {
    fn send(&self, event: LoadEvent) {
        if self.tx.send((self.id, event)).is_ok()
            && let Ok(w) = self.waker.lock()
            && let Some(w) = &*w
        {
            w.wake();
        }
    }
}

impl LoadObserver for Report {
    fn preview(&mut self, preview: Loaded) {
        self.send(LoadEvent::Preview(preview));
    }

    fn progress(&mut self, read: u64) {
        if self.last.elapsed() >= PROGRESS_EVERY {
            self.last = Instant::now();
            self.send(LoadEvent::Progress(read));
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

impl Fude {
    /// Starts loading `path` into the document `id` on a worker thread.
    pub fn start_load(
        &mut self,
        id: DocId,
        path: &Path,
        encoding: Option<&'static Encoding>,
        kind: LoadKind,
    ) {
        let Some(i) = self.index_of(id) else { return };
        let total = std::fs::metadata(path).map_or(0, |m| m.len());
        let cancel = Arc::new(AtomicBool::new(false));
        let mut report = Report {
            id,
            tx: self.loads.tx.clone(),
            waker: self.loads.waker.clone(),
            cancel: cancel.clone(),
            last: Instant::now(),
        };
        let path = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("fude-load".into())
            .spawn(move || {
                let result = fude_buffer::load_observed(&path, encoding, &mut report);
                report.send(LoadEvent::Done(result));
            });
        if let Err(e) = spawned {
            self.say(format!("Couldn't start loading: {e}"), true);
            return;
        }
        let doc = &mut self.docs[i];
        doc.view.editor.set_read_only(true);
        doc.loading = Some(Loading {
            kind,
            read: 0,
            total,
            cancel,
            restore: None,
            preview: None,
        });
    }

    /// Whether any document is still loading.
    pub fn loading(&self) -> bool {
        self.docs.iter().any(|d| d.loading.is_some())
    }

    /// Takes in what loads have reported. Returns whether to redraw.
    pub fn poll_loads(&mut self) -> bool {
        let mut redraw = false;
        while let Ok((id, event)) = self.loads.rx.try_recv() {
            redraw |= self.load_event(id, event);
        }
        redraw
    }

    /// Waits up to [`SETTLE`] for loads to finish (or at least show a preview), so a file
    /// that loads quickly appears whole.
    pub fn settle_loads(&mut self) {
        let deadline = Instant::now() + SETTLE;
        while self.loading() {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.loads.rx.recv_timeout(left) {
                Ok((id, event)) => {
                    self.load_event(id, event);
                }
                Err(_) => return,
            }
        }
    }

    /// For the modes without a window: waits until every document has loaded.
    pub fn finish_loading(&mut self) {
        while self.loading() {
            match self.loads.rx.recv_timeout(Duration::from_secs(60)) {
                Ok((id, event)) => {
                    self.load_event(id, event);
                }
                Err(_) => return,
            }
        }
    }

    fn load_event(&mut self, id: DocId, event: LoadEvent) -> bool {
        let Some(i) = self.index_of(id) else {
            return false;
        };
        let active = i == self.active;
        let doc = &mut self.docs[i];
        let Some(loading) = &mut doc.loading else {
            return false;
        };
        match event {
            LoadEvent::Progress(read) => {
                loading.read = read;
                active
            }
            LoadEvent::Preview(preview) => {
                if matches!(loading.kind, LoadKind::Reload) {
                    return false;
                }
                let format = match &loading.kind {
                    LoadKind::Backup { format, .. } => *format,
                    _ => preview.format,
                };
                loading.preview = Some(preview.text.len());
                doc.view.editor.load(preview.text, format, 0);
                active
            }
            LoadEvent::Done(result) => {
                let loading = doc.loading.take().expect("loading");
                self.loaded(i, loading, result);
                true
            }
        }
    }

    /// A load finished: the text becomes the document's.
    fn loaded(&mut self, i: usize, loading: Loading, result: io::Result<Loaded>) {
        let doc = &mut self.docs[i];
        doc.view.editor.set_read_only(false);
        let name = doc.name();
        let loaded = match result {
            Ok(loaded) => loaded,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => return,
            Err(e) => {
                match loading.kind {
                    LoadKind::Reload => self.say(format!("Couldn't reload {name}: {e}"), true),
                    LoadKind::Open | LoadKind::Backup { .. } => {
                        let id = doc.id;
                        self.say(format!("Couldn't open {name}: {e}"), true);
                        self.remove_doc(id);
                    }
                }
                return;
            }
        };
        match &loading.kind {
            LoadKind::Reload => {
                let editor = &mut doc.view.editor;
                editor.replace_content(loaded.text, loaded.format);
                let version = editor.buffer().version();
                editor.buffer_mut().mark_saved(version);
                doc.stamp = loaded.stamp;
                doc.inexact = !loaded.exact;
                self.say(format!("Reloaded {name} (changed on disk)"), false);
                return;
            }
            LoadKind::Open => {
                let note = loaded_note(&loaded);
                let unchanged = same_start(doc.view.editor.text(), &loaded.text, loading.preview);
                doc.view.editor.load(loaded.text, loaded.format, unchanged);
                doc.stamp = loaded.stamp;
                doc.inexact = !loaded.exact;
                if let Some(note) = note {
                    self.say(note, false);
                }
            }
            LoadKind::Backup { format, path } => {
                let unchanged = same_start(doc.view.editor.text(), &loaded.text, loading.preview);
                doc.view.editor.load(loaded.text, *format, unchanged);
                doc.view.editor.buffer_mut().mark_unsaved();
                doc.stamp = path.as_deref().and_then(|p| FileStamp::of(p).ok());
            }
        }
        let doc = &mut self.docs[i];
        if let Some((cursor, scroll)) = loading.restore {
            let text = doc.view.editor.text();
            let at = fude_editor::text::floor_grapheme(text, cursor.min(text.len()));
            doc.view.editor.select(at..at);
            doc.view.scroll_to_line(scroll);
        }
        // Colors for the preview were for other text, as far as the highlighter knows.
        doc.requested = None;
    }

    /// Opens a document in a new tab and loads `source` into it in the background: the
    /// file itself, or for a backup, the backup of the document's unsaved text. `restore`
    /// puts the cursor and view back where a session left them.
    pub fn open_loading(
        &mut self,
        kind: LoadKind,
        source: &Path,
        restore: Option<(usize, f64)>,
    ) -> DocId {
        let id = self.new_id();
        let (path, encoding) = match &kind {
            LoadKind::Backup { path, .. } => (path.clone(), Encoding::for_label(b"utf-8")),
            _ => (Some(source.to_path_buf()), None),
        };
        let doc = Doc::new(id, Rope::new(), Format::default(), path);
        self.add_doc(doc);
        self.start_load(id, source, encoding, kind);
        if let Some(i) = self.index_of(id)
            && let Some(loading) = &mut self.docs[i].loading
        {
            loading.restore = restore;
        }
        id
    }
}

/// How much of the start of `new` is known to equal `old` (the preview it replaces).
fn same_start(old: &Rope, new: &Rope, preview: Option<usize>) -> usize {
    let Some(n) = preview.filter(|&n| n <= new.len() && n == old.len()) else {
        return 0;
    };
    if old.slice(0..n).to_vec() == new.slice(0..n).to_vec() {
        n
    } else {
        0
    }
}

/// Anything worth telling the user about how a file was read.
pub fn loaded_note(loaded: &Loaded) -> Option<String> {
    if loaded.binary {
        Some("This looks like a binary file".to_owned())
    } else if !loaded.exact {
        Some(format!(
            "Opened as {}; some bytes can't be represented exactly",
            loaded.format.encoding.name()
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn temp_file(name: &str, text: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("fude-load-{}-{name}", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_large_file_shows_its_start_then_loads() {
        let text = "a line of text, as in a log\n".repeat(200_000);
        let path = temp_file("big.txt", &text);
        let mut app = Fude::new(Config::default());
        let restore = Some((text.len() - 3, 1000.0));
        let id = app.open_loading(LoadKind::Open, &path, restore);
        let i = app.index_of(id).unwrap();
        assert!(app.docs[i].view.editor.is_read_only());

        // The preview comes first: the start of the file, read-only.
        let (from, event) = app.loads.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(event, LoadEvent::Preview(_)));
        app.load_event(from, event);
        let shown = app.docs[i].view.editor.text().to_vec();
        assert!(!shown.is_empty() && shown.len() <= 64 * 1024);
        assert!(text.as_bytes().starts_with(&shown));
        let editor = &mut app.docs[i].view.editor;
        editor.insert("x", Instant::now());
        assert!(editor.take_refused(), "typing waits for the whole file");
        assert_eq!(editor.text().to_vec(), shown);

        app.finish_loading();
        let doc = &mut app.docs[i];
        std::fs::remove_file(&path).unwrap();
        assert!(doc.loading.is_none());
        assert_eq!(doc.view.editor.text().len(), text.len());
        assert!(!doc.modified());
        assert!(doc.stamp.is_some());
        let head = doc.view.editor.selections().primary().head;
        assert_eq!(head, text.len() - 3, "the session's cursor is back");
        doc.view.editor.insert("x", Instant::now());
        assert!(doc.modified());
    }

    #[test]
    fn closing_a_loading_tab_stops_its_load() {
        let text = "x".repeat(8 << 20);
        let path = temp_file("closed.txt", &text);
        let mut app = Fude::new(Config::default());
        let id = app.open_loading(LoadKind::Open, &path, None);
        let cancel = app.docs[app.index_of(id).unwrap()]
            .loading
            .as_ref()
            .unwrap()
            .cancel
            .clone();
        app.remove_doc(id);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(!app.loading());
        // Whatever the thread still reports is for a tab that's gone.
        std::thread::sleep(Duration::from_millis(100));
        assert!(!app.poll_loads());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_missing_file_closes_its_tab_and_says_so() {
        let mut app = Fude::new(Config::default());
        let path = std::env::temp_dir().join("fude-load-missing/nowhere.txt");
        app.open_loading(LoadKind::Open, &path, None);
        app.finish_loading();
        assert_eq!(app.docs.len(), 1);
        assert!(app.docs[0].path.is_none(), "back to an empty document");
    }
}

//! Opening, saving and closing documents, and the questions that go with it.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use fude_buffer::{Encoding, FileStamp, Format, SaveError};
use fude_platform::{Ctx, FileDialog};

use crate::app::{Fude, Question};
use crate::doc::{Doc, DocId, SaveJob, Then};
use crate::loading::LoadKind;

impl Fude {
    /// Opens `path` in a tab, or switches to it if it's already open. A path that
    /// doesn't exist yet opens as a new document that saves there.
    pub fn open(&mut self, path: &Path) {
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Some(i) = self
            .docs
            .iter()
            .position(|d| d.path.as_deref() == Some(&path))
        {
            self.activate(i);
            return;
        }
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_dir() => {
                self.say(format!("{} is a folder", path.display()), true);
            }
            Ok(meta) if meta.len() > self.config.large_file_mb << 20 => {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                let message = format!(
                    "{name} is {}. An open file takes about as much memory as its size.",
                    human_size(meta.len())
                );
                self.ask(
                    Question::OpenLarge(path),
                    "Open a large file?",
                    &message,
                    &["Open", "Cancel"],
                    0,
                    1,
                );
            }
            Ok(_) => self.open_now(&path),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let id = self.new_id();
                let mut doc = Doc::untitled(id);
                doc.path = Some(path);
                self.add_doc(doc);
            }
            Err(e) => self.say(format!("Couldn't open {}: {e}", path.display()), true),
        }
    }

    /// Opens a file without asking, loading it in the background. A small file appears
    /// whole; a large one shows its start while the rest is read.
    fn open_now(&mut self, path: &Path) {
        self.open_loading(LoadKind::Open, path, None);
        self.settle_loads();
    }

    pub fn open_dialog(&mut self, ctx: &mut Ctx<'_>) {
        let dir = self.directory(self.doc().id);
        let options = FileDialog {
            title: "Open",
            directory: dir.as_deref(),
            file_name: None,
        };
        if let Some(path) = ctx.open_file_dialog(&options) {
            self.open(&path);
        }
    }

    fn directory(&self, id: DocId) -> Option<PathBuf> {
        let doc = &self.docs[self.index_of(id)?];
        doc.path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
    }

    /// Closes a tab, asking first if it has unsaved changes.
    pub fn close(&mut self, ctx: &mut Ctx<'_>, id: DocId) {
        let Some(i) = self.index_of(id) else { return };
        if self.docs[i].modified() {
            self.activate(i);
            self.ask_unsaved(id, Then::Close(id));
        } else {
            self.remove_doc(id);
        }
        let _ = ctx;
    }

    /// Quits. With hot exit, unsaved changes are kept for next time; otherwise asks about
    /// each document with unsaved changes in turn.
    pub fn quit(&mut self, ctx: &mut Ctx<'_>) {
        let modified = self.docs.iter().position(Doc::modified);
        match modified {
            Some(i) if !self.config.hot_exit || self.state_dir.is_none() => {
                self.activate(i);
                let id = self.docs[i].id;
                self.ask_unsaved(id, Then::Quit);
            }
            _ => {
                self.save_session(true);
                ctx.exit();
            }
        }
    }

    fn ask_unsaved(&mut self, id: DocId, then: Then) {
        let Some(i) = self.index_of(id) else { return };
        let name = self.docs[i].name();
        let others = self.docs.iter().filter(|d| d.modified()).count() - 1;
        let message = if then == Then::Quit && others > 0 {
            format!("{name} has changes that aren't saved ({others} more files too).")
        } else {
            format!("{name} has changes that aren't saved.")
        };
        self.ask(
            Question::Unsaved(id, then),
            "Save changes?",
            &message,
            &["Save", "Don't Save", "Cancel"],
            0,
            2,
        );
    }

    /// Carries on after a document's changes were saved or discarded.
    fn proceed(&mut self, ctx: &mut Ctx<'_>, then: Then) {
        match then {
            Then::Close(id) => self.remove_doc(id),
            Then::Quit => self.quit(ctx),
        }
    }

    pub fn save(&mut self, ctx: &mut Ctx<'_>, id: DocId, then: Option<Then>) {
        let Some(i) = self.index_of(id) else { return };
        if self.still_loading(i) {
            return;
        }
        let Some(path) = self.docs[i].path.clone() else {
            self.save_as(ctx, id, then);
            return;
        };
        if self.docs[i].saving.is_some() {
            self.say("Already saving…", false);
            return;
        }
        if self.docs[i].inexact {
            let doc = &self.docs[i];
            let message = format!(
                "{} has bytes that {} can't represent exactly. Saving will change them, even \
                 where you didn't edit.",
                doc.name(),
                doc.view.editor.format().encoding.name()
            );
            self.ask(
                Question::Inexact(id, then),
                "Save anyway?",
                &message,
                &["Save", "Cancel"],
                0,
                1,
            );
            return;
        }
        let snapshot = self.docs[i].view.editor.buffer().snapshot();
        let (tx, rx) = mpsc::channel();
        let waker = self.waker.clone().unwrap_or_else(|| ctx.waker());
        let job_path = path.clone();
        std::thread::spawn(move || {
            let result = fude_buffer::save(&job_path, &snapshot.text, &snapshot.format);
            let _ = tx.send(result);
            waker.wake();
        });
        self.docs[i].saving = Some(SaveJob {
            version: snapshot.version,
            path,
            then,
            result: rx,
        });
        self.say("Saving…", false);
    }

    pub fn save_as(&mut self, ctx: &mut Ctx<'_>, id: DocId, then: Option<Then>) {
        let Some(i) = self.index_of(id) else { return };
        if self.still_loading(i) {
            return;
        }
        let dir = self.directory(id);
        let name = self.docs[i].name();
        let options = FileDialog {
            title: "Save As",
            directory: dir.as_deref(),
            file_name: Some(&name),
        };
        if let Some(path) = ctx.save_file_dialog(&options) {
            let doc = &mut self.docs[i];
            doc.path = Some(path);
            // A new file has no original bytes to worry about.
            doc.inexact = false;
            self.save(ctx, id, then);
        }
    }

    /// Picks up finished saves. Returns whether to redraw.
    pub fn poll_saves(&mut self, ctx: &mut Ctx<'_>) -> bool {
        let mut redraw = false;
        for i in 0..self.docs.len() {
            let Some(job) = &self.docs[i].saving else {
                continue;
            };
            let Ok(result) = job.result.try_recv() else {
                continue;
            };
            let job = self.docs[i].saving.take().expect("checked");
            self.saved(ctx, self.docs[i].id, job, result);
            redraw = true;
        }
        redraw
    }

    fn saved(
        &mut self,
        ctx: &mut Ctx<'_>,
        id: DocId,
        job: SaveJob,
        result: Result<FileStamp, SaveError>,
    ) {
        let Some(i) = self.index_of(id) else { return };
        let doc = &mut self.docs[i];
        let name = doc.name();
        let encoding = doc.view.editor.format().encoding.name();
        let then = job.then;
        match result {
            Ok(stamp) => {
                if doc.path.as_ref() == Some(&job.path) {
                    doc.view.editor.buffer_mut().mark_saved(job.version);
                    doc.stamp = Some(stamp);
                }
                self.say(format!("Saved {name}"), false);
                if job.path.parent().is_some()
                    && crate::config::config_path().is_some_and(|c| same_file(&c, &job.path))
                {
                    self.reload_config();
                }
                if let Some(then) = then {
                    self.proceed(ctx, then);
                }
            }
            Err(SaveError::Unmappable { offset, ch, .. }) => {
                let line = doc.view.editor.text().byte_to_line(offset) + 1;
                self.ask(
                    Question::NotEncodable(id, then),
                    "Can't save in this encoding",
                    &format!("\"{ch}\" on line {line} can't be saved as {encoding}. Save as UTF-8 instead?"),
                    &["Save as UTF-8", "Cancel"],
                    0,
                    1,
                );
            }
            Err(SaveError::InvalidUtf8 { offset, .. }) => {
                let line = doc.view.editor.text().byte_to_line(offset) + 1;
                self.ask(
                    Question::NotEncodable(id, then),
                    "Can't save in this encoding",
                    &format!(
                        "Line {line} has bytes that aren't valid text, which only UTF-8 can \
                         save as they are. Save as UTF-8 instead?"
                    ),
                    &["Save as UTF-8", "Cancel"],
                    0,
                    1,
                );
            }
            Err(SaveError::ReadOnly) => {
                self.ask(
                    Question::ReadOnly(id, then),
                    "File is read-only",
                    &format!("{name} is read-only. Save a copy somewhere else?"),
                    &["Save As…", "Cancel"],
                    0,
                    1,
                );
            }
            Err(e) => {
                self.ask(
                    Question::Info,
                    "Couldn't save",
                    &format!("{name} couldn't be saved: {e}"),
                    &["OK"],
                    0,
                    0,
                );
            }
        }
    }

    pub fn answer(&mut self, ctx: &mut Ctx<'_>, choice: usize) {
        let Some((_, question)) = self.prompt.take() else {
            return;
        };
        match (question, choice) {
            (Question::Unsaved(id, then), 0) => self.save(ctx, id, Some(then)),
            (Question::Unsaved(id, then), 1) => {
                // Don't Save: the document closes either way.
                self.remove_doc(id);
                if let Then::Quit = then {
                    self.quit(ctx);
                }
            }
            (Question::NotEncodable(id, then), 0) => {
                if let Some(i) = self.index_of(id) {
                    let doc = &mut self.docs[i];
                    let editor = &mut doc.view.editor;
                    let format = Format {
                        encoding: Encoding::for_label(b"utf-8").expect("UTF-8"),
                        bom: false,
                        ..*editor.format()
                    };
                    let sels = editor.selections().clone();
                    editor.buffer_mut().set_format(format, sels.clone(), sels);
                    doc.inexact = false;
                    self.save(ctx, id, then);
                }
            }
            (Question::Inexact(id, then), 0) => {
                if let Some(i) = self.index_of(id) {
                    self.docs[i].inexact = false;
                    self.save(ctx, id, then);
                }
            }
            (Question::ReadOnly(id, then), 0) => self.save_as(ctx, id, then),
            (Question::ChangedOnDisk(id), 0) => self.reload(id),
            (Question::OpenLarge(path), 0) => self.open_now(&path),
            (Question::About, 0) => {
                ctx.set_clipboard_text(format!("fude {}", crate::app::platform_version()));
                self.say("Copied the version", false);
            }
            (Question::ChangedOnDisk(id), _) => {
                // Keep the edited text; the file on disk no longer matches any version.
                if let Some(i) = self.index_of(id) {
                    let doc = &mut self.docs[i];
                    doc.view.editor.buffer_mut().mark_unsaved();
                    doc.stamp = doc.path.as_deref().and_then(|p| FileStamp::of(p).ok());
                }
            }
            _ => {}
        }
    }

    /// When the window regains focus, notices if another program changed open files.
    pub fn check_disk(&mut self) {
        if self.prompt.is_some() {
            return;
        }
        for i in 0..self.docs.len() {
            let doc = &self.docs[i];
            if doc.saving.is_some() || doc.loading.is_some() {
                continue;
            }
            let (Some(path), Some(stamp)) = (&doc.path, doc.stamp) else {
                continue;
            };
            let Ok(current) = FileStamp::of(path) else {
                continue;
            };
            if current == stamp {
                continue;
            }
            let id = doc.id;
            if doc.modified() {
                let name = doc.name();
                self.activate(i);
                self.ask(
                    Question::ChangedOnDisk(id),
                    "File changed on disk",
                    &format!(
                        "{name} was changed by another program. Reload it and lose your \
                         changes, or keep your version?"
                    ),
                    &["Reload", "Keep Mine"],
                    1,
                    1,
                );
                return;
            }
            self.reload(id);
        }
    }

    /// Reads the file again (it changed on disk), in the background.
    fn reload(&mut self, id: DocId) {
        let Some(i) = self.index_of(id) else { return };
        let Some(path) = self.docs[i].path.clone() else {
            return;
        };
        self.start_load(id, &path, None, LoadKind::Reload);
        self.settle_loads();
    }
}

impl Fude {
    /// Says so if document `i` is still loading (it can't be saved or edited yet).
    pub fn still_loading(&mut self, i: usize) -> bool {
        if self.docs[i].loading.is_none() {
            return false;
        }
        let name = self.docs[i].name();
        self.say(
            format!("{name} is still loading; it can be edited once it's read"),
            false,
        );
        true
    }
}

/// A size in bytes for people: "3.4 GB".
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    let mut size = bytes as f64 / 1024.0;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

fn same_file(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

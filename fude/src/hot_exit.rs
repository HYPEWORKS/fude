//! Hot exit: keeping the session (open tabs and unsaved text) on disk, so quitting never
//! has to ask and a crash loses at most a few seconds of typing.
//!
//! Two seconds after anything changes, a worker writes `session.toml` and the text of
//! each document with unsaved changes (see [`crate::session`] for the files). Quitting
//! writes right away, on the UI thread. A document counts as backed up only once its
//! write has succeeded: a failed one is retried, and quitting asks about that document
//! instead of leaving without its text.
//!
//! Only the single instance keeps a session. A window opened with `--new-instance` or
//! `--wait` has none (two processes writing one session would delete each other's
//! backups), so it asks about unsaved changes when it closes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fude_buffer::{Format, LineEnding, Rope};

use crate::app::Fude;
use crate::doc::DocId;
use crate::loading::LoadKind;
use crate::session::{self, Session, Stored, Tab};

/// How long after a change the session (and unsaved text) is written.
const SESSION_DELAY: Duration = Duration::from_secs(2);
/// Unsaved text longer than this is backed up at most every [`BIG_BACKUP_EVERY`] (and on
/// quitting): rewriting hundreds of MB every couple of seconds of typing would keep the
/// disk busy. A crash can lose that much more typing in such a file.
const BIG_BACKUP: usize = 32 << 20;
const BIG_BACKUP_EVERY: Duration = Duration::from_secs(30);
/// How long after a failed write the next one is tried.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// What the session looked like when last written: each tab's document, version and
/// path, then the active tab.
type Print = Vec<(u64, u64, Option<PathBuf>)>;

/// The session's bookkeeping: what is on disk, and what is being written.
#[derive(Default)]
pub struct HotExit {
    /// When to write the session next.
    pub due: Option<Instant>,
    print: Print,
    /// The version of each document whose unsaved text is safely in its backup.
    backed_up: HashMap<DocId, u64>,
    /// When each document's backup was last written.
    backup_at: HashMap<DocId, Instant>,
    /// The write running on a worker thread. One at a time.
    writer: Option<JoinHandle<Written>>,
}

/// One document's unsaved text, to be written to its backup.
struct BackupWrite {
    doc: DocId,
    version: u64,
    name: String,
    text: Rope,
}

/// What a session write managed to do.
#[derive(Default)]
struct Written {
    /// The documents (and versions) whose backups are now on disk.
    backed_up: Vec<(DocId, u64)>,
    /// The documents whose backups couldn't be written.
    failed: Vec<DocId>,
    /// The first thing that went wrong, for the status bar.
    error: Option<String>,
}

/// Writes the backups, then the session, then removes backups no tab uses any more.
fn write(state: &Path, backups: &[BackupWrite], session: &Session) -> Written {
    let mut written = Written::default();
    for backup in backups {
        match session::write_backup(state, &backup.name, &backup.text) {
            Ok(()) => written.backed_up.push((backup.doc, backup.version)),
            Err(e) => {
                written.failed.push(backup.doc);
                written
                    .error
                    .get_or_insert(format!("Couldn't back up unsaved changes: {e}"));
            }
        }
    }
    match session.write(state) {
        Ok(()) => {
            let keep: Vec<&str> = session
                .tabs
                .iter()
                .filter_map(|t| t.backup.as_deref())
                .collect();
            session::clean_backups(state, &keep);
        }
        // The session on disk still names the older backups: leave them all.
        Err(e) => {
            written
                .error
                .get_or_insert(format!("Couldn't save the session: {e}"));
        }
    }
    written
}

impl Fude {
    fn session_print(&self) -> Print {
        self.docs
            .iter()
            .map(|d| {
                (
                    d.id.0,
                    d.view.editor.buffer().version().get(),
                    d.path.clone(),
                )
            })
            .chain([(u64::MAX, self.active as u64, None)])
            .collect()
    }

    /// Schedules a session write when tabs or text changed since the last one.
    pub fn schedule_session(&mut self, now: Instant) {
        if self.state_dir.is_none() || self.hot_exit.due.is_some() {
            return;
        }
        if self.session_print() != self.hot_exit.print {
            self.hot_exit.due = Some(now + SESSION_DELAY);
        }
    }

    /// Takes in a background session write that has finished. Returns whether to redraw.
    pub fn poll_session(&mut self) -> bool {
        let finished = self
            .hot_exit
            .writer
            .as_ref()
            .is_some_and(JoinHandle::is_finished);
        if finished {
            self.finish_session_write();
        }
        finished
    }

    /// Waits for the background write, if one is running, and takes in what it did.
    fn finish_session_write(&mut self) {
        let Some(writer) = self.hot_exit.writer.take() else {
            return;
        };
        let written = writer.join().unwrap_or_else(|_| Written {
            error: Some("Saving the session stopped unexpectedly".to_owned()),
            ..Written::default()
        });
        self.session_written(written);
    }

    /// Records what a write did. Returns a document whose unsaved text is *not* on disk,
    /// if a backup failed.
    fn session_written(&mut self, written: Written) -> Option<DocId> {
        let at = Instant::now();
        for (id, version) in written.backed_up {
            self.hot_exit.backed_up.insert(id, version);
            self.hot_exit.backup_at.insert(id, at);
        }
        if let Some(error) = written.error {
            self.say(error, true);
            let retry = at + RETRY_AFTER;
            self.hot_exit.due = Some(self.hot_exit.due.map_or(retry, |d| d.min(retry)));
        }
        written
            .failed
            .into_iter()
            .find(|&id| self.index_of(id).is_some_and(|i| self.docs[i].modified()))
    }

    /// Writes the session, and the text of documents with unsaved changes, to the state
    /// directory: on a worker, or right away when `now` (quitting). With `now`, returns a
    /// document whose unsaved text couldn't be written, if there is one: quitting must
    /// not leave without asking about it.
    pub fn save_session(&mut self, now: bool) -> Option<DocId> {
        let state = self.state_dir.clone()?;
        if now {
            // One writer at a time, and this one has the last word.
            self.finish_session_write();
        } else if self.hot_exit.writer.is_some() {
            self.hot_exit.due = Some(Instant::now() + SESSION_DELAY);
            return None;
        }
        self.hot_exit.due = None;
        let mut backups: Vec<BackupWrite> = Vec::new();
        let mut tabs = Vec::new();
        let mut active = 0;
        let mut deferred: Option<Instant> = None;
        let active_index = self.active;
        let Fude { docs, hot_exit, .. } = self;
        for (i, doc) in docs.iter_mut().enumerate() {
            // A document still loading is saved as what it's loading: its backup stays,
            // and the cursor goes back where the last session had it.
            let pending = doc
                .loading
                .as_ref()
                .filter(|l| !matches!(l.kind, LoadKind::Reload));
            let backup = if let Some(loading) = pending {
                match loading.kind {
                    LoadKind::Backup { .. } => doc.backup.clone(),
                    _ => None,
                }
            } else if doc.modified() {
                let name = doc
                    .backup
                    .get_or_insert_with(session::new_backup_name)
                    .clone();
                let version = doc.view.editor.buffer().version().get();
                let big = doc.view.editor.text().len() > BIG_BACKUP;
                let due = match hot_exit.backup_at.get(&doc.id) {
                    Some(&at) if big && !now => at + BIG_BACKUP_EVERY,
                    _ => Instant::now(),
                };
                let stale = hot_exit.backed_up.get(&doc.id) != Some(&version);
                if stale && due > Instant::now() {
                    // Keep the older backup for now.
                    deferred = Some(deferred.map_or(due, |d| d.min(due)));
                } else if stale {
                    backups.push(BackupWrite {
                        doc: doc.id,
                        version,
                        name: name.clone(),
                        text: doc.view.editor.text().clone(),
                    });
                }
                Some(name)
            } else {
                doc.backup = None;
                hot_exit.backed_up.remove(&doc.id);
                hot_exit.backup_at.remove(&doc.id);
                None
            };
            if doc.path.is_none() && backup.is_none() {
                continue;
            }
            if i <= active_index {
                active = tabs.len();
            }
            let (format, stamp) = match pending.map(|l| &l.kind) {
                Some(LoadKind::Backup { format, stamp, .. }) => (*format, *stamp),
                _ => (*doc.view.editor.format(), doc.stamp),
            };
            let (cursor, scroll) = pending.and_then(|l| l.restore).unwrap_or_else(|| {
                (
                    doc.view.editor.selections().primary().head,
                    doc.view.top_line(),
                )
            });
            tabs.push(Tab {
                path: doc.path.clone(),
                backup,
                encoding: format.encoding.name().to_owned(),
                bom: format.bom,
                line_ending: format.line_ending,
                cursor,
                scroll,
                stamp,
            });
        }
        let session = Session { active, tabs };
        self.hot_exit.print = self.session_print();
        // A backup put off comes due by itself, with nothing else changing.
        self.hot_exit.due = deferred;
        if now {
            let written = write(&state, &backups, &session);
            return self.session_written(written);
        }
        let waker = self.waker.clone();
        self.hot_exit.writer = Some(std::thread::spawn(move || {
            let written = write(&state, &backups, &session);
            if let Some(waker) = waker {
                waker.wake();
            }
            written
        }));
        None
    }

    /// Reopens the tabs of the last session, with their unsaved changes. Backups no tab
    /// names (the session file was lost or cut short) come back as untitled documents
    /// rather than being thrown away.
    pub fn restore_session(&mut self) {
        let Some(state) = self.state_dir.clone() else {
            return;
        };
        let session = match Session::read(&state) {
            Stored::Session(session) => session,
            Stored::Nothing => Session::default(),
            Stored::Unreadable => {
                session::set_aside(&state);
                self.say(
                    "The last session couldn't be read; unsaved changes were recovered",
                    true,
                );
                Session::default()
            }
        };
        let mut restored = Vec::new();
        let mut used: Vec<&str> = Vec::new();
        for tab in &session.tabs {
            let restore = Some((tab.cursor, tab.scroll));
            let backup = tab
                .backup
                .as_ref()
                .map(|name| (name, session::backup_dir(&state).join(name)))
                .filter(|(_, source)| source.is_file());
            let id = match (backup, &tab.path) {
                (Some((name, source)), path) => {
                    used.push(name);
                    let kind = LoadKind::Backup {
                        format: tab.format(),
                        path: path.clone(),
                        stamp: tab.stamp,
                    };
                    self.restore_backup(kind, &source, name, restore)
                }
                (None, Some(path)) if path.is_file() => {
                    self.open_loading(LoadKind::Open, path, restore)
                }
                _ => continue,
            };
            restored.push(id);
        }
        for name in session::list_backups(&state) {
            if used.contains(&name.as_str()) {
                continue;
            }
            let kind = LoadKind::Backup {
                format: Format {
                    line_ending: LineEnding::PLATFORM,
                    ..Format::default()
                },
                path: None,
                stamp: None,
            };
            let source = session::backup_dir(&state).join(&name);
            restored.push(self.restore_backup(kind, &source, &name, None));
        }
        self.settle_loads();
        // Tabs whose file couldn't be read are gone by now, or go when their load fails.
        if let Some(&id) = restored.get(session.active).or(restored.last())
            && let Some(i) = self.index_of(id)
        {
            self.active = i;
            self.tabs.reveal_active();
        }
    }

    /// Opens a tab that loads its unsaved text from the backup `name`.
    fn restore_backup(
        &mut self,
        kind: LoadKind,
        source: &Path,
        name: &str,
        restore: Option<(usize, f64)>,
    ) -> DocId {
        let id = self.open_loading(kind, source, restore);
        if let Some(i) = self.index_of(id) {
            self.docs[i].backup = Some(name.to_owned());
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn state(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fude-hot-exit-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A launch: the last session restored and loaded.
    fn launch(state: &Path) -> Fude {
        let mut app = Fude::new(Config::default());
        app.state_dir = Some(state.to_path_buf());
        app.restore_session();
        app.finish_loading();
        app
    }

    fn type_text(app: &mut Fude, text: &str) {
        app.doc_mut().view.editor.insert(text, Instant::now());
    }

    fn texts(app: &Fude) -> Vec<String> {
        app.docs
            .iter()
            .map(|d| String::from_utf8_lossy(&d.view.editor.text().to_vec()).into_owned())
            .collect()
    }

    fn backups(state: &Path) -> usize {
        session::list_backups(state).len()
    }

    #[test]
    fn unsaved_text_comes_back() {
        let dir = state("round-trip");
        let mut app = launch(&dir);
        type_text(&mut app, "first note");
        app.new_doc();
        type_text(&mut app, "second note");
        assert_eq!(app.save_session(true), None);
        let next = launch(&dir);
        assert_eq!(texts(&next), ["first note", "second note"]);
        assert!(next.docs.iter().all(|d| d.modified()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_file_that_cannot_be_read_keeps_the_backups() {
        let dir = state("unreadable");
        let mut app = launch(&dir);
        type_text(&mut app, "unsaved work");
        app.save_session(true);
        assert_eq!(backups(&dir), 1);

        // Cut short, as after a power cut.
        std::fs::write(dir.join("session.toml"), "").unwrap();
        let mut next = launch(&dir);
        assert_eq!(texts(&next), ["unsaved work"]);
        assert!(next.message.as_ref().is_some_and(|(m, _)| m.error));
        // What the timer does two seconds after launch.
        next.save_session(true);
        assert_eq!(backups(&dir), 1, "the backup of the unsaved text stays");
        assert!(dir.join("session.unreadable.toml").exists());
        assert_eq!(texts(&launch(&dir)), ["unsaved work"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_backup_no_tab_names_comes_back_untitled() {
        let dir = state("orphan");
        let mut app = launch(&dir);
        type_text(&mut app, "in the session");
        app.save_session(true);
        session::write_backup(&dir, "stray.txt", &Rope::from("left behind")).unwrap();
        let next = launch(&dir);
        assert_eq!(texts(&next), ["in the session", "left behind"]);
        assert!(next.docs[1].path.is_none() && next.docs[1].modified());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_backup_that_fails_is_tried_again_and_quitting_knows() {
        use std::os::unix::fs::PermissionsExt;
        let dir = state("fails");
        let backup_dir = session::backup_dir(&dir);
        std::fs::create_dir_all(&backup_dir).unwrap();
        let allow = |mode| {
            std::fs::set_permissions(&backup_dir, std::fs::Permissions::from_mode(mode)).unwrap();
        };
        allow(0o555);
        if std::fs::write(backup_dir.join("probe"), "").is_ok() {
            // Running as root: nothing is unwritable.
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let mut app = launch(&dir);
        type_text(&mut app, "unsaved work");
        let id = app.doc().id;
        assert_eq!(app.save_session(true), Some(id), "quitting is told");
        assert!(app.message.as_ref().is_some_and(|(m, _)| m.error));
        assert!(app.hot_exit.due.is_some(), "and it's tried again");
        assert!(!app.hot_exit.backed_up.contains_key(&id));

        allow(0o755);
        assert_eq!(app.save_session(true), None);
        assert_eq!(backups(&dir), 1);
        assert_eq!(texts(&launch(&dir)), ["unsaved work"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quitting_waits_for_the_background_write() {
        let dir = state("background");
        let mut app = launch(&dir);
        type_text(&mut app, "one");
        app.save_session(false);
        assert!(app.hot_exit.writer.is_some());
        // A tab the background write doesn't know about, then quit at once.
        app.new_doc();
        type_text(&mut app, "two");
        assert_eq!(app.save_session(true), None);
        assert!(app.hot_exit.writer.is_none());
        assert_eq!(texts(&launch(&dir)), ["one", "two"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_changed_while_fude_was_closed_is_noticed() {
        let dir = state("stamp");
        let file = dir.join("notes.txt");
        std::fs::write(&file, "on disk\n").unwrap();
        let mut app = launch(&dir);
        app.open(&file);
        app.finish_loading();
        type_text(&mut app, "mine ");
        app.save_session(true);
        let stamp = app.doc().stamp;
        assert!(stamp.is_some());

        // Another program rewrites the file while fude isn't running.
        std::fs::write(&file, "changed by someone else, and longer\n").unwrap();
        let mut next = launch(&dir);
        assert_eq!(texts(&next), ["mine on disk\n"]);
        assert_eq!(
            next.doc().stamp,
            stamp,
            "the stamp is the one fude last saw"
        );
        next.check_disk();
        assert!(
            matches!(
                next.prompt,
                Some((_, crate::app::Question::ChangedOnDisk(_)))
            ),
            "so the change is asked about"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

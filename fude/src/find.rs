//! Find and replace: the bar's actions, and the searches that run on workers.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use fude_buffer::{Rope, RopeBuilder, Version};
use fude_editor::{Query, Searcher};
use fude_ui::{FindAction, FindStatus};

use crate::app::{Focus, Fude};
use crate::doc::DocId;

/// Up to this many matches, the count keeps their positions to say "3 of 120".
const KEEP_STARTS: usize = 200_000;

/// Replace All with more matches than this builds the new text on the worker instead of
/// editing the document match by match on the UI thread (about 2 µs each).
const REBUILD_OVER: usize = 10_000;

/// What Replace All worked out on its worker.
#[derive(Debug, PartialEq)]
pub enum Replaced {
    /// Few matches: edits to apply, which share the rest of the text with the undo step.
    Edits(Vec<(Range<usize>, Vec<u8>)>),
    /// Many: the whole new text, and how many matches it replaced.
    Text(Rope, usize),
}

/// Replaces every match in `text`. Past `rebuild_over` matches, the new text is built
/// here, so applying it costs the same however many there were.
fn replace_all_in(
    searcher: &Searcher,
    text: &Rope,
    replacement: &str,
    rebuild_over: usize,
) -> Replaced {
    let matches = searcher.matches_in(text, 0..text.len());
    if matches.len() <= rebuild_over {
        let edits = matches
            .into_iter()
            .map(|r| {
                let bytes = searcher.replacement(text, r.clone(), replacement);
                (r, bytes)
            })
            .collect();
        return Replaced::Edits(edits);
    }
    let mut builder = RopeBuilder::new();
    let mut at = 0;
    for r in &matches {
        for chunk in text.chunks_in(at..r.start) {
            builder.push(chunk);
        }
        builder.push(&searcher.replacement(text, r.clone(), replacement));
        at = r.end;
    }
    for chunk in text.chunks_in(at..text.len()) {
        builder.push(chunk);
    }
    Replaced::Text(builder.finish(), matches.len())
}

/// A count of all matches, computed on a worker.
pub struct Count {
    pub doc: DocId,
    pub version: Version,
    pub query: Query,
    pub total: usize,
    pub starts: Option<Vec<usize>>,
}

pub struct CountJob {
    rx: mpsc::Receiver<Count>,
    cancel: Arc<AtomicBool>,
}

impl Drop for CountJob {
    /// A count nobody waits for stops at its next window.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct ReplaceAllJob {
    doc: DocId,
    version: Version,
    rx: mpsc::Receiver<Replaced>,
}

/// Find state shared by all documents.
#[derive(Default)]
pub struct FindState {
    pub open: bool,
    pub searcher: Option<Searcher>,
    /// Where incremental search starts from: the cursor when typing began.
    pub origin: Option<usize>,
    pub count: Option<Count>,
    pub count_job: Option<CountJob>,
    /// The document changed while a count ran: count again when it finishes.
    pub recount: bool,
    pub replace_job: Option<ReplaceAllJob>,
}

impl Fude {
    /// Ctrl+F / Ctrl+H: opens the bar, seeded with the selection if it's one line.
    pub fn open_find(&mut self, replace: bool, now: Instant) {
        let editor = &self.doc().view.editor;
        let sel = *editor.selections().primary();
        let seed = (!sel.is_empty() && sel.range().len() < 500).then(|| {
            let mut bytes = Vec::new();
            editor.text().copy_range(sel.range(), &mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        });
        let seed = seed.filter(|s| !s.contains(['\n', '\r']));
        self.find.open(replace, seed.as_deref(), now);
        self.find_state.open = true;
        self.find_state.origin = Some(sel.start());
        self.focus = Focus::Find;
        self.doc_mut().view.set_focused(false, now);
        self.query_changed(now);
    }

    pub fn close_find(&mut self, now: Instant) {
        self.find_state.open = false;
        self.focus = Focus::Editor;
        self.find.blur(now);
        self.doc_mut().view.set_focused(true, now);
    }

    pub fn find_action(&mut self, action: FindAction, now: Instant) {
        match action {
            FindAction::QueryChanged => self.query_changed(now),
            FindAction::Next => self.find_next(false, now),
            FindAction::Previous => self.find_next(true, now),
            FindAction::ReplaceOne => self.replace_one(now),
            FindAction::ReplaceAll => self.replace_all(),
            FindAction::Close => self.close_find(now),
            FindAction::Redraw => {}
        }
    }

    /// Compiles the query and jumps to the first match from where typing began.
    fn query_changed(&mut self, now: Instant) {
        let query = self.find.query();
        self.find_state.count = None;
        self.find_state.count_job = None;
        match Searcher::new(&query) {
            Err(e) => {
                self.find_state.searcher = None;
                self.find.status = FindStatus::Error(e.clone());
                self.say(format!("Invalid regex: {e}"), true);
            }
            Ok(None) => {
                self.find_state.searcher = None;
                self.find.status = FindStatus::Empty;
            }
            Ok(Some(searcher)) => {
                let origin = self.find_state.origin.unwrap_or(0);
                let found = searcher.next(self.doc().view.editor.text(), origin);
                match found {
                    Some(range) => {
                        let view = &mut self.doc_mut().view;
                        view.editor.select(range);
                        view.changed(now);
                        self.find.status = FindStatus::Counting;
                        self.start_count(&searcher, query);
                    }
                    None => self.find.status = FindStatus::NoMatches,
                }
                self.find_state.searcher = Some(searcher);
            }
        }
    }

    /// F3, Enter: selects the next (or previous) match.
    pub fn find_next(&mut self, backwards: bool, now: Instant) {
        if self.find_state.searcher.is_none() {
            // F3 with nothing searched yet: open the bar instead.
            if !self.find.query().text.is_empty() {
                self.query_changed(now);
            } else {
                self.open_find(false, now);
                return;
            }
        }
        let Some(searcher) = &self.find_state.searcher else {
            return;
        };
        let editor = &self.doc().view.editor;
        let sel = *editor.selections().primary();
        let found = if backwards {
            searcher.previous(editor.text(), sel.start())
        } else {
            searcher.next(
                editor.text(),
                sel.end().max(sel.start() + usize::from(sel.is_empty())),
            )
        };
        match found {
            Some(range) => {
                self.find_state.origin = Some(range.start);
                let view = &mut self.doc_mut().view;
                view.editor.select(range);
                view.changed(now);
                self.update_find_status();
            }
            None => self.find.status = FindStatus::NoMatches,
        }
    }

    /// Replaces the selected match (if the selection is one) and moves to the next.
    fn replace_one(&mut self, now: Instant) {
        let Some(searcher) = self.find_state.searcher.clone() else {
            return;
        };
        let replacement = self.find.replacement();
        let view = &mut self.doc_mut().view;
        let sel = *view.editor.selections().primary();
        let is_match = !sel.is_empty()
            && searcher.find_from(view.editor.text(), sel.start()) == Some(sel.range());
        if is_match {
            let bytes = searcher.replacement(view.editor.text(), sel.range(), &replacement);
            let end = sel.start() + bytes.len();
            view.editor.apply_edits(&[(sel.range(), bytes)]);
            view.editor.select(end..end);
        }
        self.find_next(false, now);
        self.restart_count();
    }

    /// Computes every replacement on a worker; they're applied when it finishes, if the
    /// document hasn't changed in the meantime.
    fn replace_all(&mut self) {
        let Some(searcher) = self.find_state.searcher.clone() else {
            return;
        };
        let replacement = self.find.replacement();
        let doc = self.doc();
        let snapshot = doc.view.editor.buffer().snapshot();
        let (tx, rx) = mpsc::channel();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let replaced = replace_all_in(&searcher, &snapshot.text, &replacement, REBUILD_OVER);
            let _ = tx.send(replaced);
            if let Some(w) = waker {
                w.wake();
            }
        });
        self.find_state.replace_job = Some(ReplaceAllJob {
            doc: doc.id,
            version: snapshot.version,
            rx,
        });
        self.say("Replacing…", false);
    }

    fn start_count(&mut self, searcher: &Searcher, query: Query) {
        self.find_state.recount = false;
        let doc = self.doc();
        let snapshot = doc.view.editor.buffer().snapshot();
        let id = doc.id;
        let searcher = searcher.clone();
        let (tx, rx) = mpsc::channel();
        let waker = self.waker.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        std::thread::spawn(move || {
            let (total, starts) = searcher.count_until(&snapshot.text, KEEP_STARTS, &stop);
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let _ = tx.send(Count {
                doc: id,
                version: snapshot.version,
                query,
                total,
                starts,
            });
            if let Some(w) = waker {
                w.wake();
            }
        });
        self.find_state.count_job = Some(CountJob { rx, cancel });
    }

    /// Counts again after the document changed; one count at a time.
    pub fn restart_count(&mut self) {
        if self.find_state.count_job.is_some() {
            self.find_state.recount = true;
            return;
        }
        if let Some(searcher) = self.find_state.searcher.clone() {
            let query = self.find.query();
            self.start_count(&searcher, query);
        }
    }

    /// Picks up finished counts and replacements. Returns whether to redraw.
    pub fn poll_find(&mut self, now: Instant) -> bool {
        let mut redraw = false;
        if let Some(job) = &self.find_state.count_job
            && let Ok(count) = job.rx.try_recv()
        {
            self.find_state.count_job = None;
            self.find_state.count = Some(count);
            if std::mem::take(&mut self.find_state.recount) {
                self.restart_count();
            } else {
                self.update_find_status();
            }
            redraw = true;
        }
        if let Some(job) = &self.find_state.replace_job
            && let Ok(replaced) = job.rx.try_recv()
        {
            let (doc_id, version) = (job.doc, job.version);
            self.find_state.replace_job = None;
            redraw = true;
            match self.index_of(doc_id) {
                Some(i) if self.docs[i].view.editor.buffer().version() == version => {
                    let view = &mut self.docs[i].view;
                    let n = match replaced {
                        Replaced::Edits(edits) => {
                            view.editor.apply_edits(&edits);
                            edits.len()
                        }
                        Replaced::Text(text, n) => {
                            let format = *view.editor.format();
                            view.editor.replace_content(text, format);
                            n
                        }
                    };
                    view.changed(now);
                    self.say(
                        format!("Replaced {n} {}", if n == 1 { "match" } else { "matches" }),
                        false,
                    );
                    self.restart_count();
                }
                _ => self.say(
                    "The document changed while replacing; nothing was replaced",
                    true,
                ),
            }
        }
        redraw
    }

    /// Says "3 of 120" if the selection is a counted match, or just the total.
    fn update_find_status(&mut self) {
        let doc = self.doc();
        let version = doc.view.editor.buffer().version();
        let Some(count) = &self.find_state.count else {
            return;
        };
        if count.doc != doc.id || count.version != version || count.query != self.find.query() {
            self.find.status = FindStatus::Counting;
            self.restart_count();
            return;
        }
        if count.total == 0 {
            self.find.status = FindStatus::NoMatches;
            return;
        }
        let start = doc.view.editor.selections().primary().start();
        let current = count
            .starts
            .as_ref()
            .and_then(|s| s.binary_search(&start).ok())
            .map(|i| i + 1);
        self.find.status = FindStatus::Count {
            current,
            total: count.total,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_all_builds_the_same_text_either_way() {
        let text = Rope::from("one two one\r\nthree one".repeat(500).as_str());
        let query = Query {
            text: "one".into(),
            ..Query::default()
        };
        let searcher = Searcher::new(&query).unwrap().unwrap();
        let Replaced::Edits(edits) = replace_all_in(&searcher, &text, "1", usize::MAX) else {
            panic!("few matches are edits");
        };
        assert_eq!(edits.len(), 1500);
        let mut edited = text.clone();
        for (range, bytes) in edits.iter().rev() {
            edited.replace(range.clone(), bytes);
        }
        let Replaced::Text(built, n) = replace_all_in(&searcher, &text, "1", 0) else {
            panic!("many matches are a new text");
        };
        assert_eq!(n, 1500);
        assert_eq!(built.to_vec(), edited.to_vec());
        assert_eq!(
            built.to_vec(),
            "1 two 1\r\nthree 1".repeat(500).into_bytes()
        );

        let none = Query {
            text: "absent".into(),
            ..Query::default()
        };
        let searcher = Searcher::new(&none).unwrap().unwrap();
        assert_eq!(
            replace_all_in(&searcher, &text, "x", 0),
            Replaced::Edits(Vec::new())
        );
    }
}

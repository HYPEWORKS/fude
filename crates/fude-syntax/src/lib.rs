//! Syntax highlighting, done on a worker thread so the UI never waits for it.
//!
//! The UI sends a [`Request`] (a document snapshot and the lines it shows) whenever its
//! highlighting is missing or out of date, draws plain text meanwhile, and picks up the
//! [`Reply`] when the worker wakes it. The worker uses `syntect` (Sublime Text grammars,
//! loaded on first use) and keeps the lexer state every [`CHECKPOINT`] lines, so after an
//! edit it re-lexes only from the edited line, and scrolling back is free.
//!
//! Large files: jumping far past the last checkpoint starts lexing [`RESYNC`] lines above
//! the view with a fresh state, which is usually right; a background pass then fills in
//! exact checkpoints (for files up to [`BACKGROUND_LIMIT`]) and corrects what's shown.

mod comments;
mod style;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use fude_buffer::Rope;
use syntect::parsing::{ParseState, ScopeStack, SyntaxReference, SyntaxSet};

pub use comments::{Comments, comments};
pub use style::Style;
use style::StyleMap;

/// Lexer state is kept every this many lines.
pub const CHECKPOINT: usize = 64;
/// How far above the view lexing starts when there's no checkpoint near.
pub const RESYNC: usize = 1000;
/// Documents up to this size get a background pass for exact checkpoints throughout.
pub const BACKGROUND_LIMIT: usize = 16 << 20;
/// Longer lines aren't highlighted (grammars can be very slow on minified files).
pub const MAX_LINE: usize = 10_000;

/// A styled byte range of one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub style: Style,
}

/// What a document needs highlighted.
pub struct Request {
    pub doc: u64,
    /// The document version the snapshot is of; replies carry it back.
    pub version: u64,
    pub text: Rope,
    /// For choosing the grammar: the file name and the first line.
    pub file_name: Option<String>,
    /// The first line that changed since the last request for this document, if any.
    pub changed_from: Option<usize>,
    /// The lines wanted (the view and a margin).
    pub lines: Range<usize>,
}

/// Highlighted lines for a document version.
#[derive(Clone, Debug)]
pub struct Reply {
    pub doc: u64,
    pub version: u64,
    /// The grammar's name ("Rust", "Plain Text").
    pub language: String,
    pub first_line: usize,
    pub lines: Vec<Vec<Span>>,
}

enum Message {
    Request(Request),
    Close(u64),
}

/// The UI's handle on the worker.
pub struct Highlighter {
    tx: Sender<Message>,
    rx: Receiver<Reply>,
}

impl Highlighter {
    /// Starts the worker. `wake` is called after each reply is sent.
    pub fn start(wake: impl Fn() + Send + 'static) -> Highlighter {
        let (tx, worker_rx) = mpsc::channel();
        let (worker_tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("fude-syntax".into())
            .spawn(move || Worker::new(worker_rx, worker_tx, Box::new(wake)).run())
            .expect("spawning the highlighter thread");
        Highlighter { tx, rx }
    }

    pub fn request(&self, request: Request) {
        let _ = self.tx.send(Message::Request(request));
    }

    /// Forgets a closed document.
    pub fn close(&self, doc: u64) {
        let _ = self.tx.send(Message::Close(doc));
    }

    pub fn try_recv(&self) -> Option<Reply> {
        self.rx.try_recv().ok()
    }
}

/// Lexer state at the start of a line.
#[derive(Clone)]
struct Checkpoint {
    line: usize,
    parse: ParseState,
    scopes: ScopeStack,
}

struct DocState {
    syntax: Option<usize>,
    file_name: Option<String>,
    version: u64,
    text: Rope,
    /// Sorted by line; exact (lexed from the start of the document).
    checkpoints: Vec<Checkpoint>,
    /// The last lines requested, and whether they were highlighted from a guess.
    shown: Range<usize>,
    guessed: bool,
}

struct Worker {
    rx: Receiver<Message>,
    tx: Sender<Reply>,
    wake: Box<dyn Fn() + Send>,
    syntaxes: Option<SyntaxSet>,
    docs: HashMap<u64, DocState>,
    styles: StyleMap,
}

impl Worker {
    fn new(rx: Receiver<Message>, tx: Sender<Reply>, wake: Box<dyn Fn() + Send>) -> Worker {
        Worker {
            rx,
            tx,
            wake,
            syntaxes: None,
            docs: HashMap::new(),
            styles: StyleMap::default(),
        }
    }

    fn run(mut self) {
        loop {
            // Wait for work, unless there's background work to do.
            let first = if self.background_pending() {
                match self.rx.try_recv() {
                    Ok(m) => Some(m),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => return,
                }
            } else {
                match self.rx.recv() {
                    Ok(m) => Some(m),
                    Err(_) => return,
                }
            };
            let Some(first) = first else {
                self.background_step();
                continue;
            };
            // Take everything queued; only the newest request per document matters.
            let mut latest: Vec<Request> = Vec::new();
            let mut message = Some(first);
            while let Some(m) = message {
                match m {
                    Message::Close(doc) => {
                        self.docs.remove(&doc);
                        latest.retain(|r| r.doc != doc);
                    }
                    Message::Request(r) => {
                        if let Some(old) = latest.iter_mut().find(|o| o.doc == r.doc) {
                            let changed = match (old.changed_from, r.changed_from) {
                                (Some(a), Some(b)) => Some(a.min(b)),
                                (a, b) => a.or(b),
                            };
                            *old = Request {
                                changed_from: changed,
                                ..r
                            };
                        } else {
                            latest.push(r);
                        }
                    }
                }
                message = self.rx.try_recv().ok();
            }
            for request in latest {
                self.serve(request);
            }
        }
    }

    fn syntaxes(&mut self) -> &SyntaxSet {
        self.syntaxes
            .get_or_insert_with(SyntaxSet::load_defaults_newlines)
    }

    fn detect(&mut self, file_name: Option<&str>, first_line: &str) -> Option<usize> {
        let set = self.syntaxes();
        let by_name = file_name.and_then(|name| {
            let ext = name.rsplit_once('.').map_or(name, |(_, ext)| ext);
            set.find_syntax_by_extension(ext)
                .or_else(|| set.find_syntax_by_extension(name))
        });
        let syntax: &SyntaxReference =
            by_name.or_else(|| set.find_syntax_by_first_line(first_line))?;
        set.syntaxes().iter().position(|s| s.name == syntax.name)
    }

    fn serve(&mut self, r: Request) {
        let doc = r.doc;
        self.docs.entry(doc).or_insert_with(|| DocState {
            syntax: None,
            file_name: None,
            version: 0,
            text: Rope::new(),
            checkpoints: Vec::new(),
            shown: 0..0,
            guessed: false,
        });
        let needs_detect = {
            let state = &self.docs[&doc];
            state.file_name != r.file_name || state.checkpoints.is_empty() && state.syntax.is_none()
        };
        if needs_detect {
            let first_line = first_line(&r.text);
            let syntax = self.detect(r.file_name.as_deref(), &first_line);
            let state = self.docs.get_mut(&doc).expect("inserted");
            if state.syntax != syntax {
                state.checkpoints.clear();
            }
            state.syntax = syntax;
            state.file_name = r.file_name.clone();
        }
        let state = self.docs.get_mut(&doc).expect("inserted");
        if let Some(changed) = r.changed_from {
            // A checkpoint holds the state before its line, so one at the changed line
            // itself is still good.
            state.checkpoints.retain(|c| c.line <= changed);
        }
        state.version = r.version;
        state.text = r.text;
        let lines = r.lines.start..r.lines.end.min(state.text.line_count());
        state.shown = lines.clone();
        self.highlight(doc, lines);
    }

    /// Lexes `lines` of a document and sends them.
    fn highlight(&mut self, doc: u64, lines: Range<usize>) {
        let syntaxes = self
            .syntaxes
            .get_or_insert_with(SyntaxSet::load_defaults_newlines);
        let state = self.docs.get_mut(&doc).expect("known document");
        let language = state
            .syntax
            .map_or("Plain Text", |i| syntaxes.syntaxes()[i].name.as_str())
            .to_owned();
        let mut reply = Reply {
            doc,
            version: state.version,
            language,
            first_line: lines.start,
            lines: Vec::new(),
        };
        let Some(syntax_index) = state.syntax else {
            reply.lines = vec![Vec::new(); lines.len()];
            let _ = self.tx.send(reply);
            (self.wake)();
            return;
        };
        let syntax = &syntaxes.syntaxes()[syntax_index];
        // Start from the nearest checkpoint, or guess near the view if that's far.
        let nearest = state
            .checkpoints
            .iter()
            .rev()
            .find(|c| c.line <= lines.start)
            .cloned();
        let (mut line, mut parse, mut scopes, exact) = match nearest {
            Some(c) if lines.start - c.line <= RESYNC * 2 => (c.line, c.parse, c.scopes, true),
            _ if lines.start <= RESYNC * 2 => (0, ParseState::new(syntax), ScopeStack::new(), true),
            _ => (
                lines.start - RESYNC,
                ParseState::new(syntax),
                ScopeStack::new(),
                false,
            ),
        };
        let text = state.text.clone();
        let mut buf = Vec::new();
        while line < lines.end {
            if exact && line % CHECKPOINT == 0 && !state.checkpoints.iter().any(|c| c.line == line)
            {
                let at = state.checkpoints.partition_point(|c| c.line < line);
                state.checkpoints.insert(
                    at,
                    Checkpoint {
                        line,
                        parse: parse.clone(),
                        scopes: scopes.clone(),
                    },
                );
            }
            let spans = lex_line(
                &text,
                line,
                &mut buf,
                &mut parse,
                &mut scopes,
                syntaxes,
                &mut self.styles,
            );
            if line >= lines.start {
                reply.lines.push(spans);
            }
            line += 1;
        }
        state.guessed = !exact;
        let _ = self.tx.send(reply);
        (self.wake)();
    }

    /// Whether some document could use more exact checkpoints.
    fn background_pending(&self) -> bool {
        self.docs
            .values()
            .any(|d| d.syntax.is_some() && background_from(d).is_some())
    }

    /// Lexes a stretch past the last checkpoint of one document; re-sends its view if it
    /// was guessed and is now covered.
    fn background_step(&mut self) {
        let Some((&doc, from)) = self
            .docs
            .iter()
            .find_map(|(id, d)| Some((id, background_from(d)?)))
        else {
            return;
        };
        let syntaxes = self
            .syntaxes
            .get_or_insert_with(SyntaxSet::load_defaults_newlines);
        let state = self.docs.get_mut(&doc).expect("found");
        let syntax = &syntaxes.syntaxes()[state.syntax.expect("has syntax")];
        let (mut line, mut parse, mut scopes) = match state.checkpoints.last() {
            Some(c) => (c.line, c.parse.clone(), c.scopes.clone()),
            None => (0, ParseState::new(syntax), ScopeStack::new()),
        };
        debug_assert!(line <= from);
        let text = state.text.clone();
        let end = (line + CHECKPOINT * 32).min(text.line_count());
        let mut buf = Vec::new();
        while line < end {
            if line % CHECKPOINT == 0 && state.checkpoints.last().is_none_or(|c| c.line < line) {
                state.checkpoints.push(Checkpoint {
                    line,
                    parse: parse.clone(),
                    scopes: scopes.clone(),
                });
            }
            lex_line(
                &text,
                line,
                &mut buf,
                &mut parse,
                &mut scopes,
                syntaxes,
                &mut self.styles,
            );
            line += 1;
        }
        let state = &self.docs[&doc];
        let covered = state
            .checkpoints
            .last()
            .is_some_and(|c| c.line + CHECKPOINT * 2 >= state.shown.start);
        if state.guessed && covered {
            let shown = state.shown.clone();
            self.docs.get_mut(&doc).expect("found").guessed = false;
            self.highlight(doc, shown);
        }
    }
}

/// Where a document's background pass would continue, if it should.
fn background_from(d: &DocState) -> Option<usize> {
    if d.text.len() > BACKGROUND_LIMIT {
        return None;
    }
    let last = d.checkpoints.last().map_or(0, |c| c.line);
    let lines = d.text.line_count();
    (last + CHECKPOINT < lines).then_some(last)
}

fn first_line(text: &Rope) -> String {
    let span = text.line_span(0);
    let mut bytes = Vec::new();
    text.copy_range(span.start..span.end.min(span.start + 256), &mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Lexes one line, advancing the state, and returns its spans (empty for lines that are
/// too long or not valid UTF-8, which still advance the state as far as they can).
fn lex_line(
    text: &Rope,
    line: usize,
    buf: &mut Vec<u8>,
    parse: &mut ParseState,
    scopes: &mut ScopeStack,
    syntaxes: &SyntaxSet,
    styles: &mut StyleMap,
) -> Vec<Span> {
    let span = text.line_span(line);
    if span.next - span.start > MAX_LINE {
        return Vec::new();
    }
    buf.clear();
    text.copy_range(span.start..span.end, buf);
    // The grammars expect lines ending in \n.
    buf.push(b'\n');
    let Ok(s) = std::str::from_utf8(buf) else {
        return Vec::new();
    };
    let Ok(ops) = parse.parse_line(s, syntaxes) else {
        *parse = ParseState::new(syntaxes.syntaxes().first().expect("plain text"));
        return Vec::new();
    };
    let content = (span.end - span.start) as u32;
    let mut spans: Vec<Span> = Vec::new();
    let mut pos = 0u32;
    let push = |start: u32, end: u32, style: Style, spans: &mut Vec<Span>| {
        let end = end.min(content);
        if start >= end || style == Style::Plain {
            return;
        }
        match spans.last_mut() {
            Some(last) if last.end == start && last.style == style => last.end = end,
            _ => spans.push(Span { start, end, style }),
        }
    };
    for (at, op) in ops {
        let at = at as u32;
        if at > pos {
            push(pos, at, styles.style(scopes), &mut spans);
            pos = at;
        }
        let _ = scopes.apply(&op);
    }
    push(pos, content, styles.style(scopes), &mut spans);
    spans
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    fn wait(h: &Highlighter) -> Reply {
        for _ in 0..500 {
            if let Some(r) = h.try_recv() {
                return r;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("no reply");
    }

    fn style_of(reply: &Reply, line: usize, text: &str, needle: &str) -> Style {
        let at = text.lines().nth(line).unwrap().find(needle).unwrap() as u32;
        reply.lines[line - reply.first_line]
            .iter()
            .find(|s| s.start <= at && at < s.end)
            .map_or(Style::Plain, |s| s.style)
    }

    #[test]
    fn highlights_rust_and_updates_after_edits() {
        let woke = Arc::new(AtomicUsize::new(0));
        let w = woke.clone();
        let h = Highlighter::start(move || {
            w.fetch_add(1, Ordering::Relaxed);
        });
        let src = "// note\nfn main() {\n    let s = \"hi\";\n    let n = 42;\n}\n";
        let text = Rope::from(src);
        h.request(Request {
            doc: 1,
            version: 1,
            text: text.clone(),
            file_name: Some("main.rs".into()),
            changed_from: None,
            lines: 0..5,
        });
        let r = wait(&h);
        assert_eq!(r.language, "Rust");
        assert_eq!(r.lines.len(), 5);
        assert_eq!(style_of(&r, 0, src, "note"), Style::Comment);
        assert_eq!(style_of(&r, 1, src, "fn"), Style::Storage);
        assert_eq!(style_of(&r, 1, src, "main"), Style::Function);
        assert_eq!(style_of(&r, 2, src, "hi"), Style::String);
        assert_eq!(style_of(&r, 3, src, "42"), Style::Number);
        assert!(woke.load(Ordering::Relaxed) >= 1);

        // Opening a block comment on line 1 turns the following lines into comment.
        let mut edited = text.clone();
        let at = src.find("fn").unwrap();
        edited.insert(at, b"/* ");
        let src2 = String::from_utf8(edited.to_vec()).unwrap();
        h.request(Request {
            doc: 1,
            version: 2,
            text: edited,
            file_name: Some("main.rs".into()),
            changed_from: Some(1),
            lines: 0..5,
        });
        let r = wait(&h);
        assert_eq!(r.version, 2);
        assert_eq!(style_of(&r, 3, &src2, "42"), Style::Comment);
    }

    #[test]
    fn unknown_files_are_plain_text() {
        let h = Highlighter::start(|| {});
        h.request(Request {
            doc: 7,
            version: 1,
            text: Rope::from("just some words\n"),
            file_name: Some("notes.unknown-extension".into()),
            changed_from: None,
            lines: 0..1,
        });
        let r = wait(&h);
        assert_eq!(r.language, "Plain Text");
        assert!(r.lines[0].is_empty());
    }

    #[test]
    fn shebang_picks_the_grammar() {
        let h = Highlighter::start(|| {});
        h.request(Request {
            doc: 3,
            version: 1,
            text: Rope::from("#!/usr/bin/env python3\nprint('x')\n"),
            file_name: Some("script".into()),
            changed_from: None,
            lines: 0..2,
        });
        assert_eq!(wait(&h).language, "Python");
    }
}

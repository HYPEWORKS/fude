//! The application: one document in one window, files, saving, prompts.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fude_buffer::{FileStamp, Format, LineEnding, Loaded, Rope, SaveError, Version};
use fude_editor::Editor;
use fude_platform::{Canvas, Ctx, CursorShape, Event, FileDialog, Key, KeyInput, Waker};
use fude_render::Rect;
use fude_ui::{
    Action, EditorView, Painter, Prompt, PromptInput, STATUS_BAR_HEIGHT, SUMI, StatusMessage,
    TAB_STRIP_HEIGHT, action_for, status_bar, tab_strip,
};

/// How long a status bar message stays.
const MESSAGE_FOR: Duration = Duration::from_secs(4);

pub struct Doc {
    pub view: EditorView,
    pub path: Option<PathBuf>,
    /// The file as last read or written, to notice changes made by other programs.
    stamp: Option<FileStamp>,
    /// Saving would also change bytes the user didn't edit (the encoding can't represent
    /// them exactly); asked about once before the first save.
    inexact: bool,
}

impl Doc {
    pub fn untitled() -> Doc {
        let format = Format {
            line_ending: LineEnding::PLATFORM,
            ..Format::default()
        };
        Doc::new(Rope::new(), format, None)
    }

    pub fn new(text: Rope, format: Format, path: Option<PathBuf>) -> Doc {
        Doc {
            view: EditorView::new(Editor::new(text, format)),
            path,
            stamp: None,
            inexact: false,
        }
    }

    pub fn loaded(loaded: Loaded, path: PathBuf) -> Doc {
        let mut doc = Doc::new(loaded.text, loaded.format, Some(path));
        doc.stamp = loaded.stamp;
        doc.inexact = !loaded.exact;
        doc
    }

    pub fn name(&self) -> String {
        match &self.path {
            Some(p) => p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            None => "Untitled".to_owned(),
        }
    }

    fn modified(&self) -> bool {
        self.view.editor.buffer().is_modified()
    }
}

/// What to do once unsaved changes are dealt with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Then {
    New,
    Open,
    Quit,
}

/// Which question a prompt asks, and what happens after.
enum Question {
    /// Save, Don't Save, Cancel.
    Unsaved(Then),
    /// Save as UTF-8, Cancel.
    NotEncodable(Option<Then>),
    /// Save, Cancel.
    Inexact(Option<Then>),
    /// Save As…, Cancel.
    ReadOnly(Option<Then>),
    /// Reload, Keep Mine.
    ChangedOnDisk,
    /// OK.
    Info,
}

struct SaveJob {
    version: Version,
    path: PathBuf,
    then: Option<Then>,
    result: mpsc::Receiver<Result<FileStamp, SaveError>>,
}

pub struct Fude {
    pub doc: Doc,
    pub painter: Painter,
    prompt: Option<(Prompt, Question)>,
    message: Option<(StatusMessage, Instant)>,
    saving: Option<SaveJob>,
    waker: Option<Waker>,
    title: String,
    /// Window size, physical pixels.
    size: (u32, u32),
    editor_rect: Rect,
    pub last_frame: Duration,
}

impl Fude {
    pub fn new(doc: Doc) -> Fude {
        Fude {
            doc,
            painter: Painter::new(&SUMI),
            prompt: None,
            message: None,
            saving: None,
            waker: None,
            title: String::new(),
            size: (0, 0),
            editor_rect: Rect::default(),
            last_frame: Duration::ZERO,
        }
    }

    pub fn window_title(&self) -> String {
        let dot = if self.doc.modified() { "• " } else { "" };
        format!("{dot}{} · fude", self.doc.name())
    }

    // --- Drawing -----------------------------------------------------------------------

    pub fn draw(&mut self, canvas: &mut Canvas<'_>, scale: f32, now: Instant) {
        let start = Instant::now();
        let (w, h) = (canvas.width() as i32, canvas.height() as i32);
        self.size = (w as u32, h as u32);
        let p = &mut self.painter;
        p.begin(scale);
        let tabs_h = p.px(TAB_STRIP_HEIGHT);
        let status_h = p.px(STATUS_BAR_HEIGHT);
        let tabs = Rect::new(0, 0, w, tabs_h);
        let status = Rect::new(0, h - status_h, w, status_h);
        self.editor_rect = Rect::new(0, tabs_h, w, (h - tabs_h - status_h).max(0));

        let modified = self.doc.modified();
        tab_strip(p, tabs, &self.doc.name(), modified);
        self.doc.view.draw(p, self.editor_rect, now);

        let editor = &self.doc.view.editor;
        let format = editor.format();
        let line_endings = if editor.text().line_ending_counts().is_mixed() {
            "Mixed".to_owned()
        } else {
            format.line_ending.name().to_owned()
        };
        let items = [
            format.encoding_label(),
            line_endings,
            "Plain Text".to_owned(),
        ];
        let position = self.doc.view.position_label();
        let message = self.message.as_ref().map(|(m, _)| m);
        status_bar(p, status, &position, message, &items);

        if let Some((prompt, _)) = &mut self.prompt {
            prompt.draw(p, Rect::new(0, 0, w, h));
        }
        p.finish(canvas);
        self.last_frame = start.elapsed();
    }

    // --- Events --------------------------------------------------------------------------

    pub fn event(&mut self, ctx: &mut Ctx<'_>, event: Event) {
        if self.waker.is_none() {
            self.waker = Some(ctx.waker());
        }
        let now = Instant::now();
        let redraw = match event {
            Event::CloseRequested => {
                self.unless_unsaved(ctx, Then::Quit);
                true
            }
            Event::Resized { .. } | Event::ScaleChanged(_) => true,
            Event::Focused(focused) => {
                self.doc.view.set_focused(focused, now);
                if focused {
                    self.check_disk(ctx);
                }
                true
            }
            Event::Key(key) => self.key(ctx, &key, now),
            Event::Ime(ime) => {
                if self.prompt.is_none() {
                    self.doc.view.ime(ime, now);
                }
                true
            }
            Event::Scroll {
                dx,
                dy,
                unit,
                modifiers,
            } => {
                if self.prompt.is_none() {
                    self.doc.view.scroll(dx, dy, unit, modifiers);
                }
                true
            }
            Event::MouseMove { x, y, .. } => {
                if let Some((prompt, _)) = &mut self.prompt {
                    ctx.set_cursor(CursorShape::Default);
                    prompt.mouse_move(x, y)
                } else {
                    ctx.set_cursor(self.doc.view.cursor_shape(x, y));
                    self.doc.view.mouse_move(&mut self.painter, x, y)
                }
            }
            Event::MouseDown {
                x,
                y,
                button,
                clicks,
                modifiers,
            } => {
                if let Some((prompt, _)) = &mut self.prompt {
                    if let Some(choice) = prompt.mouse_down(x, y) {
                        self.answer(ctx, choice);
                    }
                    true
                } else if button == fude_platform::MouseButton::Left {
                    self.doc
                        .view
                        .mouse_down(&mut self.painter, x, y, clicks, modifiers, now)
                } else {
                    false
                }
            }
            Event::MouseUp { .. } => {
                self.doc.view.mouse_up();
                true
            }
            Event::MouseLeave => self.doc.view.mouse_leave(),
            Event::Timer => {
                if self.message.as_ref().is_some_and(|(_, at)| *at <= now) {
                    self.message = None;
                }
                self.doc.view.tick(&mut self.painter, now)
            }
            Event::Wake => self.poll_save(ctx),
        };
        let next = [
            self.doc.view.next_timer(now),
            self.message.as_ref().map(|(_, at)| *at),
        ]
        .into_iter()
        .flatten()
        .min();
        ctx.set_timer(next);
        let title = self.window_title();
        if title != self.title {
            ctx.set_title(&title);
            self.title = title;
        }
        if redraw {
            ctx.request_redraw();
        }
    }

    /// After a command panicked: says so, and leaves the document as it is.
    pub fn recover(&mut self, ctx: &mut Ctx<'_>, message: &str) {
        self.prompt = None;
        self.say(
            format!("Internal error: {message}. Your text is still here; save it if you can."),
            true,
        );
        ctx.request_redraw();
    }

    /// Places the IME candidate window at the caret.
    pub fn presented(&mut self, ctx: &mut Ctx<'_>) {
        if let Some(r) = self.doc.view.caret_rect() {
            ctx.set_ime_area(r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        }
    }

    fn key(&mut self, ctx: &mut Ctx<'_>, key: &KeyInput, now: Instant) -> bool {
        if let Some((prompt, _)) = &mut self.prompt {
            match prompt.key(key) {
                PromptInput::Chose(choice) => self.answer(ctx, choice),
                PromptInput::Handled | PromptInput::Ignored => {}
            }
            return true;
        }
        if let Some(action) = action_for(key) {
            self.action(ctx, action, now);
            return true;
        }
        if key.pressed && key.key == Key::Escape && self.message.is_some() {
            self.message = None;
        }
        self.doc.view.key(&mut self.painter, key, now)
    }

    fn action(&mut self, ctx: &mut Ctx<'_>, action: Action, now: Instant) {
        let view = &mut self.doc.view;
        match action {
            Action::New | Action::Close => self.unless_unsaved(ctx, Then::New),
            Action::Open => self.unless_unsaved(ctx, Then::Open),
            Action::Quit => self.unless_unsaved(ctx, Then::Quit),
            Action::Save => self.save(ctx, None),
            Action::SaveAs => self.save_as(ctx, None),
            Action::Undo => {
                view.editor.undo();
                view.changed(now);
            }
            Action::Redo => {
                view.editor.redo();
                view.changed(now);
            }
            Action::Copy => {
                let text = view.editor.copy();
                ctx.set_clipboard_text(text);
            }
            Action::Cut => {
                let text = view.editor.cut(now);
                ctx.set_clipboard_text(text);
                view.changed(now);
            }
            Action::Paste => {
                if let Some(text) = ctx.clipboard_text() {
                    view.editor.paste(&text, now);
                    view.changed(now);
                }
            }
            Action::SelectAll => view.editor.select_all(),
            Action::ZoomIn => view.zoom(1.0),
            Action::ZoomOut => view.zoom(-1.0),
            Action::ZoomReset => view.reset_zoom(),
        }
    }

    fn say(&mut self, text: impl Into<String>, error: bool) {
        self.message = Some((
            StatusMessage {
                text: text.into(),
                error,
            },
            Instant::now() + MESSAGE_FOR,
        ));
    }

    fn ask(
        &mut self,
        question: Question,
        title: &str,
        message: &str,
        buttons: &[&str],
        default: usize,
        cancel: usize,
    ) {
        self.prompt = Some((
            Prompt::new(title, message, buttons, default, cancel),
            question,
        ));
    }

    fn answer(&mut self, ctx: &mut Ctx<'_>, choice: usize) {
        let Some((_, question)) = self.prompt.take() else {
            return;
        };
        match (question, choice) {
            (Question::Unsaved(then), 0) => self.save(ctx, Some(then)),
            (Question::Unsaved(then), 1) => self.proceed(ctx, then),
            (Question::NotEncodable(then), 0) => {
                let editor = &mut self.doc.view.editor;
                let format = Format {
                    encoding: fude_buffer::Encoding::for_label(b"utf-8").expect("UTF-8"),
                    bom: false,
                    ..*editor.format()
                };
                let sels = editor.selections().clone();
                editor.buffer_mut().set_format(format, sels.clone(), sels);
                self.doc.inexact = false;
                self.save(ctx, then);
            }
            (Question::Inexact(then), 0) => {
                self.doc.inexact = false;
                self.save(ctx, then);
            }
            (Question::ReadOnly(then), 0) => self.save_as(ctx, then),
            (Question::ChangedOnDisk, 0) => self.reload(),
            (Question::ChangedOnDisk, _) => {
                // Keep the edited text; the file on disk no longer matches any version.
                self.doc.view.editor.buffer_mut().mark_unsaved();
                if let Some(path) = &self.doc.path {
                    self.doc.stamp = FileStamp::of(path).ok();
                }
            }
            _ => {}
        }
    }

    // --- Files ---------------------------------------------------------------------------

    /// Does `then`, after asking about unsaved changes if there are any.
    fn unless_unsaved(&mut self, ctx: &mut Ctx<'_>, then: Then) {
        if !self.doc.modified() {
            self.proceed(ctx, then);
            return;
        }
        let name = self.doc.name();
        self.ask(
            Question::Unsaved(then),
            "Save changes?",
            &format!("{name} has changes that aren't saved."),
            &["Save", "Don't Save", "Cancel"],
            0,
            2,
        );
    }

    fn proceed(&mut self, ctx: &mut Ctx<'_>, then: Then) {
        match then {
            Then::Quit => ctx.exit(),
            Then::New => self.doc = Doc::untitled(),
            Then::Open => {
                let dir = self.directory();
                let options = FileDialog {
                    title: "Open",
                    directory: dir.as_deref(),
                    file_name: None,
                };
                if let Some(path) = ctx.open_file_dialog(&options) {
                    self.open(&path);
                }
            }
        }
    }

    fn directory(&self) -> Option<PathBuf> {
        self.doc
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
    }

    pub fn open(&mut self, path: &Path) {
        match fude_buffer::load(path) {
            Ok(loaded) => {
                let note = loaded_note(&loaded);
                self.doc = Doc::loaded(loaded, path.to_path_buf());
                if let Some(note) = note {
                    self.say(note, false);
                }
            }
            Err(e) => self.say(format!("Couldn't open {}: {e}", path.display()), true),
        }
    }

    fn save(&mut self, ctx: &mut Ctx<'_>, then: Option<Then>) {
        let Some(path) = self.doc.path.clone() else {
            self.save_as(ctx, then);
            return;
        };
        if self.saving.is_some() {
            self.say("Already saving…", false);
            return;
        }
        if self.doc.inexact {
            let encoding = self.doc.view.editor.format().encoding.name();
            self.ask(
                Question::Inexact(then),
                "Save anyway?",
                &format!(
                    "{} has bytes that {encoding} can't represent exactly. Saving will change \
                     them, even where you didn't edit.",
                    self.doc.name()
                ),
                &["Save", "Cancel"],
                0,
                1,
            );
            return;
        }
        let snapshot = self.doc.view.editor.buffer().snapshot();
        let (tx, rx) = mpsc::channel();
        let waker = self.waker.clone().unwrap_or_else(|| ctx.waker());
        let job_path = path.clone();
        std::thread::spawn(move || {
            let result = fude_buffer::save(&job_path, &snapshot.text, &snapshot.format);
            let _ = tx.send(result);
            waker.wake();
        });
        self.saving = Some(SaveJob {
            version: snapshot.version,
            path,
            then,
            result: rx,
        });
        self.say("Saving…", false);
    }

    fn save_as(&mut self, ctx: &mut Ctx<'_>, then: Option<Then>) {
        let dir = self.directory();
        let name = self.doc.name();
        let options = FileDialog {
            title: "Save As",
            directory: dir.as_deref(),
            file_name: Some(&name),
        };
        if let Some(path) = ctx.save_file_dialog(&options) {
            self.doc.path = Some(path);
            // A new file has no original bytes to worry about.
            self.doc.inexact = false;
            self.save(ctx, then);
        }
    }

    /// Picks up a finished save. Returns whether to redraw.
    fn poll_save(&mut self, ctx: &mut Ctx<'_>) -> bool {
        let Some(job) = &self.saving else {
            return false;
        };
        let Ok(result) = job.result.try_recv() else {
            return false;
        };
        let job = self.saving.take().expect("checked");
        let encoding = self.doc.view.editor.format().encoding.name();
        match result {
            Ok(stamp) => {
                if self.doc.path.as_ref() == Some(&job.path) {
                    self.doc.view.editor.buffer_mut().mark_saved(job.version);
                    self.doc.stamp = Some(stamp);
                }
                self.say(format!("Saved {}", self.doc.name()), false);
                if let Some(then) = job.then {
                    self.proceed(ctx, then);
                }
            }
            Err(SaveError::Unmappable { offset, ch, .. }) => {
                let line = self.doc.view.editor.text().byte_to_line(offset) + 1;
                self.ask(
                    Question::NotEncodable(job.then),
                    "Can't save in this encoding",
                    &format!("\"{ch}\" on line {line} can't be saved as {encoding}. Save as UTF-8 instead?"),
                    &["Save as UTF-8", "Cancel"],
                    0,
                    1,
                );
            }
            Err(SaveError::InvalidUtf8 { offset, .. }) => {
                let line = self.doc.view.editor.text().byte_to_line(offset) + 1;
                self.ask(
                    Question::NotEncodable(job.then),
                    "Can't save in this encoding",
                    &format!(
                        "Line {line} has bytes that aren't valid text, which only UTF-8 can save as they are. Save as UTF-8 instead?"
                    ),
                    &["Save as UTF-8", "Cancel"],
                    0,
                    1,
                );
            }
            Err(SaveError::ReadOnly) => {
                let name = self.doc.name();
                self.ask(
                    Question::ReadOnly(job.then),
                    "File is read-only",
                    &format!("{name} is read-only. Save a copy somewhere else?"),
                    &["Save As…", "Cancel"],
                    0,
                    1,
                );
            }
            Err(e) => {
                let name = self.doc.name();
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
        true
    }

    /// When the window regains focus, notices if another program changed the file.
    fn check_disk(&mut self, _ctx: &mut Ctx<'_>) {
        if self.saving.is_some() || self.prompt.is_some() {
            return;
        }
        let (Some(path), Some(stamp)) = (&self.doc.path, self.doc.stamp) else {
            return;
        };
        let Ok(current) = FileStamp::of(path) else {
            return;
        };
        if current == stamp {
            return;
        }
        if self.doc.modified() {
            let name = self.doc.name();
            self.ask(
                Question::ChangedOnDisk,
                "File changed on disk",
                &format!("{name} was changed by another program. Reload it and lose your changes, or keep your version?"),
                &["Reload", "Keep Mine"],
                1,
                1,
            );
        } else {
            self.reload();
        }
    }

    fn reload(&mut self) {
        let Some(path) = self.doc.path.clone() else {
            return;
        };
        match fude_buffer::load(&path) {
            Ok(loaded) => {
                let editor = &mut self.doc.view.editor;
                editor.replace_content(loaded.text, loaded.format);
                let version = editor.buffer().version();
                editor.buffer_mut().mark_saved(version);
                self.doc.stamp = loaded.stamp;
                self.doc.inexact = !loaded.exact;
                self.say(
                    format!("Reloaded {} (changed on disk)", self.doc.name()),
                    false,
                );
            }
            Err(e) => self.say(format!("Couldn't reload: {e}"), true),
        }
    }
}

/// Anything worth telling the user about how a file was read.
fn loaded_note(loaded: &Loaded) -> Option<String> {
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

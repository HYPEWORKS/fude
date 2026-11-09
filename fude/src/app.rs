//! The application: documents in tabs, input routing, drawing.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fude_editor::Selections;
use fude_platform::{
    Canvas, Ctx, CursorShape, Event, Key, KeyInput, MouseButton, OsTheme, ScrollUnit, Waker,
};
use fude_render::Rect;
use fude_ui::{
    Action, ContextMenu, ContextMenuItem, EditorView, FindBar, Painter, Palette, PaletteAction,
    PaletteItem, Prompt, PromptInput, STATUS_BAR_HEIGHT, StatusMessage, TAB_STRIP_HEIGHT, TabEvent,
    TabInfo, TabStrip, action_for, status_bar,
};

use crate::config::{self, Config};
use crate::doc::{Doc, DocId, Then};
use crate::find::FindState;
use crate::instance;
use crate::loading::LoadKind;
use crate::session::{self, Session, Tab};

/// How long after a change the session (and unsaved text) is written.
const SESSION_DELAY: Duration = Duration::from_secs(2);
/// Unsaved text longer than this is backed up at most every [`BIG_BACKUP_EVERY`] (and on
/// quitting): rewriting hundreds of MB every couple of seconds of typing would keep the
/// disk busy. A crash can lose that much more typing in such a file.
const BIG_BACKUP: usize = 32 << 20;
const BIG_BACKUP_EVERY: Duration = Duration::from_secs(30);

/// Lines highlighted beyond the visible ones, so short scrolls are already colored.
const HIGHLIGHT_MARGIN: usize = 60;

/// Where keys go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Find,
    Palette,
}

/// Where the cursor was before a go-to-line preview, to go back on Escape.
struct PaletteOrigin {
    doc: DocId,
    selections: Selections,
    scroll: fude_ui::ScrollPosition,
}

/// How long a status bar message stays.
const MESSAGE_FOR: Duration = Duration::from_secs(4);

/// Which question a prompt asks, and what happens after.
pub enum Question {
    /// Save, Don't Save, Cancel.
    Unsaved(DocId, Then),
    /// Save as UTF-8, Cancel.
    NotEncodable(DocId, Option<Then>),
    /// Save, Cancel.
    Inexact(DocId, Option<Then>),
    /// Save As…, Cancel.
    ReadOnly(DocId, Option<Then>),
    /// Reload, Keep Mine.
    ChangedOnDisk(DocId),
    /// Open, Cancel: a file over the size that asks first.
    OpenLarge(PathBuf),
    /// OK.
    Info,
}

pub struct Fude {
    pub docs: Vec<Doc>,
    pub active: usize,
    next_id: u64,
    pub painter: Painter,
    tabs: TabStrip,
    pub prompt: Option<(Prompt, Question)>,
    pub message: Option<(StatusMessage, Instant)>,
    pub waker: Option<Waker>,
    title: String,
    /// Last pointer position, physical pixels.
    pointer: (f32, f32),
    pub config: Config,
    pub find: FindBar,
    pub find_state: FindState,
    pub focus: Focus,
    palette: Palette,
    /// The action behind each palette item.
    palette_actions: Vec<Action>,
    palette_origin: Option<PaletteOrigin>,
    context_menu: ContextMenu,
    /// Window bounds from the last frame, for placing popups before they are drawn.
    window_area: Rect,
    /// Started on first use (it needs a waker), so startup doesn't pay for it.
    highlighter: Option<fude_syntax::Highlighter>,
    /// Where the session and backups live.
    pub state_dir: Option<PathBuf>,
    /// Other launches hand their files to this instance.
    pub instance: Option<instance::Server>,
    /// When to write the session next.
    session_due: Option<Instant>,
    /// What the session looked like when last written: tabs, versions, paths, active.
    session_print: Vec<(u64, u64, Option<PathBuf>)>,
    /// The version of each document whose unsaved text is in its backup.
    backed_up: HashMap<DocId, u64>,
    /// When each document's backup was last written.
    backup_at: HashMap<DocId, Instant>,
    session_writing: Arc<AtomicBool>,
    /// Files are being dragged over the window.
    drop_hover: bool,
    /// The caret area last given to the input method.
    ime_area: Option<Rect>,
    /// Files loading on worker threads report here.
    pub loads: crate::loading::Loads,
    /// A text tool running on a worker thread.
    pub tool_job: Option<crate::tools::ToolJob>,
    os_theme: Option<OsTheme>,
    /// Zoom steps from the configured font size, for every document.
    zoom: f32,
    pub last_frame: Duration,
}

impl Fude {
    pub fn new(config: Config) -> Fude {
        let mut app = Fude {
            docs: Vec::new(),
            active: 0,
            next_id: 0,
            painter: Painter::new(config.theme(None)),
            tabs: TabStrip::new(),
            prompt: None,
            message: None,
            waker: None,
            title: String::new(),
            pointer: (0.0, 0.0),
            config,
            find: FindBar::new(),
            find_state: FindState::default(),
            focus: Focus::Editor,
            palette: Palette::new(),
            palette_actions: Vec::new(),
            palette_origin: None,
            context_menu: ContextMenu::new(),
            window_area: Rect::default(),
            highlighter: None,
            state_dir: None,
            instance: None,
            session_due: None,
            session_print: Vec::new(),
            backed_up: HashMap::new(),
            backup_at: HashMap::new(),
            session_writing: Arc::new(AtomicBool::new(false)),
            drop_hover: false,
            ime_area: None,
            loads: Default::default(),
            tool_job: None,
            os_theme: None,
            zoom: 0.0,
            last_frame: Duration::ZERO,
        };
        let id = app.new_id();
        let mut doc = Doc::untitled(id);
        app.configure(&mut doc.view);
        app.docs.push(doc);
        app
    }

    /// Applies settings and zoom to a view.
    pub fn configure(&self, view: &mut EditorView) {
        view.set_font_size(self.config.font_size + self.zoom);
        view.editor.settings.tab_width = self.config.tab_width;
        view.editor.settings.insert_spaces = self.config.insert_spaces;
        view.wrap = self.config.word_wrap;
    }

    fn configure_all(&mut self) {
        let mut docs = std::mem::take(&mut self.docs);
        for doc in &mut docs {
            self.configure(&mut doc.view);
        }
        self.docs = docs;
    }

    /// Re-reads the settings file and applies it.
    pub fn reload_config(&mut self) {
        let (config, problems) = Config::load();
        self.config = config;
        self.configure_all();
        match problems.first() {
            Some(p) => self.say(p.clone(), true),
            None => self.say("Settings applied", false),
        }
    }

    /// Opens the settings file, creating it with every default if it doesn't exist.
    fn open_settings(&mut self) {
        let Some(path) = config::config_path() else {
            self.say("No settings directory on this system", true);
            return;
        };
        if !path.exists() {
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| std::fs::write(&path, config::TEMPLATE));
            if let Err(e) = written {
                self.say(format!("Couldn't create {}: {e}", path.display()), true);
                return;
            }
        }
        self.open(&path);
    }

    pub fn new_id(&mut self) -> DocId {
        self.next_id += 1;
        DocId(self.next_id)
    }

    pub fn doc(&self) -> &Doc {
        &self.docs[self.active]
    }

    pub fn doc_mut(&mut self) -> &mut Doc {
        &mut self.docs[self.active]
    }

    pub fn index_of(&self, id: DocId) -> Option<usize> {
        self.docs.iter().position(|d| d.id == id)
    }

    /// Adds a document as a new tab after the active one (or in place of a blank active
    /// tab) and switches to it.
    pub fn add_doc(&mut self, mut doc: Doc) {
        self.context_menu.close();
        self.configure(&mut doc.view);
        if self.doc().is_blank() {
            self.docs[self.active] = doc;
        } else {
            self.active += 1;
            self.docs.insert(self.active, doc);
        }
        self.tabs.reveal_active();
    }

    pub fn new_doc(&mut self) {
        self.context_menu.close();
        let id = self.new_id();
        let mut doc = Doc::untitled(id);
        self.configure(&mut doc.view);
        self.active = (self.active + 1).min(self.docs.len());
        self.docs.insert(self.active, doc);
        self.tabs.reveal_active();
    }

    pub fn activate(&mut self, index: usize) {
        if index < self.docs.len() && index != self.active {
            self.context_menu.close();
            self.active = index;
            self.tabs.reveal_active();
            self.doc_mut().view.set_focused(true, Instant::now());
        }
    }

    /// Removes a tab without asking. Closing the last tab leaves a new blank one.
    pub fn remove_doc(&mut self, id: DocId) {
        let Some(i) = self.index_of(id) else { return };
        self.context_menu.close();
        self.docs.remove(i);
        if let Some(h) = &self.highlighter {
            h.close(id.0);
        }
        if self.docs.is_empty() {
            let id = self.new_id();
            let mut doc = Doc::untitled(id);
            self.configure(&mut doc.view);
            self.docs.push(doc);
        }
        if i < self.active || self.active >= self.docs.len() {
            self.active = self.active.saturating_sub(1);
        }
        self.tabs.reveal_active();
    }

    pub fn window_title(&self) -> String {
        let doc = self.doc();
        let dot = if doc.modified() { "• " } else { "" };
        format!("{dot}{} · fude", doc.name())
    }

    pub fn say(&mut self, text: impl Into<String>, error: bool) {
        self.message = Some((
            StatusMessage {
                text: text.into(),
                error,
            },
            Instant::now() + MESSAGE_FOR,
        ));
    }

    pub fn ask(
        &mut self,
        question: Question,
        title: &str,
        message: &str,
        buttons: &[&str],
        default: usize,
        cancel: usize,
    ) {
        self.context_menu.close();
        self.prompt = Some((
            Prompt::new(title, message, buttons, default, cancel),
            question,
        ));
    }

    /// After a command panicked: says so, and leaves the documents as they are.
    pub fn recover(&mut self, ctx: &mut Ctx<'_>, message: &str) {
        self.prompt = None;
        self.context_menu.close();
        self.say(
            format!("Internal error: {message}. Your text is still here; save it if you can."),
            true,
        );
        ctx.request_redraw();
    }

    // --- Drawing -----------------------------------------------------------------------

    pub fn draw(&mut self, canvas: &mut Canvas<'_>, scale: f32, now: Instant) {
        let start = Instant::now();
        let (w, h) = (canvas.width() as i32, canvas.height() as i32);
        self.window_area = canvas.bounds();
        self.painter.theme = self.config.theme(self.os_theme);
        let p = &mut self.painter;
        p.begin(scale);
        let tabs_h = p.px(TAB_STRIP_HEIGHT);
        let status_h = p.px(STATUS_BAR_HEIGHT);
        let find_h = if self.find_state.open {
            p.px(self.find.height())
        } else {
            0
        };
        let editor = Rect::new(0, tabs_h, w, (h - tabs_h - status_h - find_h).max(0));

        let names: Vec<(String, bool)> =
            self.docs.iter().map(|d| (d.name(), d.modified())).collect();
        let infos: Vec<TabInfo<'_>> = names
            .iter()
            .map(|(title, modified)| TabInfo {
                title,
                modified: *modified,
            })
            .collect();
        self.tabs
            .draw(p, Rect::new(0, 0, w, tabs_h), &infos, self.active);

        let doc = &mut self.docs[self.active];
        doc.view.search = if self.find_state.open {
            self.find_state.searcher.clone()
        } else {
            None
        };
        doc.view.draw(p, editor, now);
        if self.find_state.open {
            self.find
                .draw(p, Rect::new(0, editor.bottom(), w, find_h), now);
        }

        let editor_state = &doc.view.editor;
        let format = editor_state.format();
        let line_endings = if editor_state.text().line_ending_counts().is_mixed() {
            "Mixed".to_owned()
        } else {
            format.line_ending.name().to_owned()
        };
        let items = [format.encoding_label(), line_endings, doc.language.clone()];
        let position = doc.view.position_label();
        let loading = doc.loading.as_ref().map(|l| StatusMessage {
            text: format!("Loading {}%…", l.percent()),
            error: false,
        });
        let message = loading.as_ref().or(self.message.as_ref().map(|(m, _)| m));
        status_bar(
            p,
            Rect::new(0, h - status_h, w, status_h),
            &position,
            message,
            &items,
        );

        if self.focus == Focus::Palette {
            self.palette.draw(p, Rect::new(0, 0, w, h), tabs_h, now);
        }
        self.context_menu.draw(p);
        if self.drop_hover {
            draw_drop_target(p, editor);
        }
        if let Some((prompt, _)) = &mut self.prompt {
            prompt.draw(p, Rect::new(0, 0, w, h));
        }
        p.finish(canvas);
        self.request_highlights();
        self.last_frame = start.elapsed();
    }

    /// Asks for syntax colors for the visible lines if they're missing or stale.
    pub fn request_highlights(&mut self) {
        let Some(highlighter) = &self.highlighter else {
            return;
        };
        let doc = &mut self.docs[self.active];
        let visible = doc.view.visible_lines();
        let lines_total = doc.view.editor.text().line_count();
        let want = visible.start.saturating_sub(HIGHLIGHT_MARGIN)
            ..(visible.end + HIGHLIGHT_MARGIN).min(lines_total);
        let version = doc.view.editor.buffer().version();
        let covered = doc
            .view
            .highlights
            .as_ref()
            .is_some_and(|h| h.covers(version.get(), &visible));
        let asked = doc.requested.as_ref().is_some_and(|(v, r)| {
            *v == version.get() && r.start <= visible.start && visible.end <= r.end
        });
        if covered || asked {
            return;
        }
        let text = doc.view.editor.text().clone();
        let changed_from = doc
            .view
            .editor
            .take_changed()
            .map(|at| text.byte_to_line(at.min(text.len())));
        highlighter.request(fude_syntax::Request {
            doc: doc.id.0,
            version: version.get(),
            text,
            file_name: doc.path.as_ref().map(|_| doc.name()),
            changed_from,
            lines: want.clone(),
        });
        doc.requested = Some((version.get(), want));
    }

    /// Takes in finished highlighting. Returns whether to redraw.
    pub fn poll_highlights(&mut self) -> bool {
        let Some(highlighter) = &self.highlighter else {
            return false;
        };
        let mut redraw = false;
        while let Some(reply) = highlighter.try_recv() {
            let Some(doc) = self.docs.iter_mut().find(|d| d.id.0 == reply.doc) else {
                continue;
            };
            doc.language = reply.language;
            // Python opens blocks with a colon.
            doc.view.editor.settings.indent_after_colon = doc.language == "Python";
            // Keep the newest colors; older ones still beat none while new ones are made.
            let newer = doc
                .view
                .highlights
                .as_ref()
                .is_none_or(|h| h.version <= reply.version);
            if newer {
                doc.view.highlights = Some(fude_ui::Highlights {
                    version: reply.version,
                    first_line: reply.first_line,
                    lines: reply.lines,
                });
            }
            redraw = true;
        }
        redraw
    }

    /// Starts the highlighter (once a waker exists to deliver its results).
    fn ensure_highlighter(&mut self) {
        if self.highlighter.is_none()
            && let Some(waker) = self.waker.clone()
        {
            self.highlighter = Some(fude_syntax::Highlighter::start(move || waker.wake()));
        }
    }

    /// For the modes without a window: highlights the visible lines and waits for them.
    pub fn highlight_now(&mut self) {
        if self.highlighter.is_none() {
            self.highlighter = Some(fude_syntax::Highlighter::start(|| {}));
        }
        self.docs[self.active].requested = None;
        self.request_highlights();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.poll_highlights() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    // --- Events ------------------------------------------------------------------------

    /// Targets a context click without starting a selection drag.
    fn open_editor_context_menu(&mut self, x: f32, y: f32, now: Instant) -> bool {
        if !self.docs[self.active]
            .view
            .context_menu_at(&mut self.painter, x, y, now)
        {
            return false;
        }
        self.show_editor_context_menu(x, y, now);
        true
    }

    fn show_editor_context_menu(&mut self, x: f32, y: f32, now: Instant) {
        self.focus = Focus::Editor;
        self.find.blur(now);
        self.find.mouse_up();
        self.doc_mut().view.set_focused(true, now);
        self.doc_mut().view.mouse_up();
        self.doc_mut().view.ime(fude_platform::Ime::Disabled, now);
        let items = self.editor_context_menu_items();
        self.context_menu
            .open(&mut self.painter, self.window_area, x, y, items);
    }

    fn editor_context_menu_items(&self) -> Vec<ContextMenuItem> {
        let editor = &self.doc().view.editor;
        let writable = !editor.is_read_only();
        let has_text = !editor.text().is_empty();
        let commands = fude_ui::commands();
        [
            Some(Action::Undo),
            Some(Action::Redo),
            None,
            Some(Action::Cut),
            Some(Action::Copy),
            Some(Action::Paste),
            None,
            Some(Action::SelectAll),
        ]
        .into_iter()
        .map(|action| {
            let enabled = match action {
                Some(Action::Undo) => writable && editor.buffer().can_undo(),
                Some(Action::Redo) => writable && editor.buffer().can_redo(),
                Some(Action::Cut) => writable && has_text,
                Some(Action::Copy | Action::SelectAll) => has_text,
                Some(Action::Paste) => writable,
                _ => false,
            };
            let command = commands.iter().find(|c| Some(c.0) == action);
            ContextMenuItem {
                action,
                label: command
                    .map(|c| c.1.rsplit(": ").next().unwrap_or(&c.1).to_owned())
                    .unwrap_or_default(),
                shortcut: command.map(|c| c.2.clone()).unwrap_or_default(),
                enabled,
            }
        })
        .collect()
    }

    pub fn event(&mut self, ctx: &mut Ctx<'_>, event: Event) {
        if self.waker.is_none() {
            self.waker = Some(ctx.waker());
            if let Some(server) = &self.instance {
                server.set_waker(ctx.waker());
            }
            self.loads.set_waker(ctx.waker());
            // Requests and loads that arrived before there was a waker.
            self.poll_instance(ctx);
            self.poll_loads();
        }
        self.ensure_highlighter();
        let now = Instant::now();
        let redraw = match event {
            Event::CloseRequested => {
                self.quit(ctx);
                true
            }
            Event::Resized { .. } | Event::ScaleChanged(_) => {
                self.context_menu.close();
                true
            }
            Event::ThemeChanged(theme) => {
                self.os_theme = Some(theme);
                true
            }
            Event::Focused(focused) => {
                if !focused {
                    self.context_menu.close();
                }
                self.doc_mut().view.set_focused(focused, now);
                if focused {
                    // Tell the input method again where the caret is.
                    self.ime_area = None;
                    self.check_disk();
                }
                true
            }
            Event::Key(key) => self.key(ctx, &key, now),
            Event::Ime(ime) => {
                // Only real changes redraw: an empty update redrawing would send the
                // caret area again, which KWin answers with another empty update.
                if self.prompt.is_some() || self.context_menu.is_open() {
                    false
                } else if self.focus == Focus::Palette {
                    if let Some(action) = self.palette.ime(ime, now) {
                        self.palette_action(ctx, action, now);
                        true
                    } else {
                        false
                    }
                } else if self.focus == Focus::Find {
                    if let Some(action) = self.find.ime(ime, now) {
                        self.find_action(action, now);
                        true
                    } else {
                        false
                    }
                } else {
                    self.doc_mut().view.ime(ime, now)
                }
            }
            Event::Scroll {
                dx,
                dy,
                unit,
                modifiers,
            } => {
                if self.context_menu.is_open() {
                    if self.context_menu.contains(self.pointer.0, self.pointer.1) {
                        let rows = match unit {
                            ScrollUnit::Lines => dy,
                            ScrollUnit::Pixels => dy / (28.0 * self.painter.scale),
                        };
                        let rows = if rows > 0.0 {
                            rows.ceil()
                        } else {
                            rows.floor()
                        };
                        self.context_menu.scroll(rows as i32);
                    } else {
                        self.context_menu.close();
                    }
                } else if self.prompt.is_none() {
                    if self.tabs.contains(self.pointer.0, self.pointer.1) {
                        let step = match unit {
                            ScrollUnit::Lines => 40.0 * self.painter.scale,
                            ScrollUnit::Pixels => 1.0,
                        };
                        self.tabs.scroll((dx + dy) * step);
                    } else {
                        let p = &mut self.painter;
                        self.docs[self.active]
                            .view
                            .scroll(p, dx, dy, unit, modifiers);
                    }
                }
                true
            }
            Event::MouseMove { x, y, .. } => self.mouse_move(ctx, x, y),
            Event::MouseDown {
                x,
                y,
                button,
                clicks,
                modifiers,
            } => {
                self.pointer = (x, y);
                let context_click = button == MouseButton::Right
                    || (cfg!(target_os = "macos") && button == MouseButton::Left && modifiers.ctrl);
                if let Some((prompt, _)) = &mut self.prompt {
                    if button == MouseButton::Left
                        && !context_click
                        && let Some(choice) = prompt.mouse_down(x, y)
                    {
                        self.answer(ctx, choice);
                    }
                    true
                } else if self.focus == Focus::Palette {
                    if button == MouseButton::Left && !context_click {
                        let p = &mut self.painter;
                        if let Some(action) =
                            self.palette.mouse_down(p, x, y, clicks, modifiers, now)
                        {
                            self.palette_action(ctx, action, now);
                        }
                    }
                    true
                } else if self.context_menu.is_open()
                    && (!context_click || self.context_menu.contains(x, y))
                {
                    if button == MouseButton::Left
                        && !context_click
                        && let Some(action) = self.context_menu.mouse_down(x, y)
                    {
                        self.action(ctx, action, now);
                    }
                    true
                } else if context_click {
                    self.context_menu.close();
                    let opened = self.open_editor_context_menu(x, y, now);
                    if opened {
                        ctx.set_cursor(CursorShape::Default);
                    }
                    // A context click never acts like a primary click in other controls.
                    true
                } else if self.tabs.contains(x, y) {
                    let event = self.tabs.mouse_down(x, y, button, clicks);
                    self.tab_event(ctx, event);
                    true
                } else if self.find_state.open && self.find.contains(x, y) {
                    if button == MouseButton::Left {
                        self.focus = Focus::Find;
                        self.doc_mut().view.set_focused(false, now);
                        let p = &mut self.painter;
                        if let Some(action) = self.find.mouse_down(p, x, y, clicks, modifiers, now)
                        {
                            self.find_action(action, now);
                        }
                    }
                    true
                } else if button == MouseButton::Left {
                    if self.focus == Focus::Find {
                        self.focus = Focus::Editor;
                        self.find.blur(now);
                        self.doc_mut().view.set_focused(true, now);
                    }
                    let p = &mut self.painter;
                    self.docs[self.active]
                        .view
                        .mouse_down(p, x, y, clicks, modifiers, now)
                } else {
                    false
                }
            }
            Event::MouseUp { x, y, .. } => {
                let event = self.tabs.mouse_up(x, y);
                self.tab_event(ctx, event);
                self.doc_mut().view.mouse_up();
                self.find.mouse_up();
                self.palette.mouse_up();
                true
            }
            Event::MouseLeave => {
                let menu = self.context_menu.mouse_move(-1.0, -1.0);
                let tabs = self.tabs.mouse_leave();
                self.doc_mut().view.mouse_leave() || tabs || menu
            }
            Event::Timer => {
                if self.message.as_ref().is_some_and(|(_, at)| *at <= now) {
                    self.message = None;
                }
                if self.session_due.is_some_and(|at| at <= now) {
                    self.save_session(false);
                }
                let p = &mut self.painter;
                self.docs[self.active].view.tick(p, now)
            }
            Event::Wake => {
                let version = self.doc().view.editor.buffer().version();
                let loads = self.poll_loads();
                let opened = self.poll_instance(ctx) || loads;
                let saves = self.poll_saves(ctx) || opened;
                let saves = self.poll_tools(ctx) || saves;
                let colors = self.poll_highlights();
                let find = self.poll_find(now);
                if saves || self.doc().view.editor.buffer().version() != version {
                    self.context_menu.close();
                }
                find || saves || colors
            }
            Event::DroppedFile(path) => {
                self.drop_hover = false;
                self.open(&path);
                true
            }
            Event::FileHover(on) => {
                self.drop_hover = on;
                true
            }
        };
        // Typing into a file that's still loading does nothing; say why.
        let refused = self.doc_mut().view.editor.take_refused();
        if refused {
            let active = self.active;
            self.still_loading(active);
        }
        let redraw = redraw || refused;
        let find_timer = match self.focus {
            Focus::Find if self.find_state.open => self.find.next_timer(now),
            Focus::Palette => self.palette.next_timer(now),
            _ => None,
        };
        self.schedule_session(now);
        let next = [
            self.doc().view.next_timer(now),
            find_timer,
            self.message.as_ref().map(|(_, at)| *at),
            self.session_due,
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

    /// Places the IME candidate window at the caret.
    pub fn presented(&mut self, ctx: &mut Ctx<'_>) {
        let caret = match self.focus {
            Focus::Find => self.find.caret_rect(),
            Focus::Palette => self.palette.caret_rect(),
            Focus::Editor => self.doc().view.caret_rect(),
        };
        // Only when it moved: on Wayland each update is a round trip to the compositor.
        if let Some(r) = caret
            && self.ime_area != Some(r)
        {
            self.ime_area = Some(r);
            ctx.set_ime_area(r.x as f32, r.y as f32, r.w as f32, r.h as f32);
        }
    }

    fn mouse_move(&mut self, ctx: &mut Ctx<'_>, x: f32, y: f32) -> bool {
        self.pointer = (x, y);
        if let Some((prompt, _)) = &mut self.prompt {
            ctx.set_cursor(CursorShape::Default);
            return prompt.mouse_move(x, y);
        }
        if self.context_menu.is_open() {
            ctx.set_cursor(CursorShape::Default);
            return self.context_menu.mouse_move(x, y);
        }
        if self.focus == Focus::Palette {
            ctx.set_cursor(CursorShape::Default);
            let p = &mut self.painter;
            return self.palette.mouse_move(p, x, y);
        }
        let (redraw_tabs, event) = self.tabs.mouse_move(x, y);
        self.tab_event(ctx, event);
        let over_find = self.find_state.open && self.find.contains(x, y);
        if self.tabs.contains(x, y) {
            ctx.set_cursor(CursorShape::Default);
        } else if over_find {
            ctx.set_cursor(CursorShape::Text);
        } else {
            ctx.set_cursor(self.doc().view.cursor_shape(x, y));
        }
        let p = &mut self.painter;
        let redraw_find = self.find_state.open && self.find.mouse_move(p, x, y);
        let redraw_view = self.docs[self.active].view.mouse_move(p, x, y);
        redraw_tabs || redraw_view || redraw_find
    }

    fn tab_event(&mut self, ctx: &mut Ctx<'_>, event: Option<TabEvent>) {
        match event {
            Some(TabEvent::Activate(i)) => self.activate(i),
            Some(TabEvent::Close(i)) => {
                if let Some(doc) = self.docs.get(i) {
                    let id = doc.id;
                    self.close(ctx, id);
                }
            }
            Some(TabEvent::Move { from, to }) => {
                let doc = self.docs.remove(from);
                self.docs.insert(to, doc);
                self.active = to;
            }
            Some(TabEvent::New) => self.new_doc(),
            None => {}
        }
    }

    fn key(&mut self, ctx: &mut Ctx<'_>, key: &KeyInput, now: Instant) -> bool {
        if let Some((prompt, _)) = &mut self.prompt {
            if let PromptInput::Chose(choice) = prompt.key(key) {
                self.answer(ctx, choice);
            }
            return true;
        }
        if self.context_menu.is_open() {
            if let Some(action) = self.context_menu.key(key) {
                self.action(ctx, action, now);
            }
            return true;
        }
        let m = key.modifiers;
        if self.focus == Focus::Editor
            && key.pressed
            && (key.key == Key::ContextMenu
                || (key.key == Key::F(10) && m.shift && !m.ctrl && !m.alt && !m.logo))
        {
            if let Some((x, y)) = self.docs[self.active]
                .view
                .context_menu_position(&mut self.painter)
            {
                self.show_editor_context_menu(x, y, now);
                ctx.set_cursor(CursorShape::Default);
            }
            return true;
        }
        if self.focus == Focus::Palette
            && let Some(action) = self.palette.key(key, now)
        {
            self.palette_action(ctx, action, now);
            return true;
        }
        if self.focus == Focus::Find
            && let Some(action) = self.find.key(key, now)
        {
            self.find_action(action, now);
            return true;
        }
        // External tools' own shortcuts come first, so settings can take any key.
        if let Some(i) = self
            .config
            .tools
            .iter()
            .position(|t| t.key.as_ref().is_some_and(|k| k.matches(key)))
        {
            self.action(ctx, Action::External(i as u16), now);
            return true;
        }
        if let Some(action) = action_for(key) {
            self.action(ctx, action, now);
            return true;
        }
        if self.focus != Focus::Editor {
            return false;
        }
        if key.pressed && key.key == Key::Escape {
            if self.find_state.open {
                self.close_find(now);
                return true;
            }
            self.message = None;
        }
        let version = self.doc().view.editor.buffer().version();
        let p = &mut self.painter;
        let handled = self.docs[self.active].view.key(p, key, now);
        if self.find_state.open && self.doc().view.editor.buffer().version() != version {
            self.restart_count();
        }
        handled
    }

    fn action(&mut self, ctx: &mut Ctx<'_>, action: Action, now: Instant) {
        self.context_menu.close();
        let n = self.docs.len();
        match action {
            Action::New => self.new_doc(),
            Action::Close => {
                let id = self.doc().id;
                self.close(ctx, id);
            }
            Action::Open => self.open_dialog(ctx),
            Action::Quit => self.quit(ctx),
            Action::Save => {
                let id = self.doc().id;
                self.save(ctx, id, None);
            }
            Action::SaveAs => {
                let id = self.doc().id;
                self.save_as(ctx, id, None);
            }
            Action::NextTab => self.activate((self.active + 1) % n),
            Action::PreviousTab => self.activate((self.active + n - 1) % n),
            Action::GoToTab(k) => {
                self.activate(if k == 9 { n - 1 } else { usize::from(k) - 1 });
            }
            // Editing actions go to the find bar's fields when they have focus.
            Action::Undo
            | Action::Redo
            | Action::Copy
            | Action::Cut
            | Action::Paste
            | Action::SelectAll
                if self.focus == Focus::Find =>
            {
                let input = self.find.focused_input();
                match action {
                    Action::Undo => {
                        input.editor.undo();
                    }
                    Action::Redo => {
                        input.editor.redo();
                    }
                    Action::Copy => ctx.set_clipboard_text(input.editor.copy()),
                    Action::Cut => ctx.set_clipboard_text(input.editor.cut(now)),
                    Action::SelectAll => input.select_all(),
                    _ => {
                        if let Some(text) = ctx.clipboard_text() {
                            let action = self.find.paste(&text, now);
                            self.find_action(action, now);
                        }
                    }
                }
                if matches!(action, Action::Undo | Action::Redo | Action::Cut) {
                    self.find_action(fude_ui::FindAction::QueryChanged, now);
                }
            }
            Action::Paste if self.focus == Focus::Palette => {
                if let Some(text) = ctx.clipboard_text() {
                    let action = self.palette.paste(&text, now);
                    self.palette_action(ctx, action, now);
                }
            }
            Action::CommandPalette => self.open_palette("", now),
            Action::GoToLine => self.open_palette(":", now),
            Action::Find => self.open_find(false, now),
            Action::Replace => self.open_find(true, now),
            Action::FindNext => self.find_next(false, now),
            Action::FindPrevious => self.find_next(true, now),
            Action::Undo => {
                let view = &mut self.doc_mut().view;
                view.editor.undo();
                view.changed(now);
            }
            Action::Redo => {
                let view = &mut self.doc_mut().view;
                view.editor.redo();
                view.changed(now);
            }
            Action::Copy => {
                let text = self.doc_mut().view.editor.copy();
                ctx.set_clipboard_text(text);
            }
            Action::Cut => {
                let view = &mut self.doc_mut().view;
                let text = view.editor.cut(now);
                view.changed(now);
                ctx.set_clipboard_text(text);
            }
            Action::Paste => {
                if let Some(text) = ctx.clipboard_text() {
                    let view = &mut self.doc_mut().view;
                    view.editor.paste(&text, now);
                    view.changed(now);
                }
            }
            Action::SelectAll => self.doc_mut().view.editor.select_all(),
            Action::ZoomIn | Action::ZoomOut | Action::ZoomReset => {
                self.zoom = match action {
                    Action::ZoomIn => self.zoom + 1.0,
                    Action::ZoomOut => self.zoom - 1.0,
                    _ => 0.0,
                };
                let size = (self.config.font_size + self.zoom).clamp(6.0, 72.0);
                self.zoom = size - self.config.font_size;
                self.configure_all();
            }
            Action::OpenSettings => self.open_settings(),
            Action::Tool(tool) => self.run_tool(ctx, tool),
            Action::External(i) => self.run_external(usize::from(i)),
            Action::LineEndings(le) => self.convert_line_endings(ctx, le),
            Action::AddCursorAbove | Action::AddCursorBelow => {
                let rows = if action == Action::AddCursorBelow {
                    1
                } else {
                    -1
                };
                let p = &mut self.painter;
                self.docs[self.active]
                    .view
                    .add_cursor_vertical(p, rows, now);
            }
            Action::GoToBracket => {
                if !self.doc_mut().view.go_to_bracket(now) {
                    self.say("No bracket next to the cursor, or no match for it", false);
                }
            }
            Action::ToggleComment | Action::ToggleBlockComment => {
                self.toggle_comment(action == Action::ToggleBlockComment, now);
            }
            Action::SelectAllOccurrences => {
                let n = self.doc_mut().view.editor.select_all_occurrences();
                self.doc_mut().view.changed(now);
                if n >= fude_editor::MAX_CURSORS {
                    self.say(format!("Selected the first {n} occurrences"), false);
                } else if n > 1 {
                    self.say(format!("{n} occurrences"), false);
                }
            }
            Action::SelectNextOccurrence
            | Action::SplitIntoLines
            | Action::DuplicateLine
            | Action::MoveLineUp
            | Action::MoveLineDown
            | Action::DeleteLine
            | Action::JoinLines
            | Action::InsertLineBelow
            | Action::InsertLineAbove
            | Action::Indent
            | Action::Outdent => {
                let view = &mut self.doc_mut().view;
                let e = &mut view.editor;
                match action {
                    Action::SelectNextOccurrence => {
                        e.select_next_occurrence();
                    }
                    Action::SplitIntoLines => e.split_into_lines(),
                    Action::DuplicateLine => e.duplicate(),
                    Action::MoveLineUp => e.move_lines(false),
                    Action::MoveLineDown => e.move_lines(true),
                    Action::DeleteLine => e.delete_lines(),
                    Action::JoinLines => e.join_lines(),
                    Action::InsertLineBelow => e.insert_line(false, now),
                    Action::InsertLineAbove => e.insert_line(true, now),
                    Action::Indent => e.indent_lines(),
                    _ => e.outdent_lines(),
                }
                view.changed(now);
            }
            Action::ToggleWrap => {
                // For this session; the setting in the file is the default.
                self.config.word_wrap = !self.config.word_wrap;
                self.configure_all();
                let state = if self.config.word_wrap { "on" } else { "off" };
                self.say(format!("Word wrap {state}"), false);
            }
        }
    }

    /// Toggles line comments (or with `block`, block comments) in the document's language.
    /// A language with only block comments gets them around each cursor's line.
    fn toggle_comment(&mut self, block: bool, now: Instant) {
        let doc = self.doc();
        let markers = fude_syntax::comments(&doc.language);
        let language = doc.language.clone();
        let view = &mut self.doc_mut().view;
        let editor = &mut view.editor;
        match (markers.line, markers.block) {
            (Some(token), _) if !block => editor.toggle_line_comment(token),
            (_, Some((open, close))) => {
                if !block && editor.selections().iter().all(|s| s.is_empty()) {
                    // Around the text of each cursor's line.
                    let text = editor.text().clone();
                    let lines = editor.selections().map(|s| {
                        let span = text.line_span(text.byte_to_line(s.head));
                        let start = fude_editor::text::indent_end(&text, span.start, span.end);
                        fude_editor::Selection::new(start, span.end)
                    });
                    editor.set_selections(lines);
                }
                editor.toggle_block_comment(open, close);
            }
            (Some(token), None) => editor.toggle_line_comment(token),
            (None, None) => {
                self.say(format!("{language} has no comments"), false);
                return;
            }
        }
        view.changed(now);
    }

    fn open_palette(&mut self, prefix: &str, now: Instant) {
        let mut commands = fude_ui::commands();
        commands.extend(self.config.tools.iter().enumerate().map(|(i, t)| {
            let key = t
                .key
                .as_ref()
                .map(fude_ui::Shortcut::label)
                .unwrap_or_default();
            (Action::External(i as u16), format!("Tool: {}", t.name), key)
        }));
        self.palette_actions = commands.iter().map(|c| c.0).collect();
        let items = commands
            .into_iter()
            .map(|(_, title, shortcut)| PaletteItem {
                title,
                detail: shortcut,
            })
            .collect();
        let doc = self.doc();
        self.palette_origin = Some(PaletteOrigin {
            doc: doc.id,
            selections: doc.view.editor.selections().clone(),
            scroll: doc.view.scroll_position(),
        });
        self.palette.open(items, prefix, now);
        self.focus = Focus::Palette;
        self.find.blur(now);
        self.doc_mut().view.set_focused(false, now);
    }

    fn close_palette(&mut self, restore: bool, now: Instant) {
        self.focus = Focus::Editor;
        self.palette.input.set_focused(false, now);
        let origin = self.palette_origin.take();
        if restore
            && let Some(origin) = origin
            && let Some(i) = self.index_of(origin.doc)
        {
            let view = &mut self.docs[i].view;
            view.editor.set_selections(origin.selections);
            view.set_scroll_position(origin.scroll);
        }
        self.doc_mut().view.set_focused(true, now);
    }

    fn palette_action(&mut self, ctx: &mut Ctx<'_>, action: PaletteAction, now: Instant) {
        match action {
            PaletteAction::Run(i) => {
                self.close_palette(false, now);
                if let Some(&action) = self.palette_actions.get(i) {
                    self.action(ctx, action, now);
                }
            }
            PaletteAction::Preview { line, column } => self.go_to(line, column, now),
            PaletteAction::GoTo { line, column } => {
                self.go_to(line, column, now);
                self.close_palette(false, now);
            }
            PaletteAction::Close => self.close_palette(true, now),
            PaletteAction::Redraw => {}
        }
    }

    /// Moves the cursor to a 1-based line and character column, centering it.
    fn go_to(&mut self, line: usize, column: Option<usize>, now: Instant) {
        let view = &mut self.doc_mut().view;
        let text = view.editor.text();
        let line = line.clamp(1, text.line_count()) - 1;
        let span = text.line_span(line);
        let mut at = span.start;
        if let Some(col) = column {
            let mut bytes = Vec::new();
            text.copy_range(span.start..span.end.min(span.start + (1 << 20)), &mut bytes);
            let s = String::from_utf8_lossy(&bytes);
            at += s
                .char_indices()
                .nth(col.saturating_sub(1))
                .map_or(span.end - span.start, |(i, _)| i)
                .min(span.end - span.start);
        }
        let at = fude_editor::text::floor_grapheme(text, at);
        view.editor.select(at..at);
        view.changed_centered(now);
    }

    // --- Single instance -----------------------------------------------------------------

    /// Opens files another launch handed over, and comes to the front. Returns whether
    /// anything arrived.
    fn poll_instance(&mut self, ctx: &mut Ctx<'_>) -> bool {
        let mut any = false;
        while let Some(request) = self.instance.as_ref().and_then(instance::Server::try_recv) {
            any = true;
            if request.files.is_empty() {
                self.new_doc();
            }
            for path in &request.files {
                self.open(path);
            }
        }
        if any {
            ctx.focus_window();
        }
        any
    }

    // --- Session -------------------------------------------------------------------------

    /// Schedules a session write when tabs or text changed since the last one.
    fn schedule_session(&mut self, now: Instant) {
        if self.state_dir.is_none() {
            return;
        }
        let mut print: Vec<(u64, u64, Option<PathBuf>)> = self
            .docs
            .iter()
            .map(|d| {
                (
                    d.id.0,
                    d.view.editor.buffer().version().get(),
                    d.path.clone(),
                )
            })
            .collect();
        print.push((u64::MAX, self.active as u64, None));
        if print != self.session_print && self.session_due.is_none() {
            self.session_due = Some(now + SESSION_DELAY);
        }
    }

    /// Writes the session, and the text of documents with unsaved changes, to the state
    /// directory: on a worker, or right away when `now` (quitting).
    pub fn save_session(&mut self, now: bool) {
        let Some(state) = self.state_dir.clone() else {
            return;
        };
        if !now && self.session_writing.load(Ordering::Acquire) {
            // One write at a time; try again shortly.
            self.session_due = Some(Instant::now() + SESSION_DELAY);
            return;
        }
        self.session_due = None;
        let mut writes: Vec<(String, fude_buffer::Rope)> = Vec::new();
        let mut tabs = Vec::new();
        let mut active = 0;
        let mut deferred: Option<Instant> = None;
        let Fude {
            docs,
            backed_up,
            backup_at,
            ..
        } = self;
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
                let due = match backup_at.get(&doc.id) {
                    Some(&at) if big && !now => at + BIG_BACKUP_EVERY,
                    _ => Instant::now(),
                };
                let stale = backed_up.get(&doc.id) != Some(&version);
                if stale && due > Instant::now() {
                    // Keep the older backup for now.
                    deferred = Some(deferred.map_or(due, |d| d.min(due)));
                } else if stale {
                    writes.push((name.clone(), doc.view.editor.text().clone()));
                    backed_up.insert(doc.id, version);
                    backup_at.insert(doc.id, Instant::now());
                }
                Some(name)
            } else {
                doc.backup = None;
                backed_up.remove(&doc.id);
                backup_at.remove(&doc.id);
                None
            };
            if doc.path.is_none() && backup.is_none() {
                continue;
            }
            if i <= self.active {
                active = tabs.len();
            }
            let format = match pending.map(|l| &l.kind) {
                Some(LoadKind::Backup { format, .. }) => *format,
                _ => *doc.view.editor.format(),
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
            });
        }
        let session = Session { active, tabs };
        self.session_print = self
            .docs
            .iter()
            .map(|d| {
                (
                    d.id.0,
                    d.view.editor.buffer().version().get(),
                    d.path.clone(),
                )
            })
            .chain([(u64::MAX, self.active as u64, None)])
            .collect();
        // A backup put off comes due by itself, with nothing else changing.
        if let Some(due) = deferred {
            self.session_due = Some(self.session_due.map_or(due, |d| d.min(due)));
        }
        let job = move || {
            for (name, text) in &writes {
                if let Err(e) = session::write_backup(&state, name, text) {
                    eprintln!("fude: couldn't back up unsaved changes: {e}");
                }
            }
            if let Err(e) = session.write(&state) {
                eprintln!("fude: couldn't save the session: {e}");
            }
            let keep: Vec<&str> = session
                .tabs
                .iter()
                .filter_map(|t| t.backup.as_deref())
                .collect();
            session::clean_backups(&state, &keep);
        };
        if now {
            job();
        } else {
            let writing = self.session_writing.clone();
            writing.store(true, Ordering::Release);
            std::thread::spawn(move || {
                job();
                writing.store(false, Ordering::Release);
            });
        }
    }

    /// Reopens the tabs of the last session, with their unsaved changes.
    pub fn restore_session(&mut self) {
        let Some(state) = self.state_dir.clone() else {
            return;
        };
        let Some(session) = Session::read(&state) else {
            return;
        };
        let mut restored = Vec::new();
        for tab in &session.tabs {
            let restore = Some((tab.cursor, tab.scroll));
            let id = match (&tab.backup, &tab.path) {
                (Some(name), path) => {
                    let source = session::backup_dir(&state).join(name);
                    if !source.is_file() {
                        continue;
                    }
                    let kind = LoadKind::Backup {
                        format: tab.format(),
                        path: path.clone(),
                    };
                    let id = self.open_loading(kind, &source, restore);
                    if let Some(i) = self.index_of(id) {
                        self.docs[i].backup = Some(name.clone());
                    }
                    id
                }
                (None, Some(path)) if path.is_file() => {
                    self.open_loading(LoadKind::Open, path, restore)
                }
                _ => continue,
            };
            restored.push(id);
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
}

/// What the editor shows while files are dragged over the window.
fn draw_drop_target(p: &mut Painter, area: Rect) {
    let t = p.theme;
    p.rect(area, t.scrim);
    let style = p.style(15.0);
    let label = "Drop to open";
    let pad = p.px(28.0);
    let w = p.measure(label, &style).ceil() as i32 + 2 * pad;
    let h = p.px(64.0);
    let x = area.x + (area.w - w) / 2;
    let y = area.y + (area.h - h) / 2;
    let border = p.px(2.0).max(1);
    p.rect(
        Rect::new(x - border, y - border, w + 2 * border, h + 2 * border),
        t.accent,
    );
    p.rect(Rect::new(x, y, w, h), t.dialog_bg);
    let baseline = p.center_baseline(y, h, &style);
    p.text(label, &style, (x + pad) as f32, baseline, t.text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fude_editor::Selection;
    use fude_platform::{Ime, Modifiers};

    fn draw(app: &mut Fude) {
        let mut pixels = vec![0; 800 * 600];
        app.draw(&mut Canvas::new(&mut pixels, 800, 600), 1.0, Instant::now());
    }

    fn enabled(app: &Fude, action: Action) -> bool {
        app.editor_context_menu_items()
            .iter()
            .find(|item| item.action == Some(action))
            .unwrap()
            .enabled
    }

    #[test]
    fn context_commands_follow_history_and_read_only_state() {
        let mut app = Fude::new(Config::default());
        for action in [
            Action::Undo,
            Action::Redo,
            Action::Cut,
            Action::Copy,
            Action::SelectAll,
        ] {
            assert!(!enabled(&app, action));
        }
        assert!(enabled(&app, Action::Paste));

        app.doc_mut().view.editor.insert("alpha", Instant::now());
        for action in [
            Action::Undo,
            Action::Cut,
            Action::Copy,
            Action::Paste,
            Action::SelectAll,
        ] {
            assert!(enabled(&app, action));
        }
        assert!(!enabled(&app, Action::Redo));
        app.doc_mut().view.editor.undo();
        assert!(enabled(&app, Action::Redo));
        assert!(!enabled(&app, Action::Undo));
        app.doc_mut().view.editor.redo();
        app.doc_mut().view.editor.set_read_only(true);
        for action in [Action::Undo, Action::Redo, Action::Cut, Action::Paste] {
            assert!(!enabled(&app, action));
        }
        assert!(enabled(&app, Action::Copy));
        assert!(enabled(&app, Action::SelectAll));
    }

    #[test]
    fn context_click_returns_focus_from_find_without_losing_selection() {
        let mut app = Fude::new(Config::default());
        let now = Instant::now();
        let view = &mut app.doc_mut().view;
        view.editor.insert("alpha beta", now);
        view.editor
            .set_selections(Selections::single(Selection::new(5, 0)));
        app.find_state.open = true;
        app.find.open(false, None, now);
        app.focus = Focus::Find;
        draw(&mut app);
        let caret = app.doc().view.caret_rect().unwrap();
        let selections = app.doc().view.editor.selections().clone();
        assert!(app.open_editor_context_menu(caret.x as f32 + 5.0, caret.y as f32 + 2.0, now));
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(*app.doc().view.editor.selections(), selections);
        assert!(app.context_menu.is_open());
        draw(&mut app);

        let key = |key| KeyInput {
            key,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
            text: None,
            base: None,
        };
        app.context_menu.key(&key(Key::End));
        assert_eq!(
            app.context_menu.key(&key(Key::Enter)),
            Some(Action::SelectAll)
        );
        assert!(!app.context_menu.is_open());
    }

    #[test]
    fn opening_menu_clears_composition_and_document_changes_dismiss_it() {
        let mut app = Fude::new(Config::default());
        let now = Instant::now();
        app.doc_mut().view.ime(Ime::Preedit("か".into(), None), now);
        draw(&mut app);
        assert!(app.open_editor_context_menu(100.0, 60.0, now));
        assert!(!app.doc_mut().view.ime(Ime::Disabled, now));
        assert!(app.doc().view.editor.text().is_empty());
        app.new_doc();
        assert!(!app.context_menu.is_open());

        draw(&mut app);
        assert!(app.open_editor_context_menu(100.0, 60.0, now));
        app.activate(0);
        assert!(!app.context_menu.is_open());
        app.show_editor_context_menu(100.0, 60.0, now);
        app.ask(Question::Info, "Test", "Message", &["OK"], 0, 0);
        assert!(!app.context_menu.is_open());
    }
}

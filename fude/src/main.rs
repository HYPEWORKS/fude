//! fude 筆: Fast, Uncomplicated Document Editor.

// Release builds on Windows are GUI apps (no console window). Output still reaches
// inherited pipes, which is what `cargo xtask bench-startup` uses.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod args;
mod bench_file;
mod config;
mod doc;
mod files;
mod find;
mod hot_exit;
mod instance;
mod loading;
mod session;
mod tools;
mod welcome;

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use app::Fude;
use args::{Args, Mode, USAGE};
use doc::Doc;
use fude_buffer::{Format, Rope};
use fude_editor::Motion;
use fude_platform::{App, Canvas, Ctx, Event, StartupMark, WindowOptions};

fn main() -> ExitCode {
    let started = Instant::now();
    let args = match args::parse(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("fude: {msg}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match &args.mode {
        Mode::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Mode::Version => {
            println!("fude {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Mode::Window => run_window(&args, started),
        Mode::Screenshot(path) => screenshot(&args, path),
        Mode::BenchRender => bench_render(&args),
        Mode::BenchTyping => bench_typing(&args),
        Mode::BenchFile(path) => bench_file::run(&args, path, args.check),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("fude: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// The app with the files given open (a path that doesn't exist yet opens as a new
/// document), or an empty document. `demo` shows the welcome text when no file is given
/// and keeps no session; `session` restores the last one and keeps it up to date.
fn initial_app(args: &Args, demo: bool, session: bool) -> Result<Fude, String> {
    let (config, problems) = config::Config::load();
    let mut app = Fude::new(config);
    if let Some(problem) = problems.first() {
        app.say(problem.clone(), true);
    }
    for path in &args.files {
        if path.is_dir() {
            return Err(format!("{} is a directory", path.display()));
        }
    }
    // The single instance keeps a session. The modes without a window, benchmarks, and
    // windows of their own (--new-instance, --wait) don't touch it: two processes writing
    // one session would delete each other's backups.
    if session && !demo {
        app.state_dir = config::state_dir();
        app.restore_session();
    }
    for path in &args.files {
        if path.is_dir() {
            return Err(format!("{} is a directory", path.display()));
        }
        app.open(path);
    }
    if args.files.is_empty() && demo {
        let id = app.new_id();
        app.add_doc(Doc::new(
            id,
            Rope::from(welcome::text().as_str()),
            Format::default(),
            None,
        ));
    }
    Ok(app)
}

struct Window {
    app: Fude,
    bench: Option<StartupBench>,
}

/// Startup milestones, printed as `fude-bench ...` lines for `cargo xtask bench-startup`.
struct StartupBench {
    started: Instant,
    marks: Vec<(&'static str, Duration)>,
}

impl StartupBench {
    fn mark(&mut self, name: &'static str) {
        self.marks.push((name, self.started.elapsed()));
    }

    fn report(&self) {
        for (name, at) in &self.marks {
            println!("fude-bench mark {name} {:.3}", at.as_secs_f64() * 1000.0);
        }
        if let Some(m) = fude_platform::memory_usage() {
            println!("fude-bench rss-bytes {}", m.rss);
            println!("fude-bench peak-rss-bytes {}", m.peak_rss);
        }
    }
}

/// Starts this launch again in the background, detached from the terminal it was run
/// from, and returns true if that worked (then this process can exit). Only for launches
/// from a terminal (stdin is one), on Unix; elsewhere the window simply opens.
fn detach(args: &Args, state: Option<&Path>) -> bool {
    #[cfg(unix)]
    {
        use std::io::IsTerminal;
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};

        if !std::io::stdin().is_terminal() {
            return false;
        }
        for path in &args.files {
            if path.is_dir() {
                // Report it here, where the user can see it.
                return false;
            }
        }
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        // Errors from the detached process go to a log, since its terminal is gone.
        let log = state.and_then(|dir| {
            std::fs::create_dir_all(dir).ok()?;
            std::fs::File::create(dir.join("fude.log")).ok()
        });
        let stderr = log.map_or_else(Stdio::null, Stdio::from);
        let args_os: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        Command::new(exe)
            .arg("--detached")
            .args(args_os)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            // Its own process group: Ctrl+C and the terminal closing don't reach it.
            .process_group(0)
            .spawn()
            .is_ok()
    }
    #[cfg(not(unix))]
    {
        let _ = (args, state);
        false
    }
}

fn run_window(args: &Args, started: Instant) -> Result<(), String> {
    let mut bench = args.bench_startup.then(|| StartupBench {
        started,
        marks: vec![],
    });
    // Hand the files to a running fude, if there is one.
    let single = !args.new_instance && !args.wait && !args.bench_startup;
    let state = config::state_dir();
    if single
        && let Some(state) = &state
        && instance::hand_off(state, &args.files)
    {
        return Ok(());
    }
    // From a terminal, run the window in the background and give the terminal back.
    if !args.wait && !args.detached && !args.bench_startup && detach(args, state.as_deref()) {
        return Ok(());
    }
    let mut app = initial_app(args, false, single)?;
    if single && let Some(state) = &state {
        app.instance = instance::serve(state);
    }
    if let Some(b) = &mut bench {
        b.mark("view-ready");
    }
    let options = WindowOptions {
        title: app.window_title(),
        width: 1100.0,
        height: 720.0,
    };
    fude_platform::run(options, Window { app, bench }).map_err(|e| e.to_string())
}

impl App for Window {
    fn startup_mark(&mut self, mark: StartupMark) {
        if let Some(b) = &mut self.bench {
            b.mark(match mark {
                StartupMark::EventLoopCreated => "event-loop",
                StartupMark::WindowCreated => "window",
                StartupMark::SurfaceCreated => "surface",
            });
        }
    }

    fn event(&mut self, ctx: &mut Ctx<'_>, event: Event) {
        // A bug in a command shouldn't take unsaved text down with it: report it and keep
        // going, so the user can still save (ARCHITECTURE.md §11).
        let app = &mut self.app;
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.event(ctx, event)));
        if let Err(payload) = result {
            self.app.recover(ctx, &panic_message(payload.as_ref()));
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, scale: f32) {
        // As for events: a bug in drawing costs a frame, not the unsaved text.
        let app = &mut self.app;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            app.draw(canvas, scale, Instant::now());
        }));
        if let Err(payload) = result {
            self.app.recover_draw(&panic_message(payload.as_ref()));
        }
        if let Some(b) = &mut self.bench {
            b.mark("first-draw");
        }
    }

    fn presented(&mut self, ctx: &mut Ctx<'_>) {
        self.app.presented(ctx);
        if let Some(b) = &mut self.bench {
            b.mark("first-present");
            b.report();
            ctx.exit();
        }
    }
}

/// What a panic said.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown error".to_owned())
}

fn screenshot(args: &Args, path: &Path) -> Result<(), String> {
    let mut app = initial_app(args, true, false)?;
    // Without a window, wait for files to load: the frames are what's measured or shown.
    app.finish_loading();
    let (w, h) = args.size;
    let mut pixels = vec![0u32; w as usize * h as usize];
    app.draw(
        &mut Canvas::new(&mut pixels, w as usize, h as usize),
        args.scale,
        Instant::now(),
    );
    // Draw again with syntax colors.
    app.highlight_now();
    app.draw(
        &mut Canvas::new(&mut pixels, w as usize, h as usize),
        args.scale,
        Instant::now(),
    );
    let png = fude_render::png::encode_rgb(w, h, &pixels);
    std::fs::write(path, png).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("wrote {} ({w}x{h} @{}x)", path.display(), args.scale);
    Ok(())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Sorted durations: min, median, p95, max.
fn summary(times: &mut [Duration]) -> String {
    times.sort();
    let n = times.len();
    format!(
        "min {:.2}  median {:.2}  p95 {:.2}  max {:.2} ms",
        ms(times[0]),
        ms(times[n / 2]),
        ms(times[n * 95 / 100]),
        ms(times[n - 1])
    )
}

/// Full-frame redraws without a window: what the CPU renderer costs per frame, worst case
/// (no damage tracking, every pixel drawn).
fn bench_render(args: &Args) -> Result<(), String> {
    let mut app = initial_app(args, true, false)?;
    // Without a window, wait for files to load: the frames are what's measured or shown.
    app.finish_loading();
    let (w, h) = args.size;
    let mut pixels = vec![0u32; w as usize * h as usize];
    let mut draw = |app: &mut Fude| {
        app.draw(
            &mut Canvas::new(&mut pixels, w as usize, h as usize),
            args.scale,
            Instant::now(),
        );
        app.last_frame
    };
    let cold = draw(&mut app);
    const FRAMES: usize = 120;
    let mut times = Vec::with_capacity(FRAMES);
    for _ in 0..FRAMES {
        let p = &mut app.painter;
        app.docs[app.active].view.scroll(
            p,
            0.0,
            1.0,
            fude_platform::ScrollUnit::Lines,
            Default::default(),
        );
        times.push(draw(&mut app));
    }
    println!(
        "fude render benchmark: {w}x{h} @{}x, {FRAMES} full-frame redraws",
        args.scale
    );
    println!("  first frame (cold glyph cache): {:>7.2} ms", ms(cold));
    println!("  warm frames: {}", summary(&mut times));
    Ok(())
}

/// Keystroke latency without a window: each step is an edit (or move) followed by a full
/// frame, so it measures fude's whole share of "key pressed → frame ready".
fn bench_typing(args: &Args) -> Result<(), String> {
    let mut app = initial_app(args, true, false)?;
    // Without a window, wait for files to load: the frames are what's measured or shown.
    app.finish_loading();
    let (w, h) = args.size;
    let mut pixels = vec![0u32; w as usize * h as usize];
    let mut draw = |app: &mut Fude| {
        app.draw(
            &mut Canvas::new(&mut pixels, w as usize, h as usize),
            args.scale,
            Instant::now(),
        );
    };
    draw(&mut app);
    // With syntax colors, so frames draw colored runs (they go stale as typing goes on,
    // which costs the same to draw).
    app.highlight_now();
    // Start in the middle of the document.
    let middle = {
        let text = app.doc_mut().view.editor.text();
        text.line_to_byte(text.line_count() / 2)
    };
    let view = &mut app.doc_mut().view;
    view.editor.set_selections(fude_editor::Selections::single(
        fude_editor::Selection::cursor(middle),
    ));
    view.changed(Instant::now());
    draw(&mut app);

    const KEYS: usize = 300;
    let mut typing = Vec::with_capacity(KEYS);
    let mut moving = Vec::with_capacity(KEYS);
    let mut deleting = Vec::with_capacity(KEYS);
    let mut layout = fude_editor::MonoLayout { tab_width: 4 };
    for i in 0..KEYS {
        let start = Instant::now();
        let now = Instant::now();
        app.doc_mut()
            .view
            .editor
            .insert(if i % 7 == 6 { " " } else { "x" }, now);
        app.doc_mut().view.changed(now);
        draw(&mut app);
        typing.push(start.elapsed());

        let start = Instant::now();
        app.doc_mut()
            .view
            .editor
            .move_cursors(Motion::Down, false, &mut layout);
        app.doc_mut().view.changed(now);
        draw(&mut app);
        moving.push(start.elapsed());
    }
    for _ in 0..KEYS {
        let start = Instant::now();
        let now = Instant::now();
        app.doc_mut().view.editor.backspace(now);
        app.doc_mut().view.changed(now);
        draw(&mut app);
        deleting.push(start.elapsed());
    }
    let size = app.doc_mut().view.editor.text().len();
    println!(
        "fude typing benchmark: {w}x{h} @{}x, {:.1} MB document, edit + full frame per key",
        args.scale,
        size as f64 / 1e6
    );
    println!("  type a character: {}", summary(&mut typing));
    println!("  move down a line: {}", summary(&mut moving));
    println!("  backspace:        {}", summary(&mut deleting));
    Ok(())
}

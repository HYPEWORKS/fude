//! fude 筆: Fast, Uncomplicated Document Editor.

// Release builds on Windows are GUI apps (no console window). Output still reaches
// inherited pipes, which is what `cargo xtask bench-startup` uses.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod args;
mod welcome;

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use app::{Doc, Fude};
use args::{Args, Mode, USAGE};
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
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("fude: {msg}");
            ExitCode::FAILURE
        }
    }
}

/// The document to start with: the file given (new if it doesn't exist yet), or an
/// empty one. `demo` shows the welcome text instead of an empty document.
fn initial_doc(args: &Args, demo: bool) -> Result<Doc, String> {
    match &args.file {
        Some(path) => match fude_buffer::load(path) {
            Ok(loaded) => Ok(Doc::loaded(loaded, path.clone())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(Doc::new(Rope::new(), Format::default(), Some(path.clone())))
            }
            Err(e) => Err(format!("{}: {e}", path.display())),
        },
        None if demo => Ok(Doc::new(
            Rope::from(welcome::text().as_str()),
            Format::default(),
            None,
        )),
        None => Ok(Doc::untitled()),
    }
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

fn run_window(args: &Args, started: Instant) -> Result<(), String> {
    let mut bench = args.bench_startup.then(|| StartupBench {
        started,
        marks: vec![],
    });
    let app = Fude::new(initial_doc(args, false)?);
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
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown error".to_owned());
            self.app.recover(ctx, &message);
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, scale: f32) {
        self.app.draw(canvas, scale, Instant::now());
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

fn screenshot(args: &Args, path: &Path) -> Result<(), String> {
    let mut app = Fude::new(initial_doc(args, true)?);
    let (w, h) = args.size;
    let mut pixels = vec![0u32; w as usize * h as usize];
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
    let mut app = Fude::new(initial_doc(args, true)?);
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
        app.doc.view.scroll(
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
    let mut app = Fude::new(initial_doc(args, true)?);
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
    // Start in the middle of the document.
    let middle = {
        let text = app.doc.view.editor.text();
        text.line_to_byte(text.line_count() / 2)
    };
    let view = &mut app.doc.view;
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
        app.doc
            .view
            .editor
            .insert(if i % 7 == 6 { " " } else { "x" }, now);
        app.doc.view.changed(now);
        draw(&mut app);
        typing.push(start.elapsed());

        let start = Instant::now();
        app.doc
            .view
            .editor
            .move_cursors(Motion::Down, false, &mut layout);
        app.doc.view.changed(now);
        draw(&mut app);
        moving.push(start.elapsed());
    }
    for _ in 0..KEYS {
        let start = Instant::now();
        let now = Instant::now();
        app.doc.view.editor.backspace(now);
        app.doc.view.changed(now);
        draw(&mut app);
        deleting.push(start.elapsed());
    }
    let size = app.doc.view.editor.text().len();
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

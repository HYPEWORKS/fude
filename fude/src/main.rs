//! fude 筆: Fast, Uncomplicated Document Editor.

// Release builds on Windows are GUI apps (no console window). Output still reaches
// inherited pipes, which is what `cargo xtask bench-startup` uses.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod args;
mod theme;
mod view;

use std::path::Path;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use args::{Args, Mode, USAGE};
use fude_platform::{
    App, Canvas, Ctx, Event, Key, KeyInput, ScrollUnit, StartupMark, WindowOptions,
};
use view::View;

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
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("fude: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn load_view(args: &Args) -> Result<View, String> {
    match &args.file {
        None => Ok(View::new("Welcome".into(), &view::welcome_text())),
        Some(path) => {
            // Phase 0 stand-in for the real loader (fude-buffer, phase 1).
            let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let name = path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            Ok(View::new(name, &String::from_utf8_lossy(&bytes)))
        }
    }
}

struct Fude {
    view: View,
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
    let view = load_view(args)?;
    if let Some(b) = &mut bench {
        b.mark("view-ready");
    }
    let options = WindowOptions {
        title: format!("{} · fude", view.title()),
        width: 1100.0,
        height: 720.0,
    };
    fude_platform::run(options, Fude { view, bench }).map_err(|e| e.to_string())
}

impl App for Fude {
    fn startup_mark(&mut self, mark: StartupMark) {
        if let Some(b) = &mut self.bench {
            b.mark(match mark {
                StartupMark::EventLoopCreated => "event-loop",
                StartupMark::WindowCreated => "window",
                StartupMark::SurfaceCreated => "surface",
            });
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, scale: f32) {
        self.view.draw(canvas, scale);
        if let Some(b) = &mut self.bench {
            b.mark("first-draw");
        }
    }

    fn presented(&mut self, ctx: &mut Ctx<'_>) {
        if let Some(b) = &mut self.bench {
            b.mark("first-present");
            b.report();
            ctx.exit();
        }
    }

    fn event(&mut self, ctx: &mut Ctx<'_>, event: Event) {
        let scale = ctx.scale_factor() as f32;
        match event {
            Event::CloseRequested => ctx.exit(),
            Event::Scroll { dy, unit, .. } => {
                let lines = match unit {
                    ScrollUnit::Lines => dy * 3.0,
                    ScrollUnit::Pixels => self.view.pixels_to_lines(dy, scale),
                };
                self.view.scroll_by(lines);
                ctx.request_redraw();
            }
            Event::Key(key) if key.pressed => {
                if self.key(&key, ctx) {
                    ctx.request_redraw();
                }
            }
            Event::ScaleChanged(_) | Event::Resized { .. } => ctx.request_redraw(),
            _ => {}
        }
    }
}

impl Fude {
    /// Handles a key press; returns whether the view changed.
    fn key(&mut self, key: &KeyInput, ctx: &Ctx<'_>) -> bool {
        let primary = key.modifiers.primary();
        let page = (self.view.page_lines() - 1.0).max(1.0).floor();
        match &key.key {
            Key::Character(c) if primary => match c.as_str() {
                "=" | "+" => self.view.zoom(1.0),
                "-" => self.view.zoom(-1.0),
                "0" => self.view.reset_zoom(),
                "q" | "w" => {
                    ctx.exit();
                    return false;
                }
                _ => return false,
            },
            Key::Up => self.view.scroll_by(-1.0),
            Key::Down => self.view.scroll_by(1.0),
            Key::PageUp => self.view.scroll_by(-page),
            Key::PageDown => self.view.scroll_by(page),
            Key::Home => self.view.scroll_to(0.0),
            Key::End => self.view.scroll_to(self.view.line_count() as f32),
            _ => return false,
        }
        true
    }
}

fn screenshot(args: &Args, path: &Path) -> Result<(), String> {
    let mut view = load_view(args)?;
    let (w, h) = args.size;
    let mut pixels = vec![0u32; w as usize * h as usize];
    view.draw(
        &mut Canvas::new(&mut pixels, w as usize, h as usize),
        args.scale,
    );
    let png = fude_render::png::encode_rgb(w, h, &pixels);
    std::fs::write(path, png).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("wrote {} ({w}x{h} @{}x)", path.display(), args.scale);
    Ok(())
}

/// Full-frame redraws without a window: what the CPU renderer costs per frame, worst case
/// (no damage tracking, every pixel drawn).
fn bench_render(args: &Args) -> Result<(), String> {
    let mut view = load_view(args)?;
    let (w, h) = args.size;
    let mut pixels = vec![0u32; w as usize * h as usize];

    let cold = Instant::now();
    view.draw(
        &mut Canvas::new(&mut pixels, w as usize, h as usize),
        args.scale,
    );
    let cold = cold.elapsed();

    const FRAMES: usize = 120;
    let mut times = Vec::with_capacity(FRAMES);
    for _ in 0..FRAMES {
        view.scroll_by(1.0);
        view.draw(
            &mut Canvas::new(&mut pixels, w as usize, h as usize),
            args.scale,
        );
        times.push(view.last_frame());
    }
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mean = times.iter().map(|d| ms(*d)).sum::<f64>() / FRAMES as f64;
    let (glyphs, glyph_bytes) = view.cache_stats();
    println!(
        "fude render benchmark: {w}x{h} @{}x, {FRAMES} full-frame redraws",
        args.scale
    );
    println!("  first frame (cold glyph cache): {:>7.2} ms", ms(cold));
    println!(
        "  warm frames: min {:.2}  median {:.2}  mean {:.2}  p95 {:.2}  max {:.2} ms",
        ms(times[0]),
        ms(times[FRAMES / 2]),
        mean,
        ms(times[FRAMES * 95 / 100]),
        ms(times[FRAMES - 1])
    );
    println!(
        "  glyph cache: {glyphs} glyphs, {:.1} KB",
        glyph_bytes as f64 / 1024.0
    );
    Ok(())
}

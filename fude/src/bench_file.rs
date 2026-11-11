//! `fude --bench-file <PATH>`: opens one file without a window and measures what the
//! large-file budgets (ARCHITECTURE.md) are about: the first screen, loading, memory,
//! and the cost of scrolling, jumping, moving and typing, each step including a full
//! frame. `cargo xtask bench-large` runs it over the generated corpus.

use std::path::Path;
use std::time::{Duration, Instant};

use fude_editor::{Selection, Selections};
use fude_platform::{Key, KeyInput, Modifiers, ScrollUnit};
use fude_render::Canvas;

use crate::app::Fude;
use crate::args::Args;
use crate::config::Config;

const FIRST_SCREEN_BUDGET: Duration = Duration::from_millis(100);
const LOADED_BUDGET: Duration = Duration::from_secs(1);
/// Median keystroke (or scroll step) to finished frame.
const STEP_BUDGET: Duration = Duration::from_millis(4);
/// RAM for an open file: this much per byte of text, plus a little for everything else.
const RAM_PER_BYTE: f64 = 1.1;
const RAM_SLACK: u64 = 16 << 20;

struct Bench {
    app: Fude,
    pixels: Vec<u32>,
    size: (u32, u32),
    scale: f32,
    over: Vec<String>,
}

impl Bench {
    fn frame(&mut self) {
        let (w, h) = self.size;
        self.app.draw(
            &mut Canvas::new(&mut self.pixels, w as usize, h as usize),
            self.scale,
            Instant::now(),
        );
    }

    /// Runs `step` (which changes something) then draws a frame, `n` times.
    fn steps(&mut self, n: usize, mut step: impl FnMut(&mut Fude, usize)) -> Vec<Duration> {
        (0..n)
            .map(|i| {
                let start = Instant::now();
                step(&mut self.app, i);
                self.frame();
                start.elapsed()
            })
            .collect()
    }

    fn report(&mut self, what: &str, mut times: Vec<Duration>, budget: Option<Duration>) {
        times.sort();
        let n = times.len();
        let median = times[n / 2];
        let mut line = format!(
            "  {what:<24} median {:>6.2}  p95 {:>6.2}  max {:>6.2} ms",
            ms(median),
            ms(times[n * 95 / 100]),
            ms(times[n - 1])
        );
        self.judge(&mut line, what, median, budget);
        println!("{line}");
    }

    fn report_once(&mut self, what: &str, time: Duration, budget: Option<Duration>) {
        let mut line = format!("  {what:<24} {:>8.2} ms", ms(time));
        self.judge(&mut line, what, time, budget);
        println!("{line}");
    }

    fn judge(&mut self, line: &mut String, what: &str, time: Duration, budget: Option<Duration>) {
        if let Some(budget) = budget {
            let ok = time <= budget;
            line.push_str(&format!(
                "   {} budget {} ms",
                if ok { "✓" } else { "✗" },
                ms(budget)
            ));
            if !ok {
                self.over.push(what.to_owned());
            }
        }
    }

    fn key(&mut self, key: Key, modifiers: Modifiers) {
        let now = Instant::now();
        let input = KeyInput {
            key,
            pressed: true,
            repeat: false,
            modifiers,
            text: None,
            base: None,
        };
        let Fude {
            docs,
            active,
            painter,
            ..
        } = &mut self.app;
        docs[*active].view.key(painter, &input, now);
    }

    fn place_cursor(&mut self, at: usize) {
        let view = &mut self.app.doc_mut().view;
        view.editor
            .set_selections(Selections::single(Selection::cursor(at)));
        view.changed(Instant::now());
        self.frame();
    }

    fn typing(&mut self, what: &str, at: usize) {
        self.place_cursor(at);
        let times = self.steps(100, |app, i| {
            let now = Instant::now();
            let view = &mut app.doc_mut().view;
            view.editor.insert(if i % 7 == 6 { " " } else { "x" }, now);
            view.changed(now);
        });
        self.report(what, times, Some(STEP_BUDGET));
        // Put the text back, so later steps see the file as it was.
        let view = &mut self.app.doc_mut().view;
        while view.editor.undo() {}
        view.changed(Instant::now());
    }

    fn moving(&mut self, what: &str, key: Key, n: usize) {
        let mut times = Vec::with_capacity(n);
        for _ in 0..n {
            let start = Instant::now();
            self.key(key.clone(), Modifiers::default());
            self.frame();
            times.push(start.elapsed());
        }
        self.report(what, times, Some(STEP_BUDGET));
    }

    fn scrolling(&mut self, what: &str) {
        let times = self.steps(100, |app, i| {
            let Fude {
                docs,
                active,
                painter,
                ..
            } = app;
            // Down, then back up: like a trackpad flick and return.
            let dy = if i < 50 { 3.0 } else { -3.0 };
            docs[*active]
                .view
                .scroll(painter, 0.0, dy, ScrollUnit::Lines, Modifiers::default());
        });
        self.report(what, times, Some(STEP_BUDGET));
    }
}

pub fn run(args: &Args, path: &Path, check: bool) -> Result<(), String> {
    let config = Config {
        hot_exit: false,
        ..Config::default()
    };
    let mut bench = Bench {
        app: Fude::new(config),
        pixels: vec![0; args.size.0 as usize * args.size.1 as usize],
        size: args.size,
        scale: args.scale,
        over: Vec::new(),
    };
    bench.frame();
    let ram_before = fude_platform::memory_usage();

    let start = Instant::now();
    bench.app.open(path);
    bench.frame();
    let first_screen = start.elapsed();
    bench.app.finish_loading();
    bench.frame();
    let loaded = start.elapsed();
    let ram_after = fude_platform::memory_usage();

    let doc = bench.app.doc();
    if doc.path.as_deref() != std::fs::canonicalize(path).ok().as_deref() {
        return Err(format!("{} didn't open", path.display()));
    }
    let text = doc.view.editor.text().clone();
    let format = *doc.view.editor.format();
    println!(
        "fude file benchmark: {} ({:.1} MB, {}, {}, {} lines; {}x{} @{}x)",
        path.display(),
        text.len() as f64 / 1e6,
        format.encoding_label(),
        format.line_ending.name(),
        text.line_count(),
        args.size.0,
        args.size.1,
        args.scale
    );
    bench.report_once("first screen", first_screen, Some(FIRST_SCREEN_BUDGET));
    bench.report_once("fully loaded", loaded, Some(LOADED_BUDGET));
    if let (Some(a), Some(b)) = (ram_before, ram_after) {
        let used = b.rss.saturating_sub(a.rss);
        let budget = (text.len() as f64 * RAM_PER_BYTE) as u64 + RAM_SLACK;
        let ok = used <= budget;
        println!(
            "  {:<24} {:>8.1} MB   {:.2}× the text, peak {:.1} MB   {} budget {RAM_PER_BYTE}× + {} MB",
            "memory",
            used as f64 / 1e6,
            used as f64 / text.len().max(1) as f64,
            b.peak_rss.saturating_sub(a.rss) as f64 / 1e6,
            if ok { "✓" } else { "✗" },
            RAM_SLACK >> 20,
        );
        if !ok {
            bench.over.push("memory".into());
        }
    }

    bench.app.highlight_now();
    let len = text.len();
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::default()
    };
    let jump = |bench: &mut Bench, key: Key| {
        let start = Instant::now();
        bench.key(key, ctrl);
        bench.frame();
        start.elapsed()
    };
    let to_end = jump(&mut bench, Key::End);
    bench.report_once("jump to the end", to_end, Some(STEP_BUDGET));
    let to_start = jump(&mut bench, Key::Home);
    bench.report_once("jump to the start", to_start, Some(STEP_BUDGET));
    bench.scrolling("scroll");
    bench.moving("page down", Key::PageDown, 30);

    bench.typing("type at the start", 0);
    // The middle of the text: for a file of long lines, the middle of one of them.
    let middle = fude_editor::text::floor_grapheme(&text, len / 2);
    bench.typing("type in the middle", middle);
    bench.typing("type at the end", len);
    bench.place_cursor(middle);
    bench.moving("move right", Key::Right, 100);
    bench.moving("move down", Key::Down, 100);

    // The same with word wrap, where rows depend on layout.
    bench.app.doc_mut().view.wrap = true;
    bench.place_cursor(0);
    bench.scrolling("wrapped: scroll");
    bench.typing("wrapped: type in middle", middle);
    bench.place_cursor(middle);
    bench.moving("wrapped: move down", Key::Down, 100);
    let to_end = jump(&mut bench, Key::End);
    bench.report_once("wrapped: jump to end", to_end, Some(STEP_BUDGET));

    // Multiple cursors at their limit: every occurrence of a common word (capped), and
    // typing at all of them.
    bench.app.doc_mut().view.wrap = false;
    let word_at = text.len() / 3;
    bench.place_cursor(fude_editor::text::floor_grapheme(&text, word_at));
    let start = Instant::now();
    let n = bench.app.doc_mut().view.editor.select_all_occurrences();
    bench.frame();
    bench.report_once(&format!("select {n} occurrences"), start.elapsed(), None);
    if n > 1 {
        let times = bench.steps(10, |app, _| {
            let now = Instant::now();
            let view = &mut app.doc_mut().view;
            view.editor.insert("x", now);
            view.changed(now);
        });
        bench.report(&format!("type at {n} cursors"), times, None);
        let view = &mut bench.app.doc_mut().view;
        while view.editor.undo() {}
    }

    if check && !bench.over.is_empty() {
        return Err(format!("over budget: {}", bench.over.join(", ")));
    }
    Ok(())
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

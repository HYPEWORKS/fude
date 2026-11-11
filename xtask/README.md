# xtask

Development tasks: benchmarks, the large-file corpus, the UI test and the README's screenshots. Run with `cargo xtask <task>` (`cargo xtask` alone lists them). The tasks that run fude build the release binary first unless given `--no-build`.

Dependencies: `encoding_rs` (to write the corpus's Shift_JIS file); `x11rb` with XTest on Linux and the BSDs, for the UI test.

## Startup benchmark

```sh
cargo xtask bench-startup [--runs N] [--warmup N] [--budget-ms MS] [--check] [-- FUDE_ARGS...]
```

Launches the release build repeatedly with `--bench-startup` (which exits right after the first frame is presented and prints timings) and reports launch to first frame and RSS. The budget is 40 ms; `--check` fails over it. The current breakdown is in [fude-platform](../crates/fude-platform/README.md#startup).

On Linux it runs off-screen under Xvfb. `-noreset` matters: by default Xvfb resets whenever its last client exits, which would add about 30 ms to every launch after the first.

```sh
env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 1920x1080x24 -noreset" cargo xtask bench-startup
```

## Large-file corpus

```sh
cargo xtask corpus [--small]
```

Generates the corpus into `target/corpus` (about 1 GB; with `--small`, a tenth of that, into `target/corpus-small`). It's never committed. Every file is made from a fixed seed, so runs compare like with like, and files that exist are kept.

| File | Size | |
|---|---|---|
| `log.log` | 500 MB | A log: millions of short lines |
| `minified.json` | 100 MB | Minified JSON: one huge line |
| `long-lines.csv` | 100 MB | CSV rows of 50 KB each |
| `japanese.txt` | 50 MB | Japanese prose, UTF-8 |
| `shift_jis.txt` | 50 MB | Japanese prose in Shift_JIS, no BOM |
| `utf16le.txt` | 50 MB | Mixed-script text, UTF-16LE with BOM |
| `invalid-utf8.log` | 100 MB | A UTF-8 log with stray invalid bytes |
| `mixed-endings.txt` | 50 MB | LF, CRLF and CR line endings mixed |
| `code.rs` | 20 MB | Rust, above the whole-file highlighting limit (16 MB) |

## Large-file benchmark

```sh
cargo xtask bench-large [--small] [--check] [--no-build] [FILE...]
```

Opens each corpus file (or the files given) with `fude --bench-file`, without a window at 1920×1080, and measures the first screen, loading, memory, and keystrokes, scrolling and paging at the start, middle and end, with and without word wrap, against the [budgets](../ARCHITECTURE.md#performance-budgets). `--check` fails a missed budget; CI runs it on the small corpus and checks on Linux.

Latest results (keystrokes are an edit plus a full frame, medians):

| File | First screen | Loaded | RAM ÷ text | Type in the middle | Page Down | Wrapped: type |
|---|---|---|---|---|---|---|
| 500 MB log, 5.8 M lines | 23 ms | 0.20 s | 1.04 | 0.60 ms | 0.57 ms | 0.71 ms |
| 100 MB of JSON on one line | 22 ms | 0.06 s | 1.05 | 1.28 ms | 1.91 ms | 1.28 ms |
| 100 MB CSV, 50 KB lines | 22 ms | 0.04 s | 1.05 | 1.31 ms | 1.40 ms | 1.52 ms |
| 50 MB Shift_JIS (78 MB as UTF-8) | 57 ms | 0.56 s | 1.19 | 0.66 ms | 0.67 ms | 0.99 ms |
| 50 MB UTF-16LE | 51 ms | 0.06 s | 1.28 | 0.72 ms | 0.71 ms | 1.09 ms |
| 100 MB UTF-8 log with bad bytes | 22 ms | 0.11 s | 1.06 | 0.45 ms | 0.43 ms | 0.52 ms |
| 21 MB of Rust | 10 ms | 0.01 s | 1.09 | 0.36 ms | 0.32 ms | 0.41 ms |

The first screen includes a 20 ms wait that lets files of up to about 100 MB appear whole instead of as a preview; the Japanese files spend another 30 ms opening the CJK fallback font. Loading Shift_JIS is decoding-bound (and checks that every character re-encodes exactly). Before phase 4, a keystroke in the middle of the JSON line took 12 ms and every frame of the CSV 10 ms, and both lines were cut off at 16 KB.

## UI test

```sh
env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 1920x1080x24 -noreset" cargo xtask ui-test
```

Drives the real window through X11 under Xvfb (Linux and the BSDs; an X server without a window manager). It types, clicks and uses shortcuts with XTest (synthetic input the server can't tell from a real keyboard and mouse), saves with Ctrl+S after each step, and compares the file on disk with what the step should have produced. It uses its own settings and state directories, and runs in CI on every push.

It covers typing, mouse selection, clipboard, undo, the unsaved-changes prompt, find and replace, tabs, go to line, the command palette, word wrap, hot exit, single instance, recovery after the process is killed, editing the end of a 40 MB file's 2 MB last line once it has loaded, and the power features: Ctrl+D, moving, duplicating and commenting lines, a column of cursors, sorting from the command palette, an external tool on its own shortcut, and auto-indent (26 checks).

Pixel positions assume scale 1 and the default 14 px font. With `FUDE_UI_SHOTS=<dir>`, it also saves screenshots of a selection and the unsaved-changes prompt there (needs ImageMagick's `import`).

## Screenshots

```sh
env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 2600x1700x24 -noreset" cargo xtask screenshots [--no-build] [--out DIR] [NAME...]
```

Takes the [README](../README.md)'s screenshots from the real window, into `docs/screenshots` (or `DIR`): all four, or only the scenes named (`editor-dark`, `command-palette`, `find-replace-regex`, `multiple-cursors`). Each scene opens the files in [`docs/screenshots/demo`](../docs/screenshots/demo) in a fresh fude with its own settings and session, at scale 2, drives it with the UI test's XTest driver, and saves the window with ImageMagick's `import`. The screen has to be larger than the 2200×1440 window. Each scene waits out the 10 s of caret blinking so the carets show, so the run takes about a minute. Run it again when the UI changes.

## Other benchmarks

These don't need xtask:

```sh
fude --bench-render --size 3840x2160 --scale 2        # CPU renderer cost per full frame
fude --bench-typing some/file.txt                      # edit + full frame per keystroke
fude --screenshot out.png --size 2200x1440 --scale 2   # render one frame to PNG
cargo run --release -p fude-buffer --example bench     # the buffer on a generated 500 MB log
```

Results: [rendering](../crates/fude-render/README.md#performance), [the buffer](../crates/fude-buffer/README.md#performance).

## Modules

| File | |
|---|---|
| `main.rs` | Task dispatch, `bench-startup`, `bench-large` |
| `corpus.rs` | The corpus generator |
| `ui_test.rs` | The UI test (Linux and the BSDs) |
| `screenshots.rs` | The README's screenshots (Linux and the BSDs) |

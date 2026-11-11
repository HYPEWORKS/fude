# fude architecture

**fude** (筆, "brush"): **F**ast, **U**ncomplicated **D**ocument **E**ditor.

A cross-platform text editor that sits between Notepad and Notepad++. It should start instantly, respond instantly at any file size, use little memory, and look and feel polished on every platform.

> Status: phase 5 (power features) done, plus a context menu, the application menus and accessibility (screen readers, through AccessKit). Next: the rest of phase 6, release. Last updated 2026-11-11.
>
> This is the high-level design: goals, how the system fits together, and the decisions that cut across crates. Each crate's README has its own design, measurements and decisions ([Crates](#crates)). Parts that aren't built yet are marked *(planned)*.

---

## Goals

- **Platforms:** Windows, macOS, Linux. BSD is a nice-to-have; the stack below should support it with little extra effort.
- **Fast:** near-instant launch, no perceptible input latency, smooth scrolling.
- **Large files:** files of hundreds of MB open quickly and stay responsive. Multi-GB files are a later goal ([paged mode](crates/fude-buffer/README.md#later-paged-mode-for-multi-gb-files)).
- **Light:** low RAM use and zero CPU when idle.
- **Good UX:** a consistent, polished UI on every platform.
- **Uncomplicated:** a focused feature set, not an IDE.

## Principles

1. **The UI thread never does work proportional to file size.** A frame costs O(visible text) plus O(log n) tree lookups. Anything proportional to file size (loading, line counting, find-all, full highlighting, saving, encoding conversion) runs on a worker thread and streams results back.
2. **Approximate now, exact later.** Show the first screen immediately. Line numbers, the scrollbar, syntax colors and match counts fill in or correct themselves as background work finishes.
3. **Event-driven, zero idle CPU.** No redraw loop. Repaint only what changed, only when something changed.
4. **Lazy startup.** Nothing loads before the first frame unless the first frame needs it.
5. **The file's bytes are the truth.** Opening and saving an unedited file gives back identical bytes, including invalid UTF-8 and mixed line endings.
6. **Every dependency has to earn its place,** measured in startup time, RAM and binary size.
7. **Budgets are tests.** CI fails when a performance budget regresses ([Testing](#testing)).

## Performance budgets

| Metric | Target | Measured |
|---|---|---|
| Launch → first frame, warm cache | < 40 ms | 10.3 ms median (Linux, X11); a launch that hands its files to a running fude exits in under 10 ms |
| Launch → first frame, cold | < 150 ms | |
| Open a 500 MB file → first screen | < 100 ms | 23 ms (its first 64 KB, while the rest loads in the background) |
| Open a 500 MB file → fully loaded and editable | < 1 s (SSD or warm cache) | 0.2 s |
| Keypress → frame submitted | < 4 ms of fude's own work | 0.5 ms at 1080p, 1.4 ms at 4K (edit plus a full redraw, with syntax colors); file size doesn't change it, and inside a 100 MB line it's 1.3 ms |
| Idle RAM, empty document, 1080p | < 30 MB | 14.6 MB (Linux, X11) before accessibility; the AccessKit adapter adds 2.3 MB (RSS at the first frame: 16.6 → 18.9 MB) |
| RAM for an open, unedited file | ≈ 1.1 × file size | 1.04 × the text (UTF-8); 1.2 × for Shift_JIS, UTF-16 and Japanese text, most of the difference being the CJK fallback font |
| Idle CPU | 0 % | 0 CPU ticks over 5 s once the caret stops blinking (10 s after input) |
| Release binary | < 12 MB | 11.6 MB (Linux x86-64) |

Measurements are from a fast desktop (32 threads, NVMe, warm page cache) and are a best case; laptops and Wayland/Windows/macOS numbers come next. The details and how to reproduce them are next to the code: [startup](crates/fude-platform/README.md#startup), [rendering](crates/fude-render/README.md#performance), [the buffer](crates/fude-buffer/README.md#performance), [large files](xtask/README.md#large-file-benchmark), [multiple cursors](crates/fude-editor/README.md#performance).

**Binary size is the tightest budget.** It was 10 MB until accessibility, which is a release requirement and costs 1.9 MB on Linux (nearly all AT-SPI's D-Bus stack: `zbus`, async I/O, serde), even built with `opt-level = "z"` (2.8 MB without); the binary went from 9.7 MB to 11.6 MB, and the budget to 12 MB. Syntax highlighting (syntect and its grammars) is 2.4 MB of it; font fallback, clipboard and file dialogs 1.1 MB; search 0.4 MB; encodings 0.3 MB; phases 4 and 5 0.2 MB each; the menu bar 20 KB (`muda` is only built on macOS). That leaves 0.4 MB: the next large feature has to pay for itself. Trimming the 75 bundled grammars is the obvious place, deferred for now.

## UX principles

- **Look consistent, behave native.** One visual design on every platform, but behavior follows each platform's conventions: Cmd vs Ctrl, Option+arrow word jumps on macOS, OS trackpad scrolling and momentum, native file dialogs, the macOS menu bar, double- and triple-click selection.
- **One design system.** All UI is built from shared tokens (spacing, type, color roles, corner radii) and one widget kit. No one-off styling.
- **Keyboard first, mouse friendly.** Every action is a command, and every command appears in the command palette with its shortcut, so features are discoverable without digging through menus.
- **Minimal chrome.** No toolbar. Tabs on top, status bar at the bottom. Status bar items (encoding, line endings, language, indentation) are to double as controls: click one to change it *(planned)*.
- **Only modal when necessary.** Find/replace and go-to-line are inline bars, not dialogs. Long operations show progress and can be cancelled.
- **Never lose work.** Hot exit, background backups of unsaved changes, atomic saves.
- **Themes.** Light and dark follow the OS by default. Text contrast meets WCAG AA (4.5:1) on every background it's drawn on, which a test checks. Everything is crisp on high-DPI displays.

## Crates

```
┌─────────────────────────────── fude (binary) ────────────────────────────────┐
│ documents, loading and saving, find, tools, settings, hot exit, one instance │
├──────────────────────────────────────────────────────────────────────────────┤
│ fude-ui    editor view, tabs, find bar, menus, palette, prompts, status bar  │
├──────────────┬──────────────┬────────────┬──────────────┬────────────────────┤
│ fude-editor  │ fude-syntax  │ fude-tools │ fude-text    │ fude-platform      │
│ selections,  │ highlighting │ built-in   │ fonts,       │ winit, input,      │
│ commands,    │ worker,      │ text tools │ fallback,    │ clipboard, dialogs,│
│ search       │ comments     │            │ shaping      │ macOS menu bar     │
├──────────────┴──────────────┤            ├──────────────┴────────────────────┤
│ fude-buffer                 │            │ fude-render                       │
│ rope, snapshots, undo,      │            │ draw list, CPU rasterizer         │
│ encodings, load and save    │            │                                   │
└─────────────────────────────┴────────────┴───────────────────────────────────┘
```

Dependencies point downward only. Only `fude-platform` knows about winit, and nothing outside `fude-syntax` sees syntect. `fude-buffer`, `fude-editor`, `fude-syntax` and `fude-tools` have no UI dependencies and are tested headless.

| Crate | Purpose |
|---|---|
| [`fude-buffer`](crates/fude-buffer/README.md) | Rope, snapshots, undo, encodings, line endings, loading and saving |
| [`fude-editor`](crates/fude-editor/README.md) | Selections, cursor movement, editing commands and search, without UI |
| [`fude-syntax`](crates/fude-syntax/README.md) | Syntax highlighting on a worker thread; comment markers |
| [`fude-tools`](crates/fude-tools/README.md) | Built-in text tools: sorting, case, encodings, JSON and XML formatting, hashes |
| [`fude-text`](crates/fude-text/README.md) | Fonts, fallback, shaping and the glyph cache |
| [`fude-render`](crates/fude-render/README.md) | Draw list and CPU rasterizer |
| [`fude-platform`](crates/fude-platform/README.md) | Windowing, input, clipboard, file dialogs, the macOS menu bar |
| [`fude-ui`](crates/fude-ui/README.md) | Editor view, tabs, find bar, menus, command palette, prompts, status bar, themes |
| [`fude`](fude/README.md) | The application: documents, files, find, tools, settings, hot exit, single instance |
| [`xtask`](xtask/README.md) | Development tasks (`cargo xtask`): benchmarks, the large-file corpus, the UI test |
| [`fuzz`](fuzz/README.md) | Fuzz targets for the buffer (a separate workspace) |

## Threads and snapshots

```
UI thread (winit event loop)                         Worker threads
 input → command → edit rope (new root) ─snapshot─▶ highlighter
 lay out visible lines only             ─snapshot─▶ search, count, replace all, tools
 build draw list → rasterize            ─snapshot─▶ save, session backup
 present                                            loader
   ▲                                                      │
   └──── results tagged with document version ◀───────────┘
         (results for an older version are dropped; a Waker wakes the UI)
```

- **The UI thread owns every document.** Work proportional to file size runs on a worker over a *snapshot*: an O(1) clone of the rope's root, immutable, so workers never take a lock ([fude-buffer](crates/fude-buffer/README.md#the-rope)).
- **Versions make results safe.** Every buffer state has a version number. Results come back tagged with the version they were computed from, and apply only if it's still current; otherwise they're dropped or recomputed.
- **Generations cancel.** Each job carries a generation number, so a newer request (typing a new search query) cancels the older one.
- **Plain threads and channels.** No async runtime. The event loop sleeps until input, a timer the app set, or a worker's wake ([fude-platform](crates/fude-platform/README.md#the-event-loop)); nothing polls.

## Rendering

**A custom-drawn UI on top of winit, rendered on the CPU.**

Most of an editor's UI is the text view, and no general-purpose toolkit has a text widget that handles large files or very long lines well. Since we have to write that view ourselves, the rest of the chrome is small enough to draw too. This is the approach Sublime Text takes.

| Rejected option | Why |
|---|---|
| Electron / Tauri | Startup and RAM an order of magnitude over budget |
| Qt / GTK | 100 ms+ startup, tens of MB of RAM; GTK looks foreign on Windows and macOS |
| egui / iced / Slint | GPU startup cost; their text widgets lay out the whole document |
| Native widgets per OS | Three copies of the UI code |

- **CPU, not GPU:** GPU device setup often costs 100 ms or more and tens of MB of driver memory, which by itself breaks the budgets. A full redraw is 0.5 ms at 1080p and 2.4 ms at 5K ([fude-render](crates/fude-render/README.md#performance)), so every frame is a full redraw for now.
- **The draw list is the seam.** The UI produces a flat list of rectangles and glyph images, which is also what a GPU backend would consume. If CPU rendering can't keep up (most likely on 5K Retina at 120 Hz), a Metal or Direct3D 11 backend can be added without touching the UI.
- **Native where it matters:** file dialogs (`rfd`, using the XDG portal on Linux, so no GTK) and the macOS menu bar. Questions like "Save changes?" are drawn in the window, like menus on other platforms, so they look the same everywhere and need no helper programs.
- **Every text input is an editor.** The find box, go to line and the command palette reuse `fude-editor`, so they get IME, undo, selection and the clipboard for free.
- **Accessibility:** a custom-drawn UI is invisible to screen readers unless it describes itself, so every widget does, as an [AccessKit](https://accesskit.dev) tree: roles, names, states, bounds, and text with its selection ([fude-ui](crates/fude-ui/README.md#accessibility)). AccessKit answers each OS's accessibility API (UI Automation, NSAccessibility, AT-SPI) from it ([fude-platform](crates/fude-platform/README.md#accessibility)). The tree is built only while an assistive technology is listening, after each frame, so without one it costs nothing; with one, a document describes only the rows on screen, so it costs the same at any file size.

## Commands

Everything is a named command (`line.duplicate`, `view.toggle_wrap`, …). Keybindings, the menus (drawn in the window, or the macOS menu bar), the context menu and the command palette are front-ends over one table, so a shortcut shown anywhere is the one that runs ([fude-ui](crates/fude-ui/README.md#commands-and-shortcuts)). Commands are also the seam for any future scripting ([Extensibility](#extensibility)).

## Large files

v1 holds files of hundreds of MB in memory and keeps every operation on the UI thread independent of their size:

- **Loading** streams on a worker: the first 64 KB shows at once as a read-only preview, and the document becomes editable when the rest is in, a fraction of a second later ([fude-buffer](crates/fude-buffer/README.md#loading), [fude](fude/README.md#opening-files)).
- **Lookups** are O(log n): the rope's summaries answer byte ↔ line in about a microsecond at any size.
- **Long lines** are laid out in pieces of about 4 KB, and the scroll position is a byte anchor, so a 100 MB line costs no more per frame than a short one ([fude-ui](crates/fude-ui/README.md#the-editor-view)).
- **Thresholds:** highlighting lexes the whole file in the background only up to 16 MB, and skips lines over 10 KB ([fude-syntax](crates/fude-syntax/README.md#large-files)); find, counting, Replace All and tools move to workers past small sizes ([fude](fude/README.md#find-and-replace)); files over 1 GB ask before opening.
- **Later:** paged mode for multi-GB files, reading from disk on demand.

## Never losing work

- **Atomic saves:** the original is never truncated before the new contents are safely on disk ([fude-buffer](crates/fude-buffer/README.md#saving)).
- **Hot exit and crash recovery:** the session and unsaved text are written two seconds after any change, so quitting never asks and a crash loses at most a few seconds ([fude](fude/README.md#hot-exit-and-crash-recovery)).
- **Fault isolation:** event handling and drawing run inside `catch_unwind`, so a bug costs an error message or a frame, not unsaved text ([fude](fude/README.md#fault-isolation)).

## Extensibility

No plugin API in v1. Extensibility comes in three layers:

1. **Built-in tools.** Features people commonly install plugins for are bundled as commands: sorting, case, Base64 / URL / HTML, hashes, JSON and XML formatting ([fude-tools](crates/fude-tools/README.md)).
2. **Data, not code** *(planned)*. Syntax definitions (`.sublime-syntax`), color themes (`.tmTheme` or a fude theme file) and keymaps can be dropped into the config directory.
3. **External tools.** Any program can be run as a fude command, taking the selection or document on stdin and replacing it, inserting the output, opening it in a new tab or showing it in the status bar. This covers formatters (`jq`, `prettier`, `black`, `rustfmt`), linters and personal scripts in any language ([fude](fude/README.md#text-tools)).

**Why no plugin API:** an API is a compatibility promise that can't be taken back; third-party code can't be held to the "no O(n) on the UI thread" rule, but fude gets blamed when it's slow; loading plugins costs startup time; and Rust has no stable ABI, so native plugins would mean a C ABI or an embedded runtime.

**Leaving the door open:** commands are the seam. If a real need appears, an embedded runtime (Lua via `mlua`, or WASM via `wasmi`) can call and register commands without restructuring the editor.

## Platform support

| | Windows | macOS | Linux | BSD |
|---|---|---|---|---|
| Window and input | winit | winit | winit (X11 + Wayland, loaded at runtime); file drops on Wayland by fude | winit |
| Font lookup | DirectWrite | CoreText | fontconfig | fontconfig |
| File dialogs | IFileDialog | NSOpenPanel | XDG portal, with fallback | XDG portal, with fallback |
| Menus | in-window | native menu bar | in-window | in-window |
| Single instance | loopback TCP + token | loopback TCP + token (Finder open events: later) | loopback TCP + token | loopback TCP + token |

Details: [fude-platform](crates/fude-platform/README.md) (windowing, Wayland drops, the menu bar), [fude](fude/README.md#single-instance) (single instance, launching from a terminal).

## Testing

- **Unit and property tests** in each crate (`cargo test --workspace`); the buffer's run with tiny nodes so short inputs build deep trees.
- **Fuzzing:** three `cargo-fuzz` targets for the buffer ([fuzz](fuzz/README.md)).
- **Benchmarks:** startup, rendering, typing, the buffer, and a generated large-file corpus ([xtask](xtask/README.md)).
- **UI test:** `cargo xtask ui-test` drives the real window through X11 and checks the files it saves (26 checks).
- **Accessibility tests** read the tree back with `accesskit_consumer` (what the OS adapters use), which also checks it's valid: the document's text and selection, focus through the find bar, palette, menus and dialogs, and that only the rows on screen are described.
- **Pixel snapshot tests** for the renderer and widgets *(planned; CI uploads a screenshot per platform, nothing compares them yet)*.
- **CI** on Windows, macOS and Linux: formatting, clippy, tests, a release build, the render and typing benchmarks, and the large-file benchmark on the small corpus, which fails a missed budget on Linux. On Linux under Xvfb, the startup benchmark (report-only until there's a baseline for runner noise) and the UI test. A FreeBSD build and tests in a VM, and a 45-second fuzz run per target.

## Scope

**v1**

- Files: tabs, open/save, recent files, drag and drop, hot exit, single instance.
- Editing: undo/redo, multiple cursors, column selection, bracket matching, auto-indent, toggle comment.
- Search and navigation: find/replace with regex, go to line, command palette.
- View: line numbers, status bar (line/column, encoding, line endings, language), word wrap, zoom, light/dark following the OS.
- Formats: encoding and line-ending detection and conversion, syntax highlighting (75 bundled grammars), large files up to hundreds of MB.
- Tools: built-in text tools and external tools.

**v1.x:** find in files, split view, compare files (side-by-side diff), bookmarks, code folding, highlight all occurrences, macros, spell check.

**Later:** paged mode for multi-GB files, hex view, GPU rendering backend (only if measurements call for it).

**Non-goals:** language servers, a plugin API, an integrated terminal, git integration, a project sidebar. Settings live in a TOML file opened like any other document, at least initially.

## Roadmap

| Phase | Deliverable |
|---|---|
| 0. Skeleton and budgets | Workspace; a window drawing text through the text stack; `--bench-startup`; CI on all platforms. Go/no-go on CPU rendering. |
| 1. Buffer | Rope, snapshots, undo, encodings, line endings, safe save; property tests and fuzzing. |
| 2. Basic editor | Cursor, selection, keyboard and mouse, scrolling, clipboard, IME, gutter, status bar. A usable Notepad. |
| 3. Core features | Tabs, find/replace, syntax highlighting, word wrap, zoom, command palette, config and themes, single instance, hot exit. |
| 4. Large files | Background loading, long-line layout, feature thresholds, large-file benchmark corpus. |
| 5. Power features | Multiple cursors, column selection, bracket matching, auto-indent, toggle comment, built-in and external tools. |
| 6. Release | macOS bundle and notarization, Windows portable build and installer, Linux AppImage and Flatpak, accessibility. |

## Risks

| Risk | Mitigation |
|---|---|
| Text rendering quality from a pure-Rust stack (ClearType expectations; CJK and emoji fallback) | Native rasterizers (DirectWrite, CoreText) inside `fude-text` if needed |
| CPU rendering on high-DPI, high-refresh displays | Damage tracking, scroll blits; measured in phase 0; GPU backend as plan B |
| Linux without GTK: Wayland client-side decorations, X11 IME, systems without an XDG portal | Our own title bar on Wayland; dialog fallbacks |
| Accessibility in a custom-drawn UI is real work | Done through AccessKit: every widget describes itself, tested by reading the tree back with AccessKit's own consumer, and on Linux over AT-SPI. Not yet tried with NVDA, Narrator or VoiceOver |
| Very long lines (minified files) | Done in phase 4: lines are laid out in pieces; a keystroke in a 100 MB line costs 1.3 ms |

## Dependencies

Each is evaluated for startup, RAM and binary size when introduced.

| Crate | Purpose | Used in |
|---|---|---|
| `winit` | Windows, input, IME, DPI | fude-platform |
| `softbuffer` | Present CPU framebuffers | fude-platform |
| `rfd` | Native file dialogs | fude-platform |
| `arboard` | Clipboard | fude-platform |
| `muda` | The macOS menu bar (macOS only) | fude-platform |
| `fontique` | Font discovery and fallback | fude-text |
| `harfrust` | Text shaping | fude-text |
| `swash` | Glyph rasterization | fude-text |
| `syntect` | Syntax highlighting | fude-syntax |
| `regex-cursor`, `regex-syntax` | Search over the rope | fude-editor |
| `unicode-segmentation` | Grapheme and word boundaries | fude-editor |
| `encoding_rs`, `chardetng` | Encodings and detection | fude-buffer |
| `memchr` | Fast byte scanning | fude-buffer |
| `toml` | Settings | fude |
| `accesskit`, `accesskit_winit` | Accessibility: the tree, and each OS's API for it | fude-ui (the tree), fude-platform (the adapter) |

## Decision log

Decisions that shape the whole project. Each crate's README logs its own.

| Date | Decision |
|---|---|
| 2026-11-07 | Rust. |
| 2026-11-07 | Custom-drawn UI with a consistent look; behavior follows platform conventions. |
| 2026-11-07 | CPU rendering first; GPU backend only if measurements require it. |
| 2026-11-07 | Custom rope buffer storing raw bytes. |
| 2026-11-07 | v1 large-file target is hundreds of MB, loaded in memory in the background. Paged mode for multi-GB files is deferred. |
| 2026-11-07 | No plugin API in v1. Extensibility through built-in tools, data files and external tools. |
| 2026-11-07 | CPU rendering confirmed by phase 0 measurements. |
| 2026-11-07 | Apache-2.0 license. |
| 2026-11-11 | Accessibility through AccessKit (`accesskit_winit`, as egui and Bevy use) rather than code per OS: it does the hard part, each OS's API and its text interfaces. The tree is built after each frame, only while an assistive technology listens; documents describe the rows on screen, not the file. Its crates are built with `opt-level = "z"`; on Linux it costs 1.9 MB of binary. |
| 2026-11-11 | The binary budget goes from 10 MB to 12 MB, for accessibility's 1.9 MB on Linux, rather than paying for it now by dropping syntax grammars (deferred). |
| 2026-11-09 | Application menus: drawn in the window on Windows, Linux and BSD, below which the tabs move down 28 px; the native menu bar on macOS through `muda` (Tauri's menu crate, macOS-only dependency) rather than hand-written AppKit code, which would be another `unsafe` module. Shortcuts become key equivalents there, so a menu item and its key always do the same thing; a shortcut an external tool takes over is left off its built-in item. |

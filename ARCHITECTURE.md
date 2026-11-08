# fude architecture

**fude** (筆, "brush"): **F**ast, **U**ncomplicated **D**ocument **E**ditor.

A cross-platform text editor that sits between Notepad and Notepad++. It should start instantly, respond instantly at any file size, use little memory, and look and feel polished on every platform.

> Status: phase 0 (skeleton and budgets) done. Next: phase 1, the buffer. Last updated 2026-11-07.

---

## 1. Goals

- **Platforms:** Windows, macOS, Linux. BSD is a nice-to-have; the stack below should support it with little extra effort.
- **Fast:** near-instant launch, no perceptible input latency, smooth scrolling.
- **Large files:** files of hundreds of MB open quickly and stay responsive. Multi-GB files are a later goal (§9).
- **Light:** low RAM use and zero CPU when idle.
- **Good UX:** a consistent, polished UI on every platform.
- **Uncomplicated:** a focused feature set, not an IDE.

## 2. Principles

1. **The UI thread never does work proportional to file size.** A frame costs O(visible text) plus O(log n) tree lookups. Anything proportional to file size (loading, line counting, find-all, full highlighting, saving, encoding conversion) runs on a worker thread and streams results back.
2. **Approximate now, exact later.** Show the first screen immediately. Line numbers, the scrollbar, syntax colors and match counts fill in or correct themselves as background work finishes.
3. **Event-driven, zero idle CPU.** No redraw loop. Repaint only what changed, only when something changed.
4. **Lazy startup.** Nothing loads before the first frame unless the first frame needs it.
5. **The file's bytes are the truth.** Opening and saving an unedited file gives back identical bytes, including invalid UTF-8 and mixed line endings.
6. **Every dependency has to earn its place,** measured in startup time, RAM and binary size.
7. **Budgets are tests.** CI fails when a performance budget regresses (§16).

## 3. Performance budgets

| Metric | Target | Measured (phase 0) |
|---|---|---|
| Launch → first frame, warm cache | < 40 ms | 11.4 ms median (Linux, X11) |
| Launch → first frame, cold | < 150 ms | |
| Open a 500 MB file → first screen | < 100 ms | |
| Open a 500 MB file → fully loaded and editable | < 1 s (SSD or warm cache) | |
| Keypress → frame submitted | < 4 ms of fude's own work | |
| Idle RAM, empty document, 1080p | < 30 MB | 12.7 MB (Linux, X11) |
| RAM for an open, unedited file | ≈ 1.1 × file size | |
| Idle CPU | 0 % | 0 % (event-driven) |
| Release binary | < 10 MB | 5.0 MB (Linux x86-64) |

Phase 0 measurements were taken on a fast desktop (32 threads) with `cargo xtask bench-startup` under Xvfb, so they are a best case; laptops and Wayland/Windows/macOS numbers come next.

**CPU rendering go/no-go (phase 0):** full-frame redraws with no damage tracking, single thread, `fude --bench-render`:

| Resolution | Median full redraw | First frame (cold glyph cache) |
|---|---|---|
| 1920×1080 @1× | 0.54 ms | 3.2 ms |
| 2560×1440 @2× | 0.78 ms | 4.8 ms |
| 3840×2160 @2× | 1.29 ms | 9.2 ms |
| 5120×2880 @2× | 2.43 ms | 15.3 ms |

Even the 5K worst case fits a 120 Hz frame (8.3 ms) several times over, so CPU rendering is a go. Presenting the frame (copying to the compositor) isn't included and needs measuring per platform, macOS especially.

**Startup breakdown (Linux, X11):** about 8 ms is winit creating the event loop, more than half of it libX11 and xkbcommon each parsing the X11 Compose table. fude's own work (fonts, layout, rasterizing the first frame) is about 2 ms. Deferring the Compose table until the first key press is a possible later optimization.

## 4. UX principles

- **Look consistent, behave native.** One visual design on every platform, but behavior follows each platform's conventions: Cmd vs Ctrl, Option+arrow word jumps on macOS, OS trackpad scrolling and momentum, native file dialogs, the macOS menu bar, double- and triple-click selection.
- **One design system.** All UI is built from shared tokens (spacing scale, type scale, color roles, corner radii, motion) and one widget kit. No one-off styling.
- **Keyboard first, mouse friendly.** Every action is a command, and every command appears in the command palette with its shortcut, so features are discoverable without digging through menus.
- **Minimal chrome.** No toolbar. Tabs on top, status bar at the bottom. Status bar items (encoding, line endings, language, indentation) double as controls: click one to change it.
- **Only modal when necessary.** Find/replace and go-to-line are inline bars, not dialogs. Long operations show progress and can be cancelled.
- **Never lose work.** Hot exit, background backups of unsaved changes, atomic saves.
- **Themes.** Light and dark follow the OS by default. Text contrast meets WCAG AA. Everything is crisp on high-DPI displays.

## 5. System overview

### Layers

```
┌──────────────────────────────── fude (binary) ────────────────────────────────┐
│  command line · config · single-instance IPC · wiring                          │
├────────────────────────────────────────────────────────────────────────────────┤
│  fude-ui       editor view, gutter, tabs, find bar, menus, palette, status bar │
├──────────────────┬───────────────────────┬─────────────────────────────────────┤
│  fude-render     │  fude-text            │  fude-platform                      │
│  draw list,      │  fonts, fallback,     │  winit glue, clipboard, dialogs,    │
│  CPU rasterizer  │  shaping, glyph cache │  IPC, file watching, OS theme, menus│
├──────────────────┴───────────────────────┴─────────────────────────────────────┤
│  fude-editor   documents, selections, transactions, undo, commands, search,    │
│                view state (scroll anchor, wrap)                                │
├────────────────────────────────────────┬───────────────────────────────────────┤
│  fude-buffer                           │  fude-syntax                          │
│  rope, snapshots, line index,          │  highlighter trait, syntect backend,  │
│  encodings, line endings, load/save    │  lexer-state checkpoints              │
└────────────────────────────────────────┴───────────────────────────────────────┘
```

Dependencies point downward only. `fude-buffer`, `fude-editor` and `fude-syntax` have no UI dependencies and are tested headless.

### Repository layout

```
Cargo.toml        workspace
crates/
  fude-buffer/
  fude-editor/
  fude-syntax/
  fude-text/
  fude-render/
  fude-ui/
  fude-platform/
fude/             binary crate
benches/          startup, RAM, large-file and micro benchmarks
fuzz/             cargo-fuzz targets
```

## 6. Rendering and UI

**Approach: a custom-drawn UI on top of winit, rendered on the CPU.**

Most of an editor's UI is the text view, and no general-purpose toolkit has a text widget that handles large files or very long lines well. Since we have to write that view ourselves, the rest of the chrome is small enough to draw too. This is the approach Sublime Text takes.

| Rejected option | Why |
|---|---|
| Electron / Tauri | Startup and RAM an order of magnitude over budget |
| Qt / GTK | 100 ms+ startup, tens of MB of RAM; GTK looks foreign on Windows and macOS |
| egui / iced / Slint | GPU startup cost; their text widgets lay out the whole document |
| Native widgets per OS | Three copies of the UI code |

- **Windowing and input:** `winit` (Windows, macOS, X11, Wayland, BSD; DPI; IME). It lives in `fude-platform` and is translated into fude's own event types, so it can be replaced with per-OS code if it ever becomes a limitation.
- **Renderer:** draws into a CPU framebuffer shown with `softbuffer`. Only damaged regions are redrawn; scrolling moves existing pixels and draws only the exposed strip. GPU device setup often costs 100 ms or more and tens of MB of driver memory, which by itself breaks the budgets. CPU output is also deterministic, so the renderer can have pixel snapshot tests.
- **Draw list:** the UI produces a flat list of rectangles and glyph instances from an atlas, which is also what a GPU backend would consume. If CPU rendering can't keep up (most likely on 5K Retina at 120 Hz), a Metal or Direct3D 11 backend can be added behind the same interface without touching the UI.
- **Native pieces:** file dialogs and message boxes (`rfd`, using the XDG portal on Linux, so no GTK) and the macOS menu bar. Menus on other platforms are drawn in-window, so theming is consistent.
- **Every text input is an editor.** The find box, go-to-line and the command palette reuse `fude-editor`, so they get IME, undo, selection and clipboard for free.
- **Accessibility:** the widget tree carries roles and labels from the start, so AccessKit can be wired up in v1.x without restructuring.

## 7. Text stack (`fude-text`)

- **Default font:** JetBrains Mono is bundled (OFL; two variable font files, about 380 KB, covering regular, bold and italic), so the first frame needs no font discovery and looks the same on every platform.
- **Font discovery and fallback:** resolve system fonts (user-chosen fonts, CJK, emoji) only when needed. Candidate: `fontique`, which uses fontconfig, CoreText and DirectWrite under the hood.
- **Shaping:** `harfrust` (pure-Rust HarfBuzz port). Simple runs skip shaping and map characters straight to glyphs, which is correct for monospace fonts without ligatures, so most lines never touch the shaper.
- **Rasterization:** `swash` renders alpha masks (and color emoji) into an LRU glyph atlas keyed by font, glyph, size and ¼-pixel horizontal offset. Shaped lines are cached by content.
- **Escape hatch:** rasterization sits behind a trait so a DirectWrite backend can be added on Windows if swash's output doesn't match ClearType expectations.

## 8. Buffer (`fude-buffer`)

A custom persistent B-tree rope. No existing crate gives us exact byte preservation, a path to file-backed leaves, and O(1) snapshots together.

```
              [bytes=212M, lines=3.1M]      ← every node caches a summary
             /            |           \
     [leaf "…"]     [leaf "fn ma…"]    [leaf "…"]      leaves hold ~1–4 KB of bytes
```

- **Summaries:** each node caches its byte length and line-break count, so converting between byte offsets and line numbers is O(log n).
- **Shared, copy-on-write nodes.** An edit copies O(log n) nodes. A snapshot is a clone of the root pointer, O(1). This is what makes the threading model (§10) work: the highlighter, search and save each work from an immutable snapshot and never take a lock.
- **Undo** is a stack of previous roots plus selections. Structural sharing makes it nearly free in memory. "Is the document unsaved?" is "is the current root the saved root?" (compared by version ID).
- **Positions are byte offsets.** Line and column are derived from them.
- **Line endings:** LF, CRLF and CR are stored as-is. A CRLF split across two leaves still counts as one line break. The dominant style is detected and used for new lines; conversion is a command.
- **Encodings:** BOM sniffing, then `chardetng` (Firefox's detector, which handles Shift-JIS and friends). UTF-8 files are stored raw; invalid bytes render as `<FF>`-style chips and round-trip untouched. UTF-16 and legacy encodings are transcoded to UTF-8 on load (`encoding_rs`) and back on save.
- **Future-proofing:** the leaf type is an enum. v1 only has owned-byte leaves; a file-span leaf for paged mode (§9) can be added later without changing the public API.
- **Testing:** property tests against a naive `Vec<u8>` model, plus `cargo-fuzz` targets for edits, line indexing and encoding round-trips.

## 9. Opening and saving files

### v1: in memory, loaded in the background

1. Read the first 64 KB, detect encoding and line endings, and show the first screen as a read-only preview.
2. A worker reads the rest in ~1 MB chunks, transcodes if needed, counts line breaks, and builds the rope bottom-up.
3. When it finishes, the full rope replaces the preview. The scroll position is a byte-offset anchor, so the view stays put. Editing is enabled.
4. Files above a configurable size (default 1 GB) ask before opening.

### Later: paged mode for multi-GB files

Add file-span leaves that point into the file on disk. Chunks are read on demand with `pread` into a bounded LRU cache, and a background `memchr` pass counts lines (several GB/s). Line numbers and the scrollbar are estimates until it finishes.

Why `pread` rather than `mmap`: on Unix, a mapped file crashes the process (SIGBUS) if another program truncates it. On Windows, mapping a file stops other programs from truncating or replacing it ("file in use"). `pread` costs one extra copy and avoids both. If the file changes on disk while paged, fude detects it (mtime and size) and asks what to do.

### Saving

Saves run on a snapshot in a worker, so the UI stays responsive. fude writes a temporary file in the same directory, flushes it to disk, then renames it over the original (`ReplaceFile` on Windows). It preserves permissions, follows symlinks, and never truncates the original in place.

### External changes

fude checks mtime and size when the window gains focus and through file watching. A clean document reloads automatically (configurable); a modified one prompts.

## 10. Threads

```
UI thread (winit event loop)                         Worker pool
 input → command → edit rope (new root) ─snapshot─▶ highlighter
 lay out visible lines only             ─snapshot─▶ search / replace all
 build draw list → rasterize damage     ─snapshot─▶ save, session backup
 present                                            loader, file watcher
   ▲                                                      │
   └──── results tagged with document version ◀───────────┘
         (results for an older version are dropped; EventLoopProxy wakes the UI)
```

- Plain threads and channels. No async runtime.
- Each job carries a generation number, so typing a new search query cancels the previous search.

## 11. Editing model

- **Multiple selections from day one.** Selections are a sorted, non-overlapping list of `{anchor, head, goal_x}`; a single cursor is a list of one. Column (box) selection builds on this.
- **Edits are transactions:** a list of range replacements applied atomically. All selections are mapped through each edit. Undo groups typing by pauses and word boundaries.
- **Everything is a named command** (`edit.duplicate_line`, `view.toggle_wrap`, …). Keybindings, menus and the command palette are front-ends that invoke commands with arguments.
- **Fault isolation:** commands run inside `catch_unwind`. A bug in a feature shows an error instead of losing unsaved text.
- **Hot exit and crash recovery:** unsaved documents are backed up to a session directory in the background and restored on next launch along with tabs and cursor positions.
- **Unicode:** cursor movement steps over grapheme clusters; an invalid byte is one step.

## 12. Syntax highlighting (`fude-syntax`)

- **Line-state model** (like Sublime and Scintilla): the lexer state is saved every ~128 lines. Highlighting starts from the nearest checkpoint above the viewport. After an edit, re-lex until the state matches the old checkpoint, then stop.
- **Engine:** `syntect` (Sublime syntax definitions, wide language coverage, pure Rust via `fancy-regex`). Grammars are precompiled into the binary and loaded lazily per language.
- **Jumping deep into a large file:** lex from a guessed resync point ~1,000 lines above the viewport for colors that are immediately (usually) right; a background pass corrects them.
- **Size thresholds:** above a configurable size, highlighting covers only the viewport; above a higher one, it is off.
- **Why not tree-sitter:** it parses the whole file up front, and each grammar adds hundreds of KB to several MB to the binary. The `Highlighter` trait leaves room to add it later for structure features like folding.

## 13. Search

- **Engine:** the `regex` crate (linear time, so no pathological pattern can hang the editor), run directly over rope chunks through `regex-cursor`'s cursor trait. Replace supports `$1`-style groups. Patterns that need lookaround or backreferences can fall back to `fancy-regex`.
- **Incremental:** the viewport is searched first; the rest of the document streams from a worker and can be cancelled. The match count fills in progressively.
- **Find in files (v1.x):** ripgrep's `ignore` and `grep-searcher` crates.

## 14. Extensibility

No plugin API in v1. Extensibility comes in three layers:

1. **Built-in tools.** Features people commonly install plugins for are bundled as commands: sort lines, remove duplicates, reverse, trim whitespace, tabs ↔ spaces, case conversion, join lines, Base64 / URL / HTML encode and decode, hashes, JSON format and minify, XML format. They cost nothing at startup and live in the command palette and a Tools menu rather than in the main menus.
2. **Data, not code.** Syntax definitions (`.sublime-syntax`), color themes (`.tmTheme` or a fude theme file) and keymaps can be dropped into the config directory.
3. **External tools.** Any program can be run as a fude command, taking the selection or document on stdin and replacing it, inserting the output, or showing it in a panel. This covers formatters (`jq`, `prettier`, `black`, `rustfmt`), linters and personal scripts in any language:

   ```toml
   [[tool]]
   name    = "Format JSON (jq)"
   command = ["jq", "."]
   input   = "selection-or-document"
   output  = "replace"
   key     = "ctrl+alt+j"
   ```

**Why no plugin API:** an API is a compatibility promise that can't be taken back; third-party code can't be held to the "no O(n) on the UI thread" rule, but fude gets blamed when it's slow; loading plugins costs startup time; and Rust has no stable ABI, so native plugins would mean a C ABI or an embedded runtime.

**Leaving the door open:** commands are the seam. If a real need appears, an embedded runtime (Lua via `mlua`, or WASM via `wasmi`) can call and register commands without restructuring the editor.

## 15. Platform support

| | Windows | macOS | Linux | BSD |
|---|---|---|---|---|
| Window and input | winit | winit | winit (X11 + Wayland, loaded at runtime) | winit |
| Font lookup | DirectWrite | CoreText | fontconfig | fontconfig |
| File dialogs | IFileDialog | NSOpenPanel | XDG portal, with fallback | XDG portal, with fallback |
| Menus | in-window | native menu bar | in-window | in-window |
| Single instance | named pipe | OS open-file events | Unix socket | Unix socket |

Single instance: opening a file while fude is running sends it to the existing window as a new tab, so no second process starts.

## 16. Testing and benchmarks

- **Unit and property tests** for buffer, editor and syntax crates; `cargo-fuzz` targets for the buffer.
- **Pixel snapshot tests** for the renderer and widgets.
- **Startup:** a `--bench-startup` flag exits right after the first frame is presented and prints timings; measured with `hyperfine`.
- **RAM:** peak and idle RSS measured for an empty document and for the large-file corpus.
- **Large-file corpus,** generated by a script rather than committed: a 500 MB log, a 100 MB single-line JSON file, UTF-16 and Shift-JIS files, invalid UTF-8, mixed line endings.
- **Micro benchmarks** for rope edits, line lookups, layout and search.
- **CI:** Windows, macOS and Linux; a FreeBSD build in a VM. Budget regressions fail the build.

Release profile:

```toml
[profile.release]
lto = "fat"
codegen-units = 1
strip = true
panic = "unwind"   # commands run under catch_unwind (§11)
```

## 17. Scope

**v1**

- Files: tabs, open/save, recent files, drag and drop, hot exit, single instance.
- Editing: undo/redo, multiple cursors, column selection, bracket matching, auto-indent, toggle comment.
- Search and navigation: find/replace with regex, go to line, command palette.
- View: line numbers, status bar (line/column, encoding, line endings, language), word wrap, zoom, light/dark following the OS.
- Formats: encoding and line-ending detection and conversion, syntax highlighting for about 30 languages, large files up to hundreds of MB.
- Tools: built-in text tools and external tools (§14).

**v1.x:** find in files, split view, compare files (side-by-side diff), bookmarks, code folding, highlight all occurrences, macros, spell check, screen reader support via AccessKit.

**Later:** paged mode for multi-GB files, hex view, GPU rendering backend (only if measurements call for it).

**Non-goals:** language servers, a plugin API (§14), an integrated terminal, git integration, a project sidebar. Settings live in a TOML file opened like any other document, at least initially.

## 18. Roadmap

| Phase | Deliverable |
|---|---|
| 0. Skeleton and budgets | Workspace; a window drawing text through the text stack; `--bench-startup`; CI on all platforms. Go/no-go on CPU rendering. |
| 1. Buffer | Rope, snapshots, undo, encodings, line endings, safe save; property tests and fuzzing. |
| 2. Basic editor | Cursor, selection, keyboard and mouse, scrolling, clipboard, IME, gutter, status bar. A usable Notepad. |
| 3. Core features | Tabs, find/replace, syntax highlighting, word wrap, zoom, command palette, config and themes, single instance, hot exit. |
| 4. Large files | Background loading, long-line layout, feature thresholds, large-file benchmark corpus. |
| 5. Power features | Multiple cursors, column selection, bracket matching, auto-indent, toggle comment, built-in and external tools. |
| 6. Release | macOS bundle and notarization, Windows portable build and installer, Linux AppImage and Flatpak, accessibility. |

## 19. Risks

| Risk | Mitigation |
|---|---|
| Text rendering quality from a pure-Rust stack (ClearType expectations; CJK and emoji fallback) | Rasterizer trait; native backends (DirectWrite, CoreText) if needed |
| CPU rendering on high-DPI, high-refresh displays | Damage tracking, scroll blits; measured in phase 0; GPU backend as plan B |
| Linux without GTK: Wayland client-side decorations, X11 IME, systems without an XDG portal | Our own title bar on Wayland; dialog fallbacks |
| Accessibility in a custom-drawn UI is real work | Semantic widget tree from day one; AccessKit |
| Very long lines (minified files) | Lay out only visible slices; x-position checkpoints along long lines |

## 20. Candidate dependencies

Each is evaluated for startup, RAM and binary size when introduced, and pinned to an exact version.

| Crate | Purpose |
|---|---|
| `winit` | Windows, input, IME, DPI |
| `softbuffer` | Present CPU framebuffers |
| `rfd` | Native file dialogs and message boxes |
| `fontique` | Font discovery and fallback |
| `harfrust` | Text shaping |
| `swash` | Glyph rasterization |
| `syntect` | Syntax highlighting |
| `regex`, `regex-cursor` | Search over the rope |
| `encoding_rs`, `chardetng` | Encodings and detection |
| `memchr` | Fast byte scanning |
| `unicode-segmentation` | Grapheme and word boundaries |
| `arboard` | Clipboard |
| `toml` | Config |
| `accesskit` | Accessibility (v1.x) |

## 21. Decision log

| Date | Decision |
|---|---|
| 2026-11-07 | Rust. |
| 2026-11-07 | Custom-drawn UI with a consistent look; behavior follows platform conventions. |
| 2026-11-07 | CPU rendering first; GPU backend only if measurements require it. |
| 2026-11-07 | Custom rope buffer storing raw bytes. |
| 2026-11-07 | v1 large-file target is hundreds of MB, loaded in memory in the background. Paged mode for multi-GB files is deferred. |
| 2026-11-07 | No plugin API in v1. Extensibility through built-in tools, data files and external tools. |
| 2026-11-07 | winit 0.30 (stable) rather than the 0.31 beta; contained in `fude-platform`, so upgrading later is local. |
| 2026-11-07 | Bundle JetBrains Mono as the default font. |
| 2026-11-07 | CPU rendering confirmed by phase 0 measurements (§3). |
| 2026-11-07 | Shaping and glyph metrics come from harfrust (which also drives the ASCII fast path, so both paths agree); swash only rasterizes. |
| 2026-11-07 | Apache-2.0 license. |

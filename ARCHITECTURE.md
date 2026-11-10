# fude architecture

**fude** (筆, "brush"): **F**ast, **U**ncomplicated **D**ocument **E**ditor.

A cross-platform text editor that sits between Notepad and Notepad++. It should start instantly, respond instantly at any file size, use little memory, and look and feel polished on every platform.

> Status: phase 5 (power features) done, plus a context menu and the application menus. Next: phase 6, release. Last updated 2026-11-09.

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

| Metric | Target | Measured |
|---|---|---|
| Launch → first frame, warm cache | < 40 ms | 10.3 ms median (Linux, X11); a launch that hands its files to a running fude exits in under 10 ms |
| Launch → first frame, cold | < 150 ms | |
| Open a 500 MB file → first screen | < 100 ms | 23 ms (its first 64 KB, while the rest loads in the background) |
| Open a 500 MB file → fully loaded and editable | < 1 s (SSD or warm cache) | 0.2 s |
| Keypress → frame submitted | < 4 ms of fude's own work | 0.5 ms at 1080p, 1.4 ms at 4K (edit plus a full redraw, with syntax colors); file size doesn't change it, and inside a 100 MB line it's 1.3 ms |
| Idle RAM, empty document, 1080p | < 30 MB | 14.6 MB (Linux, X11) |
| RAM for an open, unedited file | ≈ 1.1 × file size | 1.04 × the text (UTF-8); 1.2 × for Shift_JIS, UTF-16 and Japanese text, most of the difference being the CJK fallback font |
| Idle CPU | 0 % | 0 CPU ticks over 5 s once the caret stops blinking (10 s after input) |
| Release binary | < 10 MB | 9.6 MB (Linux x86-64): encodings added 0.3 MB in phase 1; font fallback, clipboard and file dialogs 1.1 MB in phase 2; syntax highlighting (syntect and its grammars) 2.4 MB and search 0.4 MB in phase 3; 0.2 MB each in phases 4 and 5; the menu bar 20 KB (`muda`, for the macOS menu bar, is only built there). Little room is left: the next large feature has to pay for itself (dropping grammars, or `opt-level = "s"` for cold code). |

Measurements are from a fast desktop (32 threads, NVMe, warm page cache): startup with `cargo xtask bench-startup` under Xvfb, the buffer with `cargo run --release -p fude-buffer --example bench` on a generated 500 MB log, keystrokes with `fude --bench-typing`. They are a best case; laptops and Wayland/Windows/macOS numbers come next.

**Buffer (phase 1),** 500 MB log, 5.8 M lines: line → byte 1.1 µs and byte → line 0.8 µs at random positions (mostly cache misses); the 60 visible lines of a screen, found and copied out, 5.7 µs; snapshot 8 ns; undo history 7.6 KB per step; save including fsync 180 ms.

**CPU rendering go/no-go (phase 0):** full-frame redraws with no damage tracking, single thread, `fude --bench-render`:

| Resolution | Median full redraw | First frame (cold glyph cache) |
|---|---|---|
| 1920×1080 @1× | 0.54 ms | 3.2 ms |
| 2560×1440 @2× | 0.78 ms | 4.8 ms |
| 3840×2160 @2× | 1.29 ms | 9.2 ms |
| 5120×2880 @2× | 2.43 ms | 15.3 ms |

Even the 5K worst case fits a 120 Hz frame (8.3 ms) several times over, so CPU rendering is a go. Presenting the frame (copying to the compositor) isn't included and needs measuring per platform, macOS especially.

**Large files (phase 4),** the generated corpus (`cargo xtask bench-large`, §16), each opened without a window at 1920×1080; keystrokes are an edit plus a full frame, medians:

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

**Multiple cursors (phase 5):** typing at 10,000 cursors in a 100 MB line takes 5.7 ms per key; at 5,750 cursors spread over a 500 MB log, 10.7 ms (about 2 µs per cursor for the edit; drawing looks only at the cursors on screen). Selecting every occurrence of a word scans the text at about 10 GB/s: 55 ms for the 500 MB log.

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
  fude-buffer/    (examples/bench.rs: large-file benchmark)
  fude-editor/
  fude-syntax/
  fude-text/
  fude-render/
  fude-ui/
  fude-platform/
fude/             binary crate
xtask/            cargo xtask: startup benchmark
fuzz/             cargo-fuzz targets (a separate workspace)
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
- **Native pieces:** file dialogs (`rfd`, using the XDG portal on Linux, so no GTK) and the macOS menu bar. Questions like "Save changes?" are drawn in the window, like menus on other platforms, so they look the same everywhere and need no helper programs.
- **Menus:** one description of the menus (`fude_ui::menus`: File, Edit, Selection, View, Go, Tools, Help) drives both the menu bar drawn in the window (Windows, Linux, BSD) and the macOS menu bar, and each item shows its command's shortcut from the same table as the command palette, so the three can't disagree. The drawn bar's menus are the context menu widget; F10 or Alt and a menu's first letter open them from the keyboard. On macOS, `fude-platform` builds the menu bar with `muda`, moving About, Settings and Quit to the application menu and adding the standard items (Services, Hide, the Window menu); Quit is fude's own command, not `terminate:`, so quitting keeps hot exit. Items are enabled from the same state as the drawn menus, updated after each event.
- **Every text input is an editor.** The find box, go-to-line and the command palette reuse `fude-editor`, so they get IME, undo, selection and clipboard for free.
- **Lines in pieces.** The view lays out *pieces*: a line up to 4 KB is one; a longer one (minified code, a CSV row, a single-line JSON file) is cut into pieces of about 4 KB, each laid out on its own and starting a new row, so no frame lays out more than a screenful of pieces however long a line is. The k-th break of a line falls near `start + k × 4 KB`, moved back to just after a space or punctuation if one is within 256 bytes, and never inside a character or grapheme, so pieces need no index and nothing to update when text changes. Without word wrap, a long line shows as rows of one piece each (classic Notepad also wrapped long lines when wrap was off, at 1024 columns); with it, each piece wraps at the window's edge. A row that is wider than the window is laid out only as far as it shows.
- **The scroll position is a byte anchor:** the piece at the top, a row in it and a fraction of a row. Scrolling lays out only the pieces it passes, keeping the cursor in view counts rows only between the top and the cursor, and the position stays put while a file loads. The scrollbar shows lines, and within a long line, bytes.
- **Accessibility:** the widget tree carries roles and labels from the start, so AccessKit can be wired up in v1.x without restructuring.

## 7. Text stack (`fude-text`)

- **Default font:** JetBrains Mono is bundled (OFL; two variable font files, about 380 KB, covering regular, bold and italic), so the first frame needs no font discovery and looks the same on every platform.
- **Font fallback:** characters JetBrains Mono lacks (CJK, emoji, most scripts) come from system fonts via `fontique` (fontconfig, CoreText, DirectWrite). The system font collection opens only when the first such character appears (about 20 ms, once; fontconfig is loaded at runtime, not linked), and the font chosen is cached per character. Candidates are the emoji font for emoji, then monospace fonts, then the system's fallbacks for the character's script (with the user's locale, so Han characters get Japanese or Chinese forms as appropriate). Combining marks and joiners stay in their base character's font.
- **Chips:** control characters and bytes that aren't valid UTF-8 are laid out as small labeled boxes (`NUL`, `ESC`, `FF`), so they're visible and take one cursor step. They travel through layout as private-use code points, so tab stops and hit testing just work.
- **Shaping:** `harfrust` (pure-Rust HarfBuzz port). Simple runs skip shaping and map characters straight to glyphs, which is correct for monospace fonts without ligatures, so most lines never touch the shaper.
- **Rasterization:** `swash` renders alpha masks (and color emoji) into an LRU glyph atlas keyed by font, glyph, size and ¼-pixel horizontal offset. Shaped lines are cached by content. swash draws bitmap (CBDT, sbix) and COLRv0 color emoji but not COLRv1; where the only color emoji font is COLRv1 (Fedora's Noto Color Emoji), fallback skips it for a monochrome emoji font.
- **Escape hatch:** rasterization sits behind a trait so a DirectWrite backend can be added on Windows if swash's output doesn't match ClearType expectations.

## 8. Buffer (`fude-buffer`)

A custom persistent B-tree rope. No existing crate gives us exact byte preservation, a path to file-backed leaves, and O(1) snapshots together.

```
              [bytes=212M, lines=3.1M]      ← every node caches a summary
             /            |           \
     [leaf "…"]     [leaf "fn ma…"]    [leaf "…"]      leaves hold ~1–4 KB of bytes
```

- **Shape:** leaves hold 1–4 KB of raw bytes (filled to 3.5 KB on load, leaving room to type); internal nodes have 8–16 children. All leaves are at the same depth.
- **Summaries:** parents store a summary per child, inline, so a descent reads one array per level: byte length, LF / CRLF / CR counts, and whether the bytes start with `\n` or end with `\r`. Joining two summaries pairs a `\r` at the end of one with a `\n` at the start of the next, so a CRLF split across leaves still counts as one break, and the status bar's line ending counts are O(1). Byte ↔ line conversion is O(log n), plus a SIMD scan of one leaf.
- **Shared, copy-on-write nodes.** A snapshot is a clone of the root pointer, O(1). This is what makes the threading model (§10) work: the highlighter, search and save each work from an immutable snapshot and never take a lock. A small edit inside one leaf mutates the path in place (copying only nodes a snapshot still shares); anything else splits the tree and concatenates the pieces, O(log n).
- **Undo** is a list of previous trees plus the caller's state (selections). Every state has a version number; undo returns to an earlier number, so "modified?" is "is the current version the saved one?", and work done on a snapshot is valid exactly when the versions match. Measured cost: 7.6 KB per undo step in a 500 MB file. Typing merges into the open step until the editor seals it (pause, word boundary, cursor move); undo, redo and saving seal it too.
- **Transactions:** a list of non-overlapping range replacements in the coordinates of the text before it, applied atomically as one step. `map_offset` moves other positions (other cursors, bookmarks) through it.
- **Positions are byte offsets.** Line and column are derived from them. Leaf boundaries may fall inside a UTF-8 sequence or a CRLF pair (the builder avoids it, but nothing relies on it).
- **Line endings:** LF, CRLF and CR are stored as-is. The dominant style is detected and used for new lines; conversion is a whole-document edit (one undo step).
- **Encodings:** see §9.
- **Future-proofing:** the leaf type is an enum. v1 only has owned-byte leaves; a file-span leaf for paged mode (§9) can be added later without changing the public API.
- **Testing:** property tests against a naive `Vec<u8>` model, run with tiny nodes (16-byte leaves, 2–4 children) so short inputs build deep trees; planted bugs in the CRLF seam and the size limits are caught. Integration tests use the real sizes. Three `cargo-fuzz` targets (edits, decoding round trips, undo history) run in CI for a short smoke test.

## 9. Opening and saving files

### Encodings

Detection order:

1. A byte order mark (UTF-8, UTF-16LE/BE).
2. UTF-16 without a BOM: mostly-ASCII UTF-16 has a zero in every other byte.
3. UTF-8, checked as the file streams in. A file that is mostly valid UTF-8 with some damage (at least 4 valid multi-byte sequences per invalid one, e.g. a log with stray bytes) stays UTF-8, as does anything with zero bytes near the start (binary).
4. Otherwise `chardetng` (Firefox's detector) guesses a legacy encoding from up to 1 MB of the text, starting at a line near where UTF-8 first failed and ending at a line break (a sample cut mid-character counts against the encoding it's in; that once made a large Shift_JIS file open as windows-1252).

UTF-8 is stored raw, so invalid bytes round-trip untouched (they will render as `<FF>`-style chips). Other encodings are decoded to UTF-8 with `encoding_rs` and encoded back on save; UTF-16 is encoded by fude, since `encoding_rs` only decodes it. While decoding a legacy encoding, fude re-encodes it and compares with the file. If it doesn't match (malformed bytes, or encodings like Shift_JIS with two byte sequences for one character), the document is flagged as not exact, so the UI can warn before a save changes bytes the user didn't touch. Saving a character the encoding can't represent is an error naming the character and its position; fude never writes replacement characters or HTML entities silently.

### v1: in memory, loaded in the background

1. A thread per file reads the first 1 MB. If there's more, the first 64 KB of it is decoded (cut at a line break) and shown right away as a read-only preview.
2. The thread reads the rest in 1 MB chunks, decodes if needed, and builds the rope bottom-up, reporting progress ("Loading 42%…" in the status bar). A legacy encoding is decided from the first megabyte when that's clear (it nearly always is), so the text is decoded as it streams in and the raw bytes are never all held; whether saving would reproduce the file exactly is checked the same way, by encoding each piece back.
3. When it finishes, the full rope replaces the preview (a new version, no undo step). The view keeps its place, since the preview is the start of the same text. Editing is enabled; typing before then says the file is still loading. Closing the tab stops the read.
4. Opening waits up to 20 ms for the load, so a file of up to about 100 MB appears whole, without a flash of preview.
5. Files over a configurable size (`large_file_mb`, default 1 GB) ask before opening. Reloading a file changed on disk and restoring a session (backups included) go through the same loader.

### Later: paged mode for multi-GB files

Add file-span leaves that point into the file on disk. Chunks are read on demand with `pread` into a bounded LRU cache, and a background `memchr` pass counts lines (several GB/s). Line numbers and the scrollbar are estimates until it finishes.

Why `pread` rather than `mmap`: on Unix, a mapped file crashes the process (SIGBUS) if another program truncates it. On Windows, mapping a file stops other programs from truncating or replacing it ("file in use"). `pread` costs one extra copy and avoids both. If the file changes on disk while paged, fude detects it (mtime and size) and asks what to do.

### Saving

Saves run on a snapshot in a worker, so the UI stays responsive. fude writes a temporary file in the same directory, flushes it to disk, then renames it over the original (`ReplaceFileW` on Windows, which keeps attributes and ACLs) and flushes the directory. It copies permissions and owner, follows symlinks (a dangling link creates its target), and refuses read-only files.

When a rename would change what the file is (it has other hard links, or its owner can't be kept), or the directory isn't writable, fude still writes and flushes the new contents to a temporary file first, then copies them over the original in place. If that copy fails, the error names the temporary file holding the new contents. Any other failure leaves the original untouched and removes the temporary file.

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

- **Multiple selections from day one.** Selections are a sorted, non-overlapping list of `{anchor, head, goal_x}`; a single cursor is a list of one. Cursors come from Ctrl+Click (Cmd+Click), Ctrl+Alt+↑/↓ (a cursor on the row above or below, at the same x), Ctrl+D (the word at the cursor, then each next occurrence: whole words if it began at a bare cursor), Ctrl+Shift+L (every occurrence) and Shift+Alt+I (one per line of a selection). A column (box) selection, by Alt+drag or Shift+Alt+arrows, is one selection per line between two x positions. Commands make at most 10,000 cursors. Pasting as many lines as there are cursors gives one each.
- **Edits are transactions:** a list of range replacements applied atomically. All selections are mapped through each edit.
- **Undo steps:** consecutive typing is one step per word (a new step starts when a word follows whitespace) or until a 1 s pause; consecutive backspaces or deletes are one step; anything else (paste, cut, Enter, word deletion) is its own step. Moving the cursor, undo, redo and saving end the current step.
- **Clipboard:** copy or cut with nothing selected takes the whole line, and pasting that inserts it above the cursor's line. Pasted line breaks become the document's style. Invalid bytes copy as U+FFFD.
- **Lines:** Alt+↑/↓ move the selected lines (each keeps its text, line breaks stay put), Ctrl+Shift+D duplicates, Ctrl+Shift+K deletes, Ctrl+J joins, Ctrl+Enter / Ctrl+Shift+Enter open a line below or above. Tab and Shift+Tab indent and outdent selected lines (Ctrl+] / Ctrl+[ too).
- **Indentation and brackets:** Enter keeps the line's indentation, one level deeper after `{`, `[` or `(` (and `:` in Python); between a pair, the closer goes on a line of its own. A closing bracket typed into a line's indentation lines up with its opener's line. The bracket next to the cursor and its partner are outlined, nesting by kind and skipping brackets the syntax colors say are in strings or comments; Ctrl+Shift+\ jumps between them. Matching looks at most 64 KB away, and is cached per cursor position and version.
- **Comments:** Ctrl+/ toggles line comments at the selection's common indentation (all commented → uncomment), Shift+Alt+A block comments, with markers per language (`#` for plain text, which covers most config files without a grammar). Languages with only block comments (HTML, CSS) wrap each line.
- **Motions follow the platform:** Ctrl+Right moves to the start of the next word on Windows and Linux, Option+Right to the end of the word on macOS; Cmd+arrows go to line and document ends on macOS. Home toggles between the first non-blank character and the line start. Enter keeps the line's indentation.
- **Everything is a named command** (`edit.duplicate_line`, `view.toggle_wrap`, …). Keybindings, menus and the command palette are front-ends that invoke commands with arguments.
- **Fault isolation:** event handling runs inside `catch_unwind`. A bug in a feature shows an error in the status bar instead of losing unsaved text.
- **Hot exit and crash recovery:** two seconds after anything changes, a worker writes the session (`session.toml`: tabs, files, cursors, scroll positions, formats) to the state directory, plus the text of each document with unsaved changes (`backups/`, only those whose version changed, with the same safe save as documents). With hot exit on (the default), quitting writes it right away and exits without asking; the next launch restores everything, unsaved changes included. After a crash, the last write (at most two seconds old) is restored the same way. Unsaved text over 32 MB is backed up at most every 30 seconds while typing (and always on quit), so a huge document doesn't keep the disk busy; a crash can lose that much more in it.
- **Unicode:** cursor movement steps over grapheme clusters; an invalid byte is one step.

## 12. Syntax highlighting (`fude-syntax`)

- **On a worker:** the UI sends a snapshot and the lines it shows (plus a 60-line margin) whenever its colors are missing or stale, draws plain text meanwhile, and redraws when the worker's reply wakes it (usually within a frame). Requests that pile up for one document collapse to the newest. The UI never lexes.
- **Line-state model** (like Sublime and Scintilla): the lexer state is saved every 64 lines. Highlighting starts from the nearest checkpoint above the viewport. An edit drops the checkpoints below the first changed line; undo and redo drop all of them (re-lexed in the background).
- **Engine:** `syntect` with its 75 bundled Sublime grammars, pure Rust via `fancy-regex`. Grammars load on the worker at first use (0.3 ms) and compile their regexes per language. Lexing runs at about 23,000 lines/s, so a screen is about 2.6 ms: fine on a worker, not on the UI thread. The grammar is chosen by file extension, then by the first line (shebangs). Scopes map to a dozen styles (comment, string, keyword, function, type…) that each theme colors.
- **Jumping deep into a large file:** lex from a guessed resync point ~1,000 lines above the viewport for colors that are immediately (usually) right; a background pass corrects them.
- **Size thresholds:** files up to 16 MB get a background pass that lexes the whole file into exact checkpoints; larger ones are highlighted around the viewport only (from a resync point when needed). Lines over 10 KB, or that aren't valid UTF-8, aren't highlighted.
- **Why not tree-sitter:** it parses the whole file up front, and each grammar adds hundreds of KB to several MB to the binary. The `Highlighter` trait leaves room to add it later for structure features like folding.

## 13. Search

- **Engine:** the `regex` crate (linear time, so no pathological pattern can hang the editor), run over the rope through `regex-cursor`. The engine needs chunks that never split a UTF-8 character, which rope leaves don't promise, so the cursor serves 64 KB windows whose edges move back onto character starts. Queries are literal unless regex is on, case-insensitive unless asked, with `^`/`$` at line breaks (CRLF-aware). Whole word adds `\b` at word-character edges. Replace expands `$1`-style groups for regex queries.
- **Incremental:** typing in the find bar jumps to the first match from where the search started, and the visible matches are highlighted every frame (searching only the visible range). Counting all matches ("3 of 120") runs on a worker over a snapshot; a new query cancels it at its next window, and edits recount, one count at a time. Replace All computes every replacement on a worker and applies them as one undo step if the document hasn't changed meanwhile.
- **Find in files (v1.x):** ripgrep's `ignore` and `grep-searcher` crates.

## 14. Extensibility

No plugin API in v1. Extensibility comes in three layers:

1. **Built-in tools** (`fude-tools`, pure functions over bytes, written out rather than pulled in as dependencies). Features people commonly install plugins for are bundled as commands: sort lines (A–Z, Z–A, ignoring case, by number), remove duplicates, reverse, trim trailing whitespace, indentation tabs ↔ spaces, UPPER / lower / Title case, Base64 / URL / HTML encode and decode, SHA-256 / SHA-1 / MD5 (copied to the clipboard), JSON format and minify (token by token: numbers, escapes and key order are kept; JSON Lines work), XML format, and line-ending conversion. They work on the selections, or the whole document if nothing is selected, as one undo step, and live in the command palette ("Tools: …"). Up to 256 KB they run at once; beyond that on a worker over a snapshot, applied only if the document hasn't changed meanwhile.
2. **Data, not code.** Syntax definitions (`.sublime-syntax`), color themes (`.tmTheme` or a fude theme file) and keymaps can be dropped into the config directory.
3. **External tools.** Any program can be run as a fude command, taking the selection or document on stdin and replacing it, inserting the output, or showing it in a panel. This covers formatters (`jq`, `prettier`, `black`, `rustfmt`), linters and personal scripts in any language:

   ```toml
   [[tool]]
   name    = "Format JSON (jq)"
   command = ["jq", "."]               # {file}, {dir}, {name}, {line} are filled in
   input   = "selection-or-document"   # or selection, document, nothing
   output  = "replace"                 # or insert, new-tab, message, nothing
   key     = "ctrl+alt+j"
   timeout = 30                        # seconds, then it's stopped
   ```

   Each runs on a worker thread, once per selection, in the file's folder; standard input is written on a thread of its own so big inputs can't deadlock. A failure shows the first line the program printed to standard error. A line break the program adds at the end is dropped if its input had none, so filters like `tr` and `jq` don't grow every selection by a line.

**Why no plugin API:** an API is a compatibility promise that can't be taken back; third-party code can't be held to the "no O(n) on the UI thread" rule, but fude gets blamed when it's slow; loading plugins costs startup time; and Rust has no stable ABI, so native plugins would mean a C ABI or an embedded runtime.

**Leaving the door open:** commands are the seam. If a real need appears, an embedded runtime (Lua via `mlua`, or WASM via `wasmi`) can call and register commands without restructuring the editor.

## 15. Platform support

| | Windows | macOS | Linux | BSD |
|---|---|---|---|---|
| Window and input | winit | winit | winit (X11 + Wayland, loaded at runtime); file drops on Wayland by fude | winit |
| Font lookup | DirectWrite | CoreText | fontconfig | fontconfig |
| File dialogs | IFileDialog | NSOpenPanel | XDG portal, with fallback | XDG portal, with fallback |
| Menus | in-window | native menu bar | in-window | in-window |
| Single instance | loopback TCP + token | loopback TCP + token (Finder open events: later) | loopback TCP + token | loopback TCP + token |

Launching from a terminal (stdin is one, on Unix) starts the window as a background process in its own process group, with its output in `fude.log` in the state directory, and returns the terminal at once, like `code`. `--wait` keeps it in the foreground (and in a new instance) for tools that wait for the editor to exit, such as git.

File drops: winit handles them on X11, Windows and macOS but not on Wayland, so fude listens itself there: it opens its own event queue on winit's Wayland connection, binds the seat's data device, accepts drags offering `text/uri-list` as copies, and reads the list on a helper thread. It's the one place fude shares a pointer with winit (`unsafe`, in one module).

Single instance: opening a file while fude is running sends it to the existing window as a new tab, and the new process exits before creating a window. The running instance listens on a loopback port and writes the port and a random token to a file in the state directory that only the user can read; a launch connects, sends the token and its files' absolute paths (as hex of their native bytes, so any path survives), and exits on the acknowledgement. If nothing answers, it becomes the instance. One mechanism, standard library only, on every platform. `--new-instance` opts out.

## 16. Testing and benchmarks

- **Unit and property tests** for buffer, editor and syntax crates; `cargo-fuzz` targets for the buffer.
- **Pixel snapshot tests** for the renderer and widgets.
- **Startup:** a `--bench-startup` flag exits right after the first frame is presented and prints timings; measured with `hyperfine`.
- **RAM:** peak and idle RSS measured for an empty document and for the large-file corpus.
- **Large-file corpus,** generated rather than committed (`cargo xtask corpus`, about 1 GB in `target/corpus`; `--small` makes a tenth of that): a 500 MB log, 100 MB of JSON on one line, a CSV of 50 KB lines, Japanese text in UTF-8 and Shift_JIS, UTF-16LE, UTF-8 with stray bytes, mixed line endings, and 20 MB of Rust. `cargo xtask bench-large` opens each with `fude --bench-file` (no window) and measures the first screen, loading, memory, and keystrokes, scrolling and paging at the start, middle and end, with and without word wrap, against the budgets in §3. CI runs it on the small corpus and fails a missed budget on Linux.
- **Micro benchmarks** for rope edits, line lookups, layout and search.
- **UI test:** `cargo xtask ui-test` drives the real window through X11 (XTest) under Xvfb and checks the files it saves: typing, mouse selection, clipboard, undo, the unsaved-changes prompt, find and replace, tabs, go to line, the command palette, word wrap, hot exit, single instance, recovery after the process is killed, and editing the end of a 40 MB file's 2 MB last line once it has loaded, and the power features: Ctrl+D, moving, duplicating and commenting lines, a column of cursors, sorting from the command palette, an external tool on its own shortcut, and auto-indent (26 checks). It uses its own settings and state directories. Runs in CI on every push.
- **Keystroke latency:** `fude --bench-typing` times an edit plus a full frame per key, without a window.
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
| Very long lines (minified files) | Done in phase 4: lines are laid out in pieces (§6); a keystroke in a 100 MB line costs 1.3 ms |

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
| `muda` | The macOS menu bar (macOS only) |
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
| 2026-11-08 | Undo keeps whole trees (sharing unchanged nodes) rather than inverse edits: O(1) undo and redo, trivially correct, "modified?" is a version comparison. Measured at 7.6 KB per step. |
| 2026-11-08 | Leaf boundaries may split CRLF pairs and UTF-8 sequences; summaries handle CRLF across the seam, so nothing has to maintain a boundary invariant through edits. |
| 2026-11-08 | Mostly-valid UTF-8 stays raw UTF-8 instead of going to the legacy-encoding detector, so a log with a few bad bytes doesn't turn into mojibake. |
| 2026-11-08 | Saving a character the encoding can't represent fails (encoding_rs would write HTML entities). |
| 2026-11-08 | When a rename would break hard links or lose the owner, save copies over the original in place, after the new contents are safely in a temporary file. |
| 2026-11-08 | Tests and fuzzing use tiny nodes (`small-nodes` feature, fuzz crate in its own workspace) to reach deep trees with short inputs. |
| 2026-11-08 | `fude-editor` takes horizontal geometry through a `Layout` trait, so editing is tested without fonts; the view implements it with the text system. |
| 2026-11-08 | `fontique` for font fallback, with fontconfig loaded at runtime (`fontconfig-dlopen`), so it isn't linked and costs nothing until a character needs it. |
| 2026-11-08 | `arboard` for the clipboard (data-control on Wayland, X11 otherwise) and `rfd` for file dialogs, both created on first use. |
| 2026-11-08 | "Save changes?" and similar questions are drawn in the window, not native message boxes: consistent look, keyboard-first, no zenity/kdialog dependency on Linux. |
| 2026-11-08 | The caret blinks for 10 s after input, then stays solid, so an idle window sets no timers. |
| 2026-11-08 | Syntax highlighting on a worker thread only; the UI draws plain text until colors arrive. syntect with `fancy-regex` (pure Rust) over Oniguruma, which is faster but adds a C dependency to every platform's build. |
| 2026-11-08 | Search windows of 64 KB moved onto character boundaries, so `regex-cursor` gets UTF-8-safe chunks (and Unicode word boundaries work) without the rope having to guarantee them. |
| 2026-11-08 | Single instance over loopback TCP with a token file, on every platform, instead of a named pipe, Unix socket and Apple events: one code path, standard library only. |
| 2026-11-08 | Hot exit on by default; the session is written two seconds after changes, so it doubles as crash recovery. |
| 2026-11-08 | Word wrap keeps the scroll position as a line plus a fraction of its rows, so scrolling only lays out the lines it passes; the scrollbar stays line-based. Continuation rows keep the line's indentation. (Replaced in phase 4 by a byte anchor.) |
| 2026-11-08 | Find bar docked at the bottom of the editor (like Sublime), so it never covers the text being searched. |
| 2026-11-08 | Launching from a terminal detaches (new process group, output to a log) and returns the terminal; `--wait` opts out, for git. |
| 2026-11-08 | Wayland file drops through the core data device protocol on winit's own connection, until winit supports them. |
| 2026-11-08 | Full redraws every frame for now: 0.6 ms at 1080p, 1.4 ms at 4K including the edit. Damage tracking waits until something needs it. |
| 2026-11-08 | Long lines are laid out in pieces of about 4 KB, breaking near spaces, each starting a new row: without word wrap a long line shows as wide rows, like classic Notepad, rather than being cut off (VS Code stops drawing at 10,000 characters) or laid out whole. Breaks depend only on nearby bytes, so there's no index to keep up to date. |
| 2026-11-08 | The scroll position is a byte anchor (piece, row, fraction): independent of what's above, exact at any line count (the old fractional line number lost precision past a few million lines), and stable while a file loads. |
| 2026-11-08 | A loading file is read-only until it's all in, rather than accepting edits to the preview and replaying them: simpler, and loading takes a fraction of a second up to hundreds of MB. |
| 2026-11-08 | A legacy encoding is decided from the first megabyte when it's clear, so the file decodes as it's read (with a streaming exactness check) instead of being held raw and decoded at the end: half the memory for a Shift_JIS file, and a bit faster. |
| 2026-11-08 | harfrust shaping plans are cached per font and segment properties (building one compiles the font's features: 11× slower CJK layout without the cache), and glyphs and advances for characters below U+0300 are cached per face. |
| 2026-11-09 | Application menus: drawn in the window on Windows, Linux and BSD, below which the tabs move down 28 px; the native menu bar on macOS through `muda` (Tauri's menu crate, macOS-only dependency) rather than hand-written AppKit code, which would be the workspace's second `unsafe` module. Shortcuts become key equivalents there, so a menu item and its key always do the same thing; a shortcut an external tool takes over is left off its built-in item. |
| 2026-11-09 | The About dialog is a drawn prompt like the others (with a button that copies the version for bug reports), on macOS too, rather than the standard About panel. |
| 2026-11-08 | Hot-exit backups of documents over 32 MB are written at most every 30 seconds while typing. |
| 2026-11-08 | Multiple-cursor and line commands follow VS Code and Sublime (Ctrl+D, Ctrl+Shift+L, Alt+↑/↓, Ctrl+Shift+D/K, Ctrl+/); column selection follows Notepad++ (Alt+drag, Shift+Alt+arrows). |
| 2026-11-08 | Shortcuts match a key's character without modifiers (winit's `key_without_modifiers`), so Ctrl+Shift+`\` matches whatever Shift turns the key into, and Option+letter shortcuts work on macOS. |
| 2026-11-08 | Bracket matching is by kind with nesting, bounded to 64 KB, and uses syntax colors (when current) to skip brackets in strings and comments, rather than a parser per language. |
| 2026-11-08 | Comment markers come from a table of the bundled grammars' names; plain text uses `#`. |
| 2026-11-08 | Built-in tools in their own crate, written out (hashes, JSON, XML) instead of adding dependencies: the binary is near its budget. The JSON formatter rewrites whitespace only, so it never changes a number or reorders keys. |
| 2026-11-08 | External tools take a command list (no shell), with placeholders, a timeout, and standard input fed from its own thread. |
| 2026-11-08 | Commands make at most 10,000 cursors, and drawing finds the ones on screen by binary search. |

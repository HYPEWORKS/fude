# fude 筆

**F**ast, **U**ncomplicated **D**ocument **E**ditor. *Fude* is Japanese for "brush".

A cross-platform text editor somewhere between Notepad and Notepad++: instant startup, instant response at any file size, low memory use. Windows, macOS and Linux, with BSD as a bonus.

> **Status: early.** Phases 0–4 are done: tabs, find and replace, syntax highlighting for 75 languages, word wrap, menus and a command palette, settings and light/dark themes, single instance, hot exit (quitting never asks; unsaved work comes back), large files (a 500 MB file shows in 23 ms and loads in the background; lines of any length edit as quickly as short ones), multiple cursors and column selection, line commands, comments, bracket matching, auto-indent, and text tools, built in or your own. Packaging and release are phase 6. See [ARCHITECTURE.md](ARCHITECTURE.md) for the design, roadmap and measurements.

## Building

Requires stable Rust (2024 edition).

```sh
cargo run --release                 # welcome view
cargo run --release -- some/file.txt
```

| | Keys (Cmd instead of Ctrl on macOS) |
|---|---|
| Move | arrows, Home / End, PageUp / PageDown, Ctrl+arrows by word, Ctrl+Home / End |
| Select | Shift + any move, drag, double-click a word, triple-click a line, click the gutter |
| Edit | Ctrl+Z / Ctrl+Y (or Ctrl+Shift+Z) undo and redo, Ctrl+X / C / V, Ctrl+A, Ctrl+/ comment, Shift+Alt+A block comment |
| Context menu | Right-click (or Control-click on macOS), Shift+F10 or the menu key; arrows to choose, Enter to run, Escape to dismiss |
| Menus | File, Edit, Selection, View, Go, Tools and Help (the menu bar on macOS). F10, or Alt and a menu's first letter (Alt+F for File), opens one; arrows move through the items and across the menus, Enter runs, Escape closes. Help → About shows the version |
| Cursors | Ctrl+Click add or remove, Ctrl+D next occurrence, Ctrl+Shift+L all occurrences, Ctrl+Alt+↑/↓ add above or below, Shift+Alt+I one per line, Alt+drag or Shift+Alt+arrows column selection, Escape back to one |
| Lines | Alt+↑/↓ move, Ctrl+Shift+D duplicate, Ctrl+Shift+K delete, Ctrl+J join, Ctrl+Enter / Ctrl+Shift+Enter new line below / above, Tab / Shift+Tab (or Ctrl+] / Ctrl+[) indent / outdent, Ctrl+Shift+\ matching bracket |
| Files | Ctrl+N new tab, Ctrl+O open, Ctrl+S save, Ctrl+Shift+S save as, Ctrl+W close tab, Ctrl+Q quit |
| Tabs | Ctrl+Tab / Ctrl+Shift+Tab, Ctrl+PageUp / PageDown, Ctrl+1…9; middle-click closes, drag reorders |
| Find | Ctrl+F find, Ctrl+H replace (⌘⌥F on macOS), F3 / Shift+F3 next and previous, Alt+C / W / R case, word, regex |
| Go | Ctrl+Shift+P or F1 command palette (with the text tools: sort, case, Base64, JSON, hashes, line endings…), Ctrl+G go to line (⌘L on macOS) |
| View | Alt+Z word wrap, Ctrl+= / Ctrl+- / Ctrl+0 zoom, Shift+wheel scrolls sideways |
| Settings | Ctrl+, opens `fude.toml` (font size, tab width, spaces, wrap, theme, hot exit, the size of file that asks before opening); saving it applies it |

Run from a terminal, fude opens its window in the background and gives the terminal back, like `code`; running `fude file` while fude is open hands the file to the open window. `--new-instance` opens another window. `--wait` stays in the terminal until the window closes, for use as git's editor: `git config --global core.editor "fude --wait"`. Files dropped on the window open in tabs. A large file shows its start at once and is editable when the rest has loaded (a fraction of a second for hundreds of MB); files over 1 GB ask first. Without word wrap, a line longer than 4 KB shows as several wide rows rather than one endless one. External programs can be added as tools in the settings (`[[tool]]`; the settings file has an example): they get the selection or document on standard input, and what they print replaces it, is inserted, opens in a new tab or shows in the status bar. Settings live in `~/.config/fude` (`%APPDATA%\fude` on Windows, `~/Library/Application Support/fude` on macOS); the session and unsaved changes in `~/.local/state/fude` (`%LOCALAPPDATA%\fude`, the same folder on macOS).

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets

cargo xtask bench-startup           # launch → first frame, RSS (opens and closes a window repeatedly)
fude --bench-render --size 3840x2160 --scale 2   # CPU renderer cost per full frame, no window
fude --bench-typing some/file.txt                 # edit + full frame per keystroke, no window
fude --screenshot out.png --size 2200x1440 --scale 2   # render one frame to PNG, no window
cargo xtask corpus                  # generate the large-file corpus (about 1 GB, in target/corpus; --small for a tenth)
cargo xtask bench-large             # open each corpus file without a window: first screen, loading, memory, typing
cargo run --release -p fude-buffer --example bench   # buffer on a generated 500 MB log (writes it to target/)
```

Fuzzing needs nightly Rust and [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz):

```sh
cd fuzz
cargo +nightly fuzz run -O rope_edits    # also: decode, history
```

On Linux, the startup benchmark and the UI test (which types, clicks and saves through the real window) run off-screen with Xvfb:

```sh
env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 1920x1080x24 -noreset" cargo xtask bench-startup
env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 1920x1080x24 -noreset" cargo xtask ui-test
```

## Layout

| Crate | Purpose |
|---|---|
| `crates/fude-buffer` | Rope, snapshots, undo, encodings, line endings, loading and saving |
| `crates/fude-editor` | Selections, cursor movement, editing commands and search, without UI |
| `crates/fude-tools` | Built-in text tools: sorting, case, encodings, JSON and XML formatting, hashes |
| `crates/fude-syntax` | Syntax highlighting on a worker thread |
| `crates/fude-ui` | Editor view, tabs, find bar, command palette, status bar, dialogs, themes |
| `crates/fude-render` | Draw list and CPU rasterizer |
| `crates/fude-text` | Fonts, fallback, shaping and glyph cache |
| `crates/fude-platform` | Windowing, input, clipboard, file dialogs (the only crate that knows about winit) |
| `fude` | The application |
| `xtask` | Development tasks (`cargo xtask`) |
| `fuzz` | Fuzz targets for the buffer (separate workspace) |

## License

fude is licensed under the [Apache License 2.0](LICENSE).

It bundles [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono), licensed under the SIL Open Font License 1.1 ([assets/fonts/jetbrains-mono/OFL.txt](assets/fonts/jetbrains-mono/OFL.txt)).

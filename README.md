# fude 筆

**F**ast, **U**ncomplicated **D**ocument **E**ditor. *Fude* is Japanese for "brush".

A cross-platform text editor somewhere between Notepad and Notepad++: instant startup, instant response at any file size, low memory use. Windows, macOS and Linux, with BSD as a bonus.

> **Status: early.** Phases 0–4 are done: tabs, find and replace, syntax highlighting for 75 languages, word wrap, a command palette, settings and light/dark themes, single instance, hot exit (quitting never asks; unsaved work comes back), and large files: a 500 MB file shows in 23 ms and loads in the background, and lines of any length (a 100 MB single-line JSON file) edit as quickly as short ones. Power features (multiple cursors, column selection, tools) are phase 5. See [ARCHITECTURE.md](ARCHITECTURE.md) for the design, roadmap and measurements.

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
| Edit | Ctrl+Z / Ctrl+Y (or Ctrl+Shift+Z) undo and redo, Ctrl+X / C / V, Ctrl+A |
| Files | Ctrl+N new tab, Ctrl+O open, Ctrl+S save, Ctrl+Shift+S save as, Ctrl+W close tab, Ctrl+Q quit |
| Tabs | Ctrl+Tab / Ctrl+Shift+Tab, Ctrl+PageUp / PageDown, Ctrl+1…9; middle-click closes, drag reorders |
| Find | Ctrl+F find, Ctrl+H replace (⌘⌥F on macOS), F3 / Shift+F3 next and previous, Alt+C / W / R case, word, regex |
| Go | Ctrl+Shift+P or F1 command palette, Ctrl+G go to line (⌘L on macOS) |
| View | Alt+Z word wrap, Ctrl+= / Ctrl+- / Ctrl+0 zoom, Shift+wheel scrolls sideways |
| Settings | Ctrl+, opens `fude.toml` (font size, tab width, spaces, wrap, theme, hot exit, the size of file that asks before opening); saving it applies it |

Run from a terminal, fude opens its window in the background and gives the terminal back, like `code`; running `fude file` while fude is open hands the file to the open window. `--new-instance` opens another window. `--wait` stays in the terminal until the window closes, for use as git's editor: `git config --global core.editor "fude --wait"`. Files dropped on the window open in tabs. A large file shows its start at once and is editable when the rest has loaded (a fraction of a second for hundreds of MB); files over 1 GB ask first. Without word wrap, a line longer than 4 KB shows as several wide rows rather than one endless one. Settings live in `~/.config/fude` (`%APPDATA%\fude` on Windows, `~/Library/Application Support/fude` on macOS); the session and unsaved changes in `~/.local/state/fude` (`%LOCALAPPDATA%\fude`, the same folder on macOS).

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

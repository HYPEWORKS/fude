# fude 筆

**F**ast, **U**ncomplicated **D**ocument **E**ditor. *Fude* is Japanese for "brush".

A cross-platform text editor somewhere between Notepad and Notepad++: instant startup, instant response at any file size, low memory use. Windows, macOS and Linux, with BSD as a bonus.

> **Status: early.** Phases 0–2 are done: fude edits one file at a time, like a fast Notepad: typing, selection with keyboard and mouse, undo, clipboard, IME, encodings, safe saving. Tabs, find/replace and syntax highlighting come in phase 3. See [ARCHITECTURE.md](ARCHITECTURE.md) for the design, roadmap and measurements.

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
| Files | Ctrl+N new, Ctrl+O open, Ctrl+S save, Ctrl+Shift+S save as, Ctrl+Q quit |
| View | Ctrl+= / Ctrl+- / Ctrl+0 zoom, Shift+wheel scrolls sideways |

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets

cargo xtask bench-startup           # launch → first frame, RSS (opens and closes a window repeatedly)
fude --bench-render --size 3840x2160 --scale 2   # CPU renderer cost per full frame, no window
fude --bench-typing some/file.txt                 # edit + full frame per keystroke, no window
fude --screenshot out.png --size 2200x1440 --scale 2   # render one frame to PNG, no window
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
| `crates/fude-editor` | Selections, cursor movement and editing commands, without UI |
| `crates/fude-ui` | Editor view, tab strip, status bar, dialogs, theme |
| `crates/fude-render` | Draw list and CPU rasterizer |
| `crates/fude-text` | Fonts, fallback, shaping and glyph cache |
| `crates/fude-platform` | Windowing, input, clipboard, file dialogs (the only crate that knows about winit) |
| `fude` | The application |
| `xtask` | Development tasks (`cargo xtask`) |
| `fuzz` | Fuzz targets for the buffer (separate workspace) |

## License

fude is licensed under the [Apache License 2.0](LICENSE).

It bundles [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono), licensed under the SIL Open Font License 1.1 ([assets/fonts/jetbrains-mono/OFL.txt](assets/fonts/jetbrains-mono/OFL.txt)).

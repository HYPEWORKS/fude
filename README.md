# fude 筆

**F**ast, **U**ncomplicated **D**ocument **E**ditor. *Fude* is Japanese for "brush".

A cross-platform text editor somewhere between Notepad and Notepad++: instant startup, instant response at any file size, low memory use. Windows, macOS and Linux, with BSD as a bonus.

> **Status: early.** Phases 0 and 1 are done: a window, CPU renderer and text stack, and the buffer (rope, snapshots, undo, encoding detection, safe saving). The window shows files read-only; editing arrives in phase 2. See [ARCHITECTURE.md](ARCHITECTURE.md) for the design, roadmap and measurements.

## Building

Requires stable Rust (2024 edition).

```sh
cargo run --release                 # welcome view
cargo run --release -- some/file.txt
```

For now the view is read-only: scroll with the wheel, trackpad or arrow keys, zoom with Ctrl+= / Ctrl+- / Ctrl+0 (Cmd on macOS), quit with Ctrl+Q.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets

cargo xtask bench-startup           # launch → first frame, RSS (opens and closes a window repeatedly)
fude --bench-render --size 3840x2160 --scale 2   # CPU renderer cost per full frame, no window
fude --screenshot out.png --size 2200x1440 --scale 2   # render one frame to PNG, no window
cargo run --release -p fude-buffer --example bench   # buffer on a generated 500 MB log (writes it to target/)
```

Fuzzing needs nightly Rust and [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz):

```sh
cd fuzz
cargo +nightly fuzz run -O rope_edits    # also: decode, history
```

On Linux you can run the startup benchmark off-screen with Xvfb:

```sh
env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 1920x1080x24 -noreset" cargo xtask bench-startup
```

## Layout

| Crate | Purpose |
|---|---|
| `crates/fude-buffer` | Rope, snapshots, undo, encodings, line endings, loading and saving |
| `crates/fude-render` | Draw list and CPU rasterizer |
| `crates/fude-text` | Fonts, shaping and glyph cache |
| `crates/fude-platform` | Windowing and input (the only crate that knows about winit) |
| `fude` | The application |
| `xtask` | Development tasks (`cargo xtask`) |
| `fuzz` | Fuzz targets for the buffer (separate workspace) |

## License

fude is licensed under the [Apache License 2.0](LICENSE).

It bundles [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono), licensed under the SIL Open Font License 1.1 ([assets/fonts/jetbrains-mono/OFL.txt](assets/fonts/jetbrains-mono/OFL.txt)).

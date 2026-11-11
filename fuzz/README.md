# fuzz

`cargo-fuzz` targets for [fude-buffer](../crates/fude-buffer/README.md). It's a separate workspace, so the buffer can be built with tiny nodes (the `small-nodes` feature: 16-byte leaves, 2–4 children), which lets short inputs build deep trees.

| Target | Checks |
|---|---|
| `rope_edits` | Random edits, splits, slices and snapshots, against a `Vec<u8>` |
| `decode` | Decoding arbitrary bytes never panics, and whenever the loader says a decode is exact, encoding it again gives back the input |
| `history` | Edits, undo, redo and saves, against a list of every state |

Needs nightly Rust and [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz):

```sh
cd fuzz
cargo +nightly fuzz run -O rope_edits    # or decode, history
```

CI runs each target for 45 seconds to catch regressions; long runs happen locally.

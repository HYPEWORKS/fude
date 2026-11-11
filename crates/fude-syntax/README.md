# fude-syntax

Syntax highlighting on a worker thread, so the UI never waits for it, and the comment markers of each language.

Dependencies: `fude-buffer`, `syntect`. Used by `fude-ui` and `fude`. Nothing outside this crate sees `syntect`.

## How it works

- **On a worker:** the UI sends a [`Request`] (a snapshot and the lines it shows, plus a 60-line margin) whenever its colors are missing or stale, draws plain text meanwhile, and redraws when the worker's [`Reply`] wakes it (usually within a frame). Requests that pile up for one document collapse to the newest. The UI never lexes.
- **Line-state model** (like Sublime and Scintilla): the lexer state is saved every 64 lines ([`CHECKPOINT`]). Highlighting starts from the nearest checkpoint above the viewport. An edit drops the checkpoints below the first changed line; undo and redo drop all of them (re-lexed in the background).
- **Engine:** `syntect` with its 75 bundled Sublime grammars, pure Rust via `fancy-regex`. Grammars load on the worker at first use (0.3 ms) and compile their regexes per language. The grammar is chosen by file extension, then by the first line (shebangs).
- **Styles:** scopes ("keyword.control.rust") map to a dozen styles (comment, string, keyword, function, type…) that each theme colors (`style.rs`).

Lexing runs at about 23,000 lines/s, so a screen is about 2.6 ms: fine on a worker, not on the UI thread.

## Large files

- **Jumping deep into a file:** lex from a guessed resync point about 1,000 lines above the viewport ([`RESYNC`]) for colors that are immediately (usually) right; a background pass corrects them.
- **Size thresholds:** files up to 16 MB ([`BACKGROUND_LIMIT`]) get a background pass that lexes the whole file into exact checkpoints; larger ones are highlighted around the viewport only (from a resync point when needed). Lines over 10 KB, or that aren't valid UTF-8, aren't highlighted.

## Comments

`comments.rs` maps each bundled grammar's name to its line and block comment markers, for toggling comments in [fude-editor](../fude-editor/README.md#commands). Plain text uses `#`, which covers most config files without a grammar. Languages with only block comments (HTML, CSS) have no line marker.

## Why not tree-sitter

It parses the whole file up front, and each grammar adds hundreds of KB to several MB to the binary, which is near its budget ([ARCHITECTURE.md](../../ARCHITECTURE.md#performance-budgets)). Since the rest of fude only sees `Request` and `Reply`, another engine could be added here later for structure features like folding.

## Modules

| File | |
|---|---|
| `lib.rs` | The worker: requests, replies, checkpoints, resync and the background pass |
| `style.rs` | From grammar scopes to the styles a theme colors |
| `comments.rs` | Comment markers per language |

## Decisions

| Date | Decision |
|---|---|
| 2026-11-08 | Syntax highlighting on a worker thread only; the UI draws plain text until colors arrive. syntect with `fancy-regex` (pure Rust) over Oniguruma, which is faster but adds a C dependency to every platform's build. |
| 2026-11-08 | Comment markers come from a table of the bundled grammars' names; plain text uses `#`. |

[`Request`]: src/lib.rs
[`Reply`]: src/lib.rs
[`CHECKPOINT`]: src/lib.rs
[`RESYNC`]: src/lib.rs
[`BACKGROUND_LIMIT`]: src/lib.rs

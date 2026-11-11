# fude-buffer

A document's text: a persistent rope of raw bytes with snapshots and undo, plus encoding detection, line endings, and safe loading and saving. No UI dependencies; everything here is tested headless.

**The file's bytes are the truth.** Opening and saving an unedited file gives back identical bytes, including invalid UTF-8 and mixed line endings.

Dependencies: `encoding_rs`, `chardetng`, `memchr`. Used by `fude-editor`, `fude-syntax`, `fude-ui` and `fude`.

## The rope

A custom persistent B-tree rope. No existing crate gives exact byte preservation, a path to file-backed leaves, and O(1) snapshots together.

```
              [bytes=212M, lines=3.1M]      ← every node caches a summary
             /            |           \
     [leaf "…"]     [leaf "fn ma…"]    [leaf "…"]      leaves hold ~1–4 KB of bytes
```

- **Shape:** leaves hold 1–4 KB of raw bytes (filled to 3.5 KB on load, leaving room to type); internal nodes have 8–16 children. All leaves are at the same depth. Only roots may be underfull, so concatenation only repairs the seam where an underfull root joins another tree (invariants in `rope/tree.rs`, checked by `Rope::check_invariants`).
- **Summaries:** parents store a summary per child, inline, so a descent reads one array per level: byte length, LF / CRLF / CR counts, and whether the bytes start with `\n` or end with `\r`. Joining two summaries pairs a `\r` at the end of one with a `\n` at the start of the next, so a CRLF split across leaves still counts as one break, and the status bar's line ending counts are O(1). Byte ↔ line conversion is O(log n), plus a SIMD scan of one leaf.
- **Shared, copy-on-write nodes.** A snapshot is a clone of the root pointer, O(1). This is what makes the threading model work ([ARCHITECTURE.md](../../ARCHITECTURE.md#threads-and-snapshots)): the highlighter, search and save each work from an immutable snapshot and never take a lock. A small edit inside one leaf mutates the path in place (copying only nodes a snapshot still shares); anything else splits the tree and concatenates the pieces, O(log n).
- **Positions are byte offsets.** Line and column are derived from them. Leaf boundaries may fall inside a UTF-8 sequence or a CRLF pair (the builder avoids it, but nothing relies on it).
- **Line endings:** LF, CRLF and CR are stored as-is. The dominant style is detected and used for new lines; conversion is a whole-document edit (one undo step).
- **Future-proofing:** the leaf type is an enum. v1 only has owned-byte leaves; a file-span leaf for [paged mode](#later-paged-mode-for-multi-gb-files) can be added later without changing the public API.

## Edits, undo and versions

- **Transactions:** an edit is a list of non-overlapping range replacements ([`Edit`]) in the coordinates of the text before it, applied atomically as one step. `map_offset` moves other positions (other cursors, bookmarks) through it.
- **Undo** is a list of previous trees plus the caller's state (selections). Every state has a [`Version`]; undo returns to an earlier number, so "modified?" is "is the current version the saved one?", and work done on a snapshot is valid exactly when the versions match.
- **Steps:** typing merges into the open step until the editor seals it (pause, word boundary, cursor move); undo, redo and saving seal it too. The grouping rules are the editor's ([fude-editor](../fude-editor/README.md#edits-and-undo-steps)).
- **Cost:** 7.6 KB per undo step in a 500 MB file. The history has no limit yet: a step that rewrites the whole document (sorting, a large Replace All, converting line endings) keeps the old text alive, so several on a very large file add up. A memory cap is *planned*.

## Encodings

Detection order:

1. A byte order mark (UTF-8, UTF-16LE/BE).
2. UTF-16 without a BOM: mostly-ASCII UTF-16 has a zero in every other byte.
3. UTF-8, checked as the file streams in. A file that is mostly valid UTF-8 with some damage (at least 4 valid multi-byte sequences per invalid one, e.g. a log with stray bytes) stays UTF-8, as does anything with zero bytes near the start (binary).
4. Otherwise `chardetng` (Firefox's detector) guesses a legacy encoding from up to 1 MB of the text, starting at a line near where UTF-8 first failed and ending at a line break (a sample cut mid-character counts against the encoding it's in; that once made a large Shift_JIS file open as windows-1252).

UTF-8 is stored raw, so invalid bytes round-trip untouched (the view shows them as `<FF>`-style chips). Other encodings are decoded to UTF-8 with `encoding_rs` and encoded back on save; UTF-16 is encoded by fude, since `encoding_rs` only decodes it.

While decoding a legacy encoding, the loader re-encodes it and compares with the file. If it doesn't match (malformed bytes, or encodings like Shift_JIS with two byte sequences for one character), [`Loaded::exact`] is false, so the UI can warn before a save changes bytes the user didn't touch. Saving a character the encoding can't represent is an error naming the character and its position; fude never writes replacement characters or HTML entities silently.

## Loading

[`load_observed`] loads on a worker thread and reports through a [`LoadObserver`]:

1. It reads the first 1 MB. If there's more, the first 64 KB of it is decoded (cut at a line break) and reported as a preview.
2. It reads the rest in 1 MB chunks, decodes if needed, and builds the rope bottom-up ([`RopeBuilder`]), reporting progress. A legacy encoding is decided from the first megabyte when that's clear (it nearly always is), so the text is decoded as it streams in and the raw bytes are never all held; whether saving would reproduce the file exactly is checked the same way, by encoding each piece back.
3. The observer can cancel at any point (closing the tab stops the read).

How the application shows the preview and swaps in the full text is in [fude](../../fude/README.md#opening-files).

### Later: paged mode for multi-GB files

Add file-span leaves that point into the file on disk. Chunks are read on demand with `pread` into a bounded LRU cache, and a background `memchr` pass counts lines (several GB/s). Line numbers and the scrollbar are estimates until it finishes.

Why `pread` rather than `mmap`: on Unix, a mapped file crashes the process (SIGBUS) if another program truncates it. On Windows, mapping a file stops other programs from truncating or replacing it ("file in use"). `pread` costs one extra copy and avoids both. If the file changes on disk while paged, fude detects it (mtime and size) and asks what to do.

## Saving

[`save`] never truncates the original before the new contents are safely on disk. The application calls it on a snapshot in a worker, so the UI stays responsive.

It writes a temporary file in the same directory, flushes it to disk, then renames it over the original (`ReplaceFileW` on Windows, which keeps attributes and ACLs) and flushes the directory. It copies permissions and owner, follows symlinks (a dangling link creates its target), and refuses read-only files.

When a rename would change what the file is (it has other hard links, or its owner can't be kept), or the directory isn't writable, it still writes and flushes the new contents to a temporary file first, then copies them over the original in place. If that copy fails, the error names the temporary file holding the new contents. Any other failure leaves the original untouched and removes the temporary file.

## Performance

`cargo run --release -p fude-buffer --example bench` generates a 500 MB log (5.8 M lines, once, in `target/bench-buffer`) and measures it:

| Operation | Time |
|---|---|
| Line → byte, random positions (mostly cache misses) | 1.1 µs |
| Byte → line, random positions | 0.8 µs |
| The 60 visible lines of a screen, found and copied out | 5.7 µs |
| Snapshot | 8 ns |
| Undo history | 7.6 KB per step |
| Save, including fsync | 180 ms |

Opening files through the whole application is measured by [`cargo xtask bench-large`](../../xtask/README.md#large-file-benchmark).

## Modules

| File | |
|---|---|
| `rope/` | The B-tree rope: nodes, summaries, tree algorithms (concat, split, in-place edits), the bottom-up builder, iterators |
| `buffer.rs` | [`Buffer`]: a rope with undo history, versions and saved-state tracking; [`Snapshot`], [`Edit`], `map_offset` |
| `load.rs` | Encoding detection, decoding, line ending detection; streaming loads with preview, progress and cancelling |
| `save.rs` | Encoding back and safe saving |
| `format.rs` | [`Format`]: how a document is stored on disk (encoding, BOM, line ending) |
| `line_ending.rs` | Line ending styles, counts and conversion |
| `utf8.rs` | UTF-8 over chunked bytes |

## Testing

- **Property tests** against a naive `Vec<u8>` model, run with tiny nodes (16-byte leaves, 2–4 children) so short inputs build deep trees. Unit tests always use them; the `small-nodes` feature turns them on for the fuzzer. Planted bugs in the CRLF seam and the size limits are caught.
- **Integration tests** (`tests/`) use the real sizes: encodings and round trips, loading on a worker, saving to a real file system.
- **Fuzzing:** three `cargo-fuzz` targets (edits, decoding round trips, undo history); see [fuzz](../../fuzz/README.md).

## Decisions

| Date | Decision |
|---|---|
| 2026-11-08 | Undo keeps whole trees (sharing unchanged nodes) rather than inverse edits: O(1) undo and redo, trivially correct, "modified?" is a version comparison. Measured at 7.6 KB per step. |
| 2026-11-08 | Leaf boundaries may split CRLF pairs and UTF-8 sequences; summaries handle CRLF across the seam, so nothing has to maintain a boundary invariant through edits. |
| 2026-11-08 | Mostly-valid UTF-8 stays raw UTF-8 instead of going to the legacy-encoding detector, so a log with a few bad bytes doesn't turn into mojibake. |
| 2026-11-08 | Saving a character the encoding can't represent fails (`encoding_rs` would write HTML entities). |
| 2026-11-08 | When a rename would break hard links or lose the owner, save copies over the original in place, after the new contents are safely in a temporary file. |
| 2026-11-08 | Tests and fuzzing use tiny nodes (`small-nodes` feature, fuzz crate in its own workspace) to reach deep trees with short inputs. |
| 2026-11-08 | A legacy encoding is decided from the first megabyte when it's clear, so the file decodes as it's read (with a streaming exactness check) instead of being held raw and decoded at the end: half the memory for a Shift_JIS file, and a bit faster. |

[`Buffer`]: src/buffer.rs
[`Snapshot`]: src/buffer.rs
[`Edit`]: src/buffer.rs
[`Version`]: src/buffer.rs
[`Format`]: src/format.rs
[`Loaded::exact`]: src/load.rs
[`LoadObserver`]: src/load.rs
[`load_observed`]: src/load.rs
[`RopeBuilder`]: src/rope/builder.rs
[`save`]: src/save.rs

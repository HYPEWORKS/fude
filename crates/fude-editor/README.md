# fude-editor

Editing, independent of any UI: selections, cursor movement, editing commands and search, on top of a [fude-buffer](../fude-buffer/README.md) buffer. The view supplies horizontal geometry through the [`Layout`] trait, so everything here is tested without fonts or a window ([`MonoLayout`] stands in for tests).

Every text input in fude is an [`Editor`]: documents, and also the find box, go to line and the command palette, which get selection, undo, the clipboard and IME for free.

Dependencies: `fude-buffer`, `unicode-segmentation`, `regex-cursor`, `regex-syntax`. Used by `fude-ui` and `fude`.

## Selections and cursors

- **Multiple selections from day one.** [`Selections`] is a sorted, non-overlapping list of `{anchor, head, goal_x}`; a single cursor is a list of one. All selections are mapped through each edit.
- **Adding cursors:** Ctrl+Click (Cmd+Click), Ctrl+Alt+↑/↓ (a cursor on the row above or below, at the same x), Ctrl+D (the word at the cursor, then each next occurrence: whole words if it began at a bare cursor), Ctrl+Shift+L (every occurrence) and Shift+Alt+I (one per line of a selection).
- **Column (box) selection,** by Alt+drag or Shift+Alt+arrows, is one selection per line between two x positions.
- **Limits:** commands make at most 10,000 cursors ([`MAX_CURSORS`]). Pasting as many lines as there are cursors gives one each.

## Edits and undo steps

- **Edits are transactions:** a list of range replacements applied atomically ([fude-buffer](../fude-buffer/README.md#edits-undo-and-versions)).
- **Undo steps:** consecutive typing is one step per word (a new step starts when a word follows whitespace) or until a 1 s pause; consecutive backspaces or deletes are one step; anything else (paste, cut, Enter, word deletion) is its own step. Moving the cursor, undo, redo and saving end the current step.
- **Clipboard:** copy or cut with nothing selected takes the whole line, and pasting that inserts it above the cursor's line. Pasted line breaks become the document's style. Invalid bytes copy as U+FFFD.

## Commands

- **Lines:** Alt+↑/↓ move the selected lines (each keeps its text, line breaks stay put), Ctrl+Shift+D duplicates, Ctrl+Shift+K deletes, Ctrl+J joins, Ctrl+Enter / Ctrl+Shift+Enter open a line below or above. Tab and Shift+Tab indent and outdent selected lines (Ctrl+] / Ctrl+[ too).
- **Indentation:** Enter keeps the line's indentation, one level deeper after `{`, `[` or `(` (and `:` in Python); between a pair, the closer goes on a line of its own. A closing bracket typed into a line's indentation lines up with its opener's line.
- **Brackets:** the bracket next to the cursor and its partner are found by kind with nesting, skipping brackets the syntax colors say are in strings or comments; Ctrl+Shift+\ jumps between them. Matching looks at most 64 KB away ([`BRACKET_LIMIT`]) and is cached per cursor position and version.
- **Comments:** Ctrl+/ toggles line comments at the selection's common indentation (all commented → uncomment), Shift+Alt+A block comments. Markers come from [fude-syntax](../fude-syntax/README.md#comments); languages with only block comments (HTML, CSS) wrap each line.

The shortcuts above are bound in [fude-ui](../fude-ui/README.md#commands-and-shortcuts); this crate only has the commands.

## Motion

- **Unicode:** valid UTF-8 moves by grapheme cluster (so `é` written as `e` + combining accent, an emoji with modifiers, or a CRLF pair is one step); each byte that isn't valid UTF-8 is a step of its own. Motion looks at a bounded window around the position, so the cost doesn't depend on line or file length (`text.rs`).
- **Platform conventions:** Ctrl+Right moves to the start of the next word on Windows and Linux, Option+Right to the end of the word on macOS ([`WordStop::PLATFORM`]); Cmd+arrows go to line and document ends on macOS. Home toggles between the first non-blank character and the line start.

## Search

[`Searcher`] runs the `regex` engine (linear time, so no pathological pattern can hang the editor) over the rope through `regex-cursor`.

- **UTF-8-safe chunks:** the engine needs chunks that never split a UTF-8 character, which rope leaves don't promise, so the cursor serves 64 KB windows whose edges move back onto character starts.
- **Queries** ([`Query`]) are literal unless regex is on, case-insensitive unless asked, with `^`/`$` at line breaks (CRLF-aware). Whole word adds `\b` at word-character edges. Replace expands `$1`-style groups for regex queries.
- **Stopping:** a search can be told to stop ([`Stop`]), so workers can cancel it at the next window. A search that was cut short never reports a match: the regex engine takes the cut for the end of the text.

How the application uses it (incremental find, counting and Replace All on workers, the 10 ms budget on the UI thread) is in [fude](../../fude/README.md#find-and-replace).

## Performance

Typing at 10,000 cursors in a 100 MB line takes 5.7 ms per key; at 5,750 cursors spread over a 500 MB log, 10.7 ms (about 2 µs per cursor for the edit; drawing looks only at the cursors on screen, found by binary search). Selecting every occurrence of a word scans the text at about 10 GB/s: 55 ms for the 500 MB log.

## Modules

| File | |
|---|---|
| `editor.rs` | [`Editor`]: a buffer, its selections, motions, typing, clipboard, undo grouping, brackets |
| `editor/cursors.rs` | Multiple cursors and column selection |
| `editor/lines.rs` | Commands on whole lines: indenting, duplicating, moving, deleting, joining, opening a line, comments |
| `selection.rs` | [`Selection`] and [`Selections`] |
| `text.rs` | Grapheme, word and line boundaries over raw bytes |
| `layout.rs` | The [`Layout`] trait the view implements, and [`MonoLayout`] for tests |
| `search.rs` | [`Query`] and [`Searcher`] over a rope |

## Testing

Unit and property tests in `tests.rs` (phases 2–3) and `tests_power.rs` (multiple cursors, line commands, indentation, comments, bracket matching). Documents are written with markers: `|` is a cursor (a selection's head), `[` before it or `]` after it marks the selection's anchor.

## Decisions

| Date | Decision |
|---|---|
| 2026-11-08 | Horizontal geometry comes through a `Layout` trait, so editing is tested without fonts; the view implements it with the text system. |
| 2026-11-08 | Search windows of 64 KB moved onto character boundaries, so `regex-cursor` gets UTF-8-safe chunks (and Unicode word boundaries work) without the rope having to guarantee them. |
| 2026-11-08 | Multiple-cursor and line commands follow VS Code and Sublime (Ctrl+D, Ctrl+Shift+L, Alt+↑/↓, Ctrl+Shift+D/K, Ctrl+/); column selection follows Notepad++ (Alt+drag, Shift+Alt+arrows). |
| 2026-11-08 | Bracket matching is by kind with nesting, bounded to 64 KB, and uses syntax colors (when current) to skip brackets in strings and comments, rather than a parser per language. |
| 2026-11-08 | Commands make at most 10,000 cursors, and drawing finds the ones on screen by binary search. |

[`Editor`]: src/editor.rs
[`MAX_CURSORS`]: src/editor.rs
[`BRACKET_LIMIT`]: src/editor.rs
[`Layout`]: src/layout.rs
[`MonoLayout`]: src/layout.rs
[`Selection`]: src/selection.rs
[`Selections`]: src/selection.rs
[`WordStop::PLATFORM`]: src/text.rs
[`Searcher`]: src/search.rs
[`Query`]: src/search.rs
[`Stop`]: src/search.rs

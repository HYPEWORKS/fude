# fude-tools

The built-in text tools: pure functions from text to text, so they can run on a worker over a snapshot and be tested without an editor. This is the first of fude's three layers of extensibility ([ARCHITECTURE.md](../../ARCHITECTURE.md#extensibility)).

No dependencies. Used by `fude-ui` (the menus and the command palette list them) and `fude` (which runs them).

## Tools

| Group | Tools |
|---|---|
| Lines | Sort (A–Z, Z–A, ignoring case, by number), remove duplicates, reverse, trim trailing whitespace, indentation tabs ↔ spaces |
| Case | UPPER, lower, Title |
| Encodings | Base64, URL (percent) and HTML entities, encode and decode |
| Hashes | SHA-256, SHA-1, MD5 (for checksums, not security), copied to the clipboard |
| Formatting | JSON format and minify, XML format |

Converting line endings sits beside them in the Tools menu and the palette, but it's a whole-document edit from [fude-buffer](../fude-buffer/README.md#the-rope) (`convert_line_endings`), not a tool here.

In the command palette they're "Tools: …". They work on the selections, or the whole document if nothing is selected, as one undo step. Up to 256 KB they run at once; beyond that on a worker over a snapshot, applied only if the document hasn't changed meanwhile ([fude](../../fude/README.md#text-tools)).

## Rules

- **Text is bytes:** the document's raw UTF-8, which may hold invalid bytes. Tools that need characters (case changes, formatters) leave invalid bytes as they are or refuse with a message saying why.
- **Line breaks are kept:** tools that reorder or drop lines join them with the text's first line break (so a CRLF file stays CRLF) and keep a final line break only if the text had one.
- **Formatters change only whitespace.** The JSON formatter works token by token without building a tree: numbers and string escapes are unchanged, keys stay in their order, duplicates are kept. The structure is checked as it goes and errors say where; several values one after another (JSON Lines) are formatted one by one. The XML formatter puts each element, comment and processing instruction on its own line, indented by depth (an element holding only text stays on one line), copies tags and text as they are, and reports mismatched and unclosed tags.
- **Written out, not pulled in.** Hashes, JSON and XML are a few hundred lines each, because the binary is near its size budget.

## Modules

| File | |
|---|---|
| `lib.rs` | The tool list and dispatch |
| `lines.rs` | Sorting, removing duplicates, reversing, trimming, indentation |
| `codec.rs` | Case changes; Base64, URL and HTML encoding |
| `hash.rs` | SHA-256, SHA-1, MD5 |
| `json.rs` | JSON format and minify |
| `xml.rs` | XML format |

## Decisions

| Date | Decision |
|---|---|
| 2026-11-08 | Built-in tools in their own crate, written out (hashes, JSON, XML) instead of adding dependencies: the binary is near its budget. The JSON formatter rewrites whitespace only, so it never changes a number or reorders keys. |

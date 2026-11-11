# fude-ui

The UI: the editor view, the window chrome (tabs, menus, status bar) and dialogs, drawn into a [fude-render](../fude-render/README.md) draw list with one theme. Everything is custom-drawn; the reasons are in [ARCHITECTURE.md](../../ARCHITECTURE.md#rendering).

Dependencies: every library crate (`fude-buffer`, `fude-editor`, `fude-render`, `fude-text`, `fude-platform`, `fude-syntax`, `fude-tools`); `accesskit` for the accessibility tree, `unicode-segmentation` for its characters. Used by `fude`.

## The editor view

[`EditorView`] draws the visible part of a document and turns keys, mouse and IME input into editor commands. Only visible text is ever laid out, so a frame costs the same at any file size.

- **Lines in pieces.** The view lays out *pieces*: a line up to 4 KB ([`PIECE`]) is one; a longer one (minified code, a CSV row, a single-line JSON file) is cut into pieces of about 4 KB, each laid out on its own and starting a new row, so no frame lays out more than a screenful of pieces however long a line is.
- **Breaks need no index.** The k-th break of a line falls near `start + k × 4 KB`, moved back to just after a space or punctuation if one is within 256 bytes, and never inside a character or grapheme, so pieces need nothing to update when text changes.
- **Wrap.** Without word wrap, a long line shows as rows of one piece each (classic Notepad also wrapped long lines when wrap was off, at 1024 columns); with it, each piece wraps at the window's edge, and continuation rows keep the line's indentation. A row wider than the window is laid out only as far as it shows.
- **The scroll position is a byte anchor** ([`ScrollPosition`]): the piece at the top, a row in it and a fraction of a row. Scrolling lays out only the pieces it passes, keeping the cursor in view counts rows only between the top and the cursor, and the position stays put while a file loads. The scrollbar shows lines, and within a long line, bytes.
- **Display text** (`display.rs`): control characters and invalid bytes become [chips](../fude-text/README.md#chips), and IME composition text is spliced in at the cursor. Offsets map both ways, so clicks and cursors land on the document's bytes.
- **The caret** blinks for 10 s after input, then stays solid, so an idle window sets no timers.
- **Highlights** come from the [fude-syntax](../fude-syntax/README.md) worker; until they arrive the view draws plain text.

## Widgets

- **One design system.** All UI draws with [`Theme`] color roles, never literal colors, and with shared sizes in logical pixels ([`Painter`]). Two themes: Sumi (墨, warm ink black) and Washi (和紙, warm paper white), with a vermilion accent; light and dark follow the OS by default. Every text color meets WCAG AA contrast (4.5:1) on each background it's drawn on, code on selections aside; a test checks each pair.
- **Every text input is an editor.** [`TextInput`] wraps a [fude-editor](../fude-editor/README.md) editor, so the find box, go to line and the command palette get IME, undo, selection, word motions and the clipboard like documents.
- **Tabs** ([`TabStrip`]): click to switch, middle-click or × to close, drag to reorder, double-click the empty strip for a new tab, the wheel when there are more tabs than fit.
- **Find bar** ([`FindBar`]), docked at the bottom of the editor so it never covers the text being searched. Enter / Shift+Enter (or F3 / Shift+F3) for next and previous, Alt+C / W / R for case, whole word and regex, Tab between the fields, Ctrl+Alt+Enter to replace all.
- **Command palette** ([`Palette`]): every command, fuzzy-filtered, with its shortcut; `:` and a line number (optionally `:column`) goes to a line.
- **Prompts** ([`Prompt`]): modal questions ("Save changes?", About) are drawn in the window rather than native message boxes, so they look the same everywhere and need no helper programs. Keyboard first: Enter picks the default, Escape cancels, a button's first letter picks it, Tab and the arrows move focus.
- **Context menu** ([`ContextMenu`]): a transient command menu at the pointer, kept inside the window.
- **Status bar** (`chrome.rs`): line and column, encoding, line endings, language, and messages.
- **Accessibility:** every widget describes itself for assistive technologies ([below](#accessibility)).

## Accessibility

Screen readers can't see a custom-drawn UI, so each widget describes itself as nodes of an [AccessKit](https://accesskit.dev) tree (`access.rs`): what it is, its name, its state (selected, toggled, expanded, disabled), its shortcut, and its bounds on screen. The app assembles the window's tree ([fude](../../fude/README.md)) and [fude-platform](../fude-platform/README.md#accessibility) hands it to the OS.

- **From the last frame.** Widgets describe what they drew, with the rectangles they drew it in, so the tree never disagrees with the screen. It's built after a frame, and only while an assistive technology is listening.
- **Ids say what a node is** ([`Part`]): a request from a screen reader ("click this", "select that text") names a node, and its id leads back to the widget, which handles it like the matching click or key.
- **Text as on screen.** A text field's text is *text runs*, one or more per row: each character's bytes, position and width (so magnifiers can follow the caret), and where words start, the same places Ctrl+Right stops. A character is a grapheme cluster; control characters read as their Unicode symbols (␀), as they show as chips; invalid bytes as U+FFFD; a line break is a character at the end of its row. Runs hold at most 255 characters (AccessKit counts word starts in bytes), so a longer row is several runs on one line, split at a word.
- **Documents describe the rows in view, not the file.** That's what keeps the tree's cost independent of file size (0.1 ms for a screenful of code; 5.5 ms for the worst case, a 1080p screen of 4 KB rows of minified JSON without wrap). The selection's ends are mapped to the runs; one that is off screen is clamped to the first or last row, and the view scrolls to the caret as it does when typing. Assistive technologies' "read to the end" stops at the bottom of the screen.
- **Focus** is the field with keyboard focus, or a dialog's focused button, or an open menu's selected item. In the command palette the field keeps focus and the selected command is its *active descendant*, so arrowing through commands reads each while typing still filters.
- **Read out as they change:** status bar messages (errors at once, others politely), the find bar's match count, and the palette's go-to-line hint. Loading progress isn't, as it changes too often.

## Commands and shortcuts

- **Every action is a named command** ([`Action`]: `line.duplicate`, `view.toggle_wrap`, …), with its shortcut in one table (`keymap.rs`). Keybindings, the menus, the context menu and the command palette all invoke commands from it, so a shortcut shown anywhere is the one that runs.
- **Shortcuts follow the platform:** Cmd instead of Ctrl on macOS, and a few platform-specific keys (Ctrl+Y redo and Ctrl+H replace elsewhere, ⌘⌥F and ⌘L on macOS). They match a key's character without modifiers, so Ctrl+Shift+`\` matches whatever Shift turns the key into, and Option+letter shortcuts work on macOS.
- **Menus** ([`menus`]): one description of File, Edit, Selection, View, Go, Tools and Help drives both the menu bar drawn in the window ([`MenuBar`], on Windows, Linux and BSD) and the macOS menu bar ([fude-platform](../fude-platform/README.md#native-pieces)). The drawn bar's menus are context menus; F10, or Alt and a menu's first letter, opens one, the arrows move through the items and across the menus, Enter runs, Escape closes. The tabs sit below the bar.

## Modules

| File | |
|---|---|
| `view.rs` | [`EditorView`]: drawing a document, input, scrolling, the caret |
| `pieces.rs` | Lines in pieces |
| `display.rs` | A line as laid out: chips and IME text, offsets both ways |
| `input.rs` | [`TextInput`]: a single-line field built on the editor |
| `find.rs` | The find and replace bar |
| `palette.rs`, `fuzzy.rs` | The command palette and its fuzzy matching |
| `tabs.rs` | The tab strip |
| `menu_bar.rs` | What's in the menus, and the menu bar drawn in the window |
| `context_menu.rs` | The context menu (and the drawn bar's menus) |
| `prompt.rs` | Modal questions |
| `chrome.rs` | The status bar |
| `keymap.rs` | Commands, their names and shortcuts |
| `access.rs` | Accessibility: node ids, text as text runs, the tree being built |
| `theme.rs` | Colors by role; the two themes |
| `painter.rs` | Drawing helpers shared by every widget |

## Decisions

| Date | Decision |
|---|---|
| 2026-11-08 | "Save changes?" and similar questions are drawn in the window, not native message boxes: consistent look, keyboard-first, no zenity/kdialog dependency on Linux. |
| 2026-11-08 | The caret blinks for 10 s after input, then stays solid, so an idle window sets no timers. |
| 2026-11-08 | Word wrap keeps the scroll position as a line plus a fraction of its rows, so scrolling only lays out the lines it passes; the scrollbar stays line-based. Continuation rows keep the line's indentation. (Replaced in phase 4 by a byte anchor.) |
| 2026-11-08 | Find bar docked at the bottom of the editor (like Sublime), so it never covers the text being searched. |
| 2026-11-08 | Long lines are laid out in pieces of about 4 KB, breaking near spaces, each starting a new row: without word wrap a long line shows as wide rows, like classic Notepad, rather than being cut off (VS Code stops drawing at 10,000 characters) or laid out whole. Breaks depend only on nearby bytes, so there's no index to keep up to date. |
| 2026-11-08 | The scroll position is a byte anchor (piece, row, fraction): independent of what's above, exact at any line count (the old fractional line number lost precision past a few million lines), and stable while a file loads. |
| 2026-11-08 | Shortcuts match a key's character without modifiers (winit's `key_without_modifiers`), so Ctrl+Shift+`\` matches whatever Shift turns the key into, and Option+letter shortcuts work on macOS. |
| 2026-11-11 | Text colors raised to WCAG AA contrast, the least each needed: line numbers, comments, muted chrome text (Sumi's was 3.3:1 on a selected menu item), headings, and Sumi's primary button text. They had been 2.4 to 4.5:1. |
| 2026-11-11 | Accessibility describes documents as the rows on screen, not the whole text: it keeps the tree's cost independent of file size, as drawing is. Screen readers' "say all" then stops at the bottom of the screen; VS Code exposes a similar window of text for the same reason. |
| 2026-11-11 | Widgets describe themselves from their last frame, in a pass after drawing (only while an assistive technology listens) rather than while drawing: drawing stays as fast as it was, and a document's rows are laid out again only then. |
| 2026-11-09 | The About dialog is a drawn prompt like the others (with a button that copies the version for bug reports), on macOS too, rather than the standard About panel. |

[`EditorView`]: src/view.rs
[`ScrollPosition`]: src/view.rs
[`PIECE`]: src/pieces.rs
[`Theme`]: src/theme.rs
[`Painter`]: src/painter.rs
[`Part`]: src/access.rs
[`TextInput`]: src/input.rs
[`TabStrip`]: src/tabs.rs
[`FindBar`]: src/find.rs
[`Palette`]: src/palette.rs
[`Prompt`]: src/prompt.rs
[`ContextMenu`]: src/context_menu.rs
[`Action`]: src/keymap.rs
[`menus`]: src/menu_bar.rs
[`MenuBar`]: src/menu_bar.rs

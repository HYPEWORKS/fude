# fude (the application)

The binary: it wires the library crates together. It owns the open documents and their tabs, routes input to the focused widget, runs the work that mustn't block the window (loading, saving, find, tools, hot exit) on workers, and handles settings, the command line and the single instance.

Dependencies: every library crate, and `toml` for settings.

## Command line

```
fude [OPTIONS] [FILE]...
  -w, --wait            stay in the terminal until the window closes, in a window of its own
  -n, --new-instance    open a new window instead of handing the files to a running fude
```

- **Launching from a terminal** (stdin is one, on Unix) starts the window as a background process in its own process group, with its output in `fude.log` in the state directory, and returns the terminal at once, like `code`.
- **`--wait`** keeps it in the foreground (and in a new instance) for tools that wait for the editor to exit, such as git: `git config --global core.editor "fude --wait"`.
- **Development flags** run without a window: `--bench-startup`, `--bench-render`, `--bench-typing`, `--bench-file <PATH>` (with `--check`), `--screenshot <PATH>`, sized by `--size WxH` and `--scale`. See [xtask](../xtask/README.md) for the benchmarks built on them, and `fude --help` for the full list.

## Opening files

Loading runs on a thread per file (`loading.rs`), through [fude-buffer's loader](../crates/fude-buffer/README.md#loading):

1. If the file is over 1 MB, its first 64 KB shows at once as a read-only preview, with "Loading 42%…" in the status bar.
2. When the load finishes, the full text replaces the preview (a new version, no undo step). The view keeps its place, since the preview is the start of the same text. Editing is enabled; typing before then says the file is still loading. Closing the tab stops the read.
3. Opening waits up to 20 ms for the load, so a file of up to about 100 MB appears whole, without a flash of preview.
4. Files over `large_file_mb` (default 1 GB) ask before opening. Reloading a file changed on disk and restoring a session (backups included) go through the same loader.

Saving runs [`fude_buffer::save`](../crates/fude-buffer/README.md#saving) on a snapshot in a worker.

**External changes:** fude checks mtime and size when the window gains focus. A clean document reloads automatically; a modified one prompts. A document restored with unsaved changes keeps the stamp of the file as fude last saw it (it's in the session), so a file that changed while fude wasn't running prompts too instead of being overwritten by the next save. Watching files while the window has focus, and making the automatic reload configurable, are *planned*.

Files dropped on the window open in tabs.

## Find and replace

`find.rs` runs [fude-editor's search](../crates/fude-editor/README.md#search) for the find bar:

- **Incremental:** typing in the find bar jumps to the first match from where the search started, and the visible matches are highlighted every frame (searching only the visible range).
- **One match within 10 ms, or on a worker.** Looking for one match (typing in the find bar, Find Next and Previous, Ctrl+D, Ctrl+Shift+L) gets 10 ms on the UI thread, which nearly every search finishes within. One that needs longer (a regex with no match in a 500 MB file) carries on on a worker over a snapshot, and its match is selected when found, if the document (and for occurrences, the selection) hasn't changed meanwhile. A newer search cancels it.
- **Counting** all matches ("3 of 120") runs on a worker over a snapshot; a new query cancels it at its next window, and edits recount, one count at a time.
- **Replace All** computes every replacement on a worker and applies them as one undo step if the document hasn't changed meanwhile: up to 10,000 matches as edits (which share the rest of the text with the undo step), beyond that as a new text built on the worker, so applying it doesn't freeze the window.
- **Find in files** *(v1.x)*: ripgrep's `ignore` and `grep-searcher` crates.

## Text tools

`tools.rs` runs the [built-in tools](../crates/fude-tools/README.md) and external programs on the selections, or the whole document if nothing is selected, as one undo step. A built-in tool on up to 256 KB runs at once; bigger inputs, and every external program, run on a worker over a snapshot, and the result applies only if the document hasn't changed meanwhile.

**External tools** are declared in the settings and appear in the command palette as "Tool: <name>":

```toml
[[tool]]
name    = "Format JSON (jq)"
command = ["jq", "."]               # {file}, {dir}, {name}, {line} are filled in
input   = "selection-or-document"   # or selection, document, nothing
output  = "replace"                 # or insert, new-tab, message, nothing
key     = "ctrl+alt+j"              # optional; takes the shortcut over from a built-in
timeout = 30                        # seconds, then it's stopped
```

The command is a list, not a shell line. Each run is once per selection, in the file's folder; standard input is written on a thread of its own so big inputs can't deadlock. A failure shows the first line the program printed to standard error. A line break the program adds at the end is dropped if its input had none, so filters like `tr` and `jq` don't grow every selection by a line.

## Settings

`fude.toml` (`config.rs`) opens in fude like any other document (Ctrl+,) and applies when saved. It has the font size, tab width, spaces or tabs, word wrap, theme (`auto` follows the OS), hot exit, `large_file_mb`, and external tools; a missing file means defaults, and a broken one gives defaults for what couldn't be read plus messages saying what's wrong.

| | Settings | Session and backups |
|---|---|---|
| Linux, BSD | `$XDG_CONFIG_HOME/fude` (`~/.config/fude`) | `$XDG_STATE_HOME/fude` (`~/.local/state/fude`) |
| macOS | `~/Library/Application Support/fude` | the same folder |
| Windows | `%APPDATA%\fude` | `%LOCALAPPDATA%\fude` |

## Hot exit and crash recovery

Two seconds after anything changes, a worker writes the session (`session.toml`: tabs, files, cursors, scroll positions, formats) to the state directory, plus the text of each document with unsaved changes (`backups/`, only those whose version changed). Both are written to a temporary file and renamed into place, so they're never half-written; unlike saving a document they aren't fsynced, since a killed process loses nothing the OS already has.

With hot exit on (the default), quitting writes the session right away and exits without asking; the next launch restores everything, unsaved changes included. After a crash, the last write (at most two seconds old) is restored the same way. Unsaved text over 32 MB is backed up at most every 30 seconds while typing (and always on quit), so a huge document doesn't keep the disk busy; a crash can lose that much more in it.

- **A document counts as backed up only once its write succeeded.** A failed write (full disk, unwritable state directory) says so in the status bar and is tried again 30 seconds later; quitting then asks about that document like quitting without hot exit does.
- **One writer at a time.** Quitting waits for a background write before doing its own, so an older write never has the last word.
- **Backups outlive the session file.** A `session.toml` that can't be read (cut short by a power cut) is set aside as `session.unreadable.toml`, and every backup no tab names comes back as an untitled document. Backups are only deleted after a session that no longer names them has been written.
- **Only the single instance keeps a session.** A window opened with `--new-instance` or `--wait` has none (two processes writing one session would delete each other's backups), so it asks about unsaved changes when it closes.

The bookkeeping is in `hot_exit.rs` (with tests); the file formats in `session.rs`.

## Single instance

Opening a file while fude is running sends it to the existing window as a new tab, and the new process exits before creating a window (`instance.rs`).

- The running instance listens on a loopback port and writes the port and a random token to a file in the state directory that only the user can read. A launch connects, sends the token and its files' absolute paths (as hex of their native bytes, so any path survives), and exits on the acknowledgement.
- If nothing answers, it becomes the instance by taking a lock on `instance.lock` in the state directory, held until the process ends (the OS releases it however that happens, so it can't go stale). Two launches at the same moment can't both get it: the other waits up to 3 seconds for the holder to start listening and hands its files over, or failing that opens a window of its own without the session.
- One mechanism, standard library only, on every platform (Finder open events on macOS come later). `--new-instance` opts out.

## Fault isolation

Event handling, drawing and building the accessibility tree run inside `catch_unwind` (`main.rs`). A bug in a command shows an error in the status bar, and a bug in drawing costs a frame, instead of losing unsaved text. The release profile keeps `panic = "unwind"` for this.

## Modules

| File | |
|---|---|
| `main.rs` | Startup, the window, fault isolation, the modes without a window |
| `args.rs` | The command line |
| `app.rs` | The application: documents in tabs, focus, input routing, drawing |
| `doc.rs` | An open document: its editor view and where it lives on disk |
| `files.rs` | Opening, saving and closing documents, and the questions that go with them; external changes |
| `loading.rs` | Loading files on worker threads |
| `find.rs` | Find and replace: the bar's actions, and the searches on workers |
| `tools.rs` | Running built-in and external tools |
| `config.rs` | Settings |
| `session.rs` | The session and backup files |
| `hot_exit.rs` | When to write the session, and what counts as backed up |
| `instance.rs` | Single instance |
| `bench_file.rs` | `--bench-file`: the large-file benchmark for one file |
| `welcome.rs` | Demo text for the modes without a window |

## Decisions

| Date | Decision |
|---|---|
| 2026-11-08 | Single instance over loopback TCP with a token file, on every platform, instead of a named pipe, Unix socket and Apple events: one code path, standard library only. |
| 2026-11-08 | Hot exit on by default; the session is written two seconds after changes, so it doubles as crash recovery. |
| 2026-11-08 | Launching from a terminal detaches (new process group, output to a log) and returns the terminal; `--wait` opts out, for git. |
| 2026-11-08 | A loading file is read-only until it's all in, rather than accepting edits to the preview and replaying them: simpler, and loading takes a fraction of a second up to hundreds of MB. |
| 2026-11-08 | Hot-exit backups of documents over 32 MB are written at most every 30 seconds while typing. |
| 2026-11-08 | External tools take a command list (no shell), with placeholders, a timeout, and standard input fed from its own thread. |
| 2026-11-10 | Hot exit hardened after a review: backups are recorded only once written, quitting asks when one fails and waits for a background write, backups survive an unreadable session file, and only the single instance keeps a session. The bookkeeping moved out of the application into `hot_exit.rs`, with tests. |
| 2026-11-10 | Replace All past 10,000 matches builds the new text on the worker rather than editing match by match on the UI thread (2 µs each: 2 s for a million). |
| 2026-11-10 | Searches for a match get 10 ms on the UI thread, then move to a worker, rather than always running on one: the common case stays synchronous and instant, and no search can freeze the window. |
| 2026-11-10 | The single instance is whoever holds an OS file lock (`File::try_lock`, standard library), not whoever wrote the port file last: simultaneous launches can no longer both become the instance and share a session. |

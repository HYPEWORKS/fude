# fude-platform

Windowing, input and OS integration. This is the only crate that knows about `winit`, `softbuffer`, the clipboard and file dialogs. Everything above it sees fude's own [`Event`] types and draws into a [fude-render](../fude-render/README.md) canvas, so the windowing layer can be replaced with per-OS code later without touching the editor.

Dependencies: `fude-render`, `winit`, `softbuffer`, `arboard`, `rfd`; `wayland-client` on Linux and the BSDs; `muda` on macOS. Used by `fude-ui` and `fude`.

## The event loop

- **Event-driven, zero idle CPU.** The application implements the [`App`] trait. The loop sleeps until input arrives, a timer the app set comes due ([`Ctx::set_timer`]), or another thread calls [`Waker::wake`] (how workers hand back results). Nothing polls.
- **winit** handles windows, input, IME and DPI on Windows, macOS, X11, Wayland and the BSDs (X11 and Wayland libraries are loaded at runtime). Its events are translated into fude's ([`KeyInput`], [`Modifiers`] in `input.rs`), so shortcuts match a key's character without modifiers (`key_without_modifiers`) the same way everywhere.
- **The OS theme** (light or dark) is reported at startup and when it changes ([`OsTheme`]).

## Native pieces

- **Clipboard:** `arboard` (data-control on Wayland, X11 otherwise), created on first use.
- **File dialogs:** `rfd`, using the XDG portal on Linux, so no GTK; created on first use. Everything else that asks a question ("Save changes?") is drawn in the window by [fude-ui](../fude-ui/README.md#widgets).
- **The macOS menu bar** (`mac_menu.rs`, through `muda`). The app describes its menus once ([`MenuBarSpec`], the same menus fude-ui draws on other platforms); this adds what every Mac app has: About, Settings and Quit move to the application menu, with Services and Hide, plus the Window menu. Chosen items arrive as [`Event::Menu`]. Quit is fude's own command, not `terminate:`, so quitting keeps hot exit. Shortcuts become key equivalents, so the menu bar handles them before the window sees the key; a disabled item lets its key through to the window, where the app's own keymap handles it the same way. Items are enabled from the same state as the drawn menus, updated after each event.
- **Memory usage** ([`memory_usage`]): current and peak RSS, for the benchmarks. Linux only for now (via `/proc`); Windows and macOS need small FFI calls.

## File drops on Wayland

winit handles file drops on X11, Windows and macOS but not on Wayland, so `wayland_dnd.rs` listens itself: it opens its own event queue on winit's Wayland connection, binds the seat's data device (core protocol), accepts drags offering `text/uri-list` (what file managers send) as copies, and reads the list on a helper thread; the paths come back through a channel, waking the event loop. It's the one place fude shares a pointer with winit (an `unsafe` call, `WaylandDrop::new`).

## Startup

Launch to first frame is about 10 ms on Linux (X11). About 8 ms of it is winit creating the event loop, more than half of that libX11 and xkbcommon each parsing the X11 Compose table. fude's own work (fonts, layout, rasterizing the first frame) is about 2 ms. Deferring the Compose table until the first key press is a possible later optimization. Measured with [`cargo xtask bench-startup`](../../xtask/README.md#startup-benchmark).

## Modules

| File | |
|---|---|
| `lib.rs` | The event loop, [`App`], [`Event`], [`Ctx`] (window, clipboard, dialogs, timers), [`Waker`], [`MenuBarSpec`] |
| `input.rs` | Keys and modifiers, translated from winit |
| `mac_menu.rs` | The macOS menu bar (macOS only) |
| `wayland_dnd.rs` | File drops on Wayland (Linux and the BSDs) |
| `memory.rs` | Resident memory of the process |

## Decisions

| Date | Decision |
|---|---|
| 2026-11-07 | winit 0.30 (stable) rather than the 0.31 beta; contained in this crate, so upgrading later is local. |
| 2026-11-08 | `arboard` for the clipboard (data-control on Wayland, X11 otherwise) and `rfd` for file dialogs, both created on first use. |
| 2026-11-08 | Wayland file drops through the core data device protocol on winit's own connection, until winit supports them. |

[`Event`]: src/lib.rs
[`Event::Menu`]: src/lib.rs
[`App`]: src/lib.rs
[`Ctx`]: src/lib.rs
[`Ctx::set_timer`]: src/lib.rs
[`Waker`]: src/lib.rs
[`Waker::wake`]: src/lib.rs
[`OsTheme`]: src/lib.rs
[`KeyInput`]: src/input.rs
[`Modifiers`]: src/input.rs
[`MenuBarSpec`]: src/lib.rs
[`memory_usage`]: src/memory.rs

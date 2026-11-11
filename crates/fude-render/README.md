# fude-render

The draw list and the CPU rasterizer. The UI describes a frame as a flat [`DrawList`] of rectangles and images (glyph masks, color glyphs); [`rasterize`] turns it into pixels on a [`Canvas`].

No dependencies. Used by `fude-text`, `fude-platform`, `fude-ui` and `fude`.

## Design

- **CPU rendering.** Frames are drawn into a CPU framebuffer that `fude-platform` presents with `softbuffer`. GPU device setup often costs 100 ms or more and tens of MB of driver memory, which by itself breaks the startup and memory budgets ([ARCHITECTURE.md](../../ARCHITECTURE.md#rendering)). CPU output is also deterministic, so the renderer can have pixel snapshot tests *(planned)*.
- **The draw list is the seam.** It's a flat list of rectangles and glyph instances from an [`ImageStore`], which is also what a GPU backend would consume. If CPU rendering can't keep up (most likely on 5K Retina at 120 Hz), a Metal or Direct3D 11 backend can be added behind the same list without touching the UI; it would mirror the image store into an atlas texture keyed by the same ids.
- **Full redraws, for now.** Every frame redraws everything (0.6 ms at 1080p). `rasterize` already takes a damage rectangle; redrawing only damaged regions and moving pixels on scroll are *planned* for when something needs them.
- **Pixels** are `0x00RRGGBB`, the format `softbuffer` presents on every platform. A [`Canvas`] borrows the platform's buffer, so the rasterizer draws straight into what's presented, with no intermediate copy.
- **Blending** is integer-only: red and blue are blended together in one `u32` (two 16-bit lanes), green separately, so a pixel costs two multiplies per input instead of three, and results match `round((src * a + dst * (255 - a)) / 255)` exactly.
- **PNG:** a minimal encoder (stored deflate blocks, no dependencies, byte-for-byte deterministic) for `fude --screenshot` and snapshot tests.

## Performance

The phase 0 go/no-go for CPU rendering: full-frame redraws with no damage tracking, single thread, `fude --bench-render`:

| Resolution | Median full redraw | First frame (cold glyph cache) |
|---|---|---|
| 1920×1080 @1× | 0.54 ms | 3.2 ms |
| 2560×1440 @2× | 0.78 ms | 4.8 ms |
| 3840×2160 @2× | 1.29 ms | 9.2 ms |
| 5120×2880 @2× | 2.43 ms | 15.3 ms |

Even the 5K worst case fits a 120 Hz frame (8.3 ms) several times over, so CPU rendering is a go. Presenting the frame (copying to the compositor) isn't included and needs measuring per platform, macOS especially.

## Modules

| File | |
|---|---|
| `lib.rs` | [`rasterize`] |
| `list.rs` | [`DrawList`] and its commands, in painter's order |
| `canvas.rs` | [`Canvas`]: a borrowed pixel buffer |
| `image.rs` | [`ImageStore`]: the bitmaps a draw list refers to |
| `blend.rs` | Integer alpha blending |
| `geom.rs` | Rectangles in physical pixels |
| `png.rs` | The PNG encoder |

## Decisions

| Date | Decision |
|---|---|
| 2026-11-08 | Full redraws every frame for now: 0.6 ms at 1080p, 1.4 ms at 4K including the edit. Damage tracking waits until something needs it. |

[`DrawList`]: src/list.rs
[`rasterize`]: src/lib.rs
[`Canvas`]: src/canvas.rs
[`ImageStore`]: src/image.rs

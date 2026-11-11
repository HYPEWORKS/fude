# fude-text

Fonts, fallback, shaping and the glyph cache. It lays out one line of text at a time and puts glyph images into a [fude-render](../fude-render/README.md) image store for the draw list.

Dependencies: `fude-render`, `harfrust`, `swash`, `fontique`, `rustc-hash`. Used by `fude-ui` and `fude`.

## Fonts

- **Default font:** JetBrains Mono is bundled (OFL; two variable font files, about 380 KB, covering regular, bold and italic), so the first frame needs no font discovery and looks the same on every platform.
- **Fallback:** characters JetBrains Mono lacks (CJK, emoji, most scripts) come from system fonts via `fontique` (fontconfig, CoreText, DirectWrite). The system font collection opens only when the first such character appears (about 20 ms, once; fontconfig is loaded at runtime, not linked), so documents in Latin, Greek and Cyrillic never pay for it. The font chosen is cached per character.
- **Fallback order:** the emoji font for emoji, then monospace fonts, then the system's fallbacks for the character's script (with the user's locale, so Han characters get Japanese or Chinese forms as appropriate). Combining marks and joiners stay in their base character's font.
- **Color emoji:** swash draws bitmap (CBDT, sbix) and COLRv0 color emoji but not COLRv1; where the only color emoji font is COLRv1 (Fedora's Noto Color Emoji), fallback skips it for a monochrome emoji font.

## Shaping

- **`harfrust`** (a pure-Rust HarfBuzz port) shapes text and provides glyph metrics.
- **Fast path:** runs that can't need shaping (code points below U+0300, ligatures off) map characters straight to glyphs, which is correct for monospace fonts without ligatures, so most lines never touch the shaper. The fast path uses harfrust's metrics too, and a test checks it produces exactly what the shaper would.
- **Caches:** shaping plans are kept per font and segment properties (building one compiles the font's features, which for a CJK font costs far more than shaping a short run), and glyphs and advances for characters below U+0300 are cached per face. Shaped lines are cached by content.

## Rasterization

`swash` renders alpha masks (and color emoji) into an LRU glyph cache keyed by font, glyph, size and ¼-pixel horizontal offset. Glyphs with no ink (spaces) are remembered as empty. swash produces straight-alpha RGBA; the cache premultiplies it for the renderer.

If swash's output doesn't match ClearType expectations, a DirectWrite rasterizer could be added on Windows *(planned, if needed)*. Rasterization is contained in `cache.rs`, but nothing is abstracted for a second backend yet.

## Chips

Control characters and bytes that aren't valid UTF-8 have no glyphs, so they are laid out as small labeled boxes ([`Chip`]: `NUL`, `ESC`, `FF`), visible and one cursor step each. The view encodes them into the text it lays out as code points from the end of Unicode's last private use plane, which real text practically never uses; layout gives each one a box-sized advance, so tab stops and hit testing just work, and the view draws the box.

## Modules

| File | |
|---|---|
| `lib.rs` | The text system: fonts, line layout, glyph lookup for drawing |
| `face.rs` | One font at one point in its design space, with its per-character cache |
| `fallback.rs` | Finding system fonts for characters the bundled font lacks |
| `layout.rs` | Shaping, the fast path, and [`LineLayout`] |
| `cache.rs` | The glyph cache: rasterizing with swash |
| `chip.rs` | Chips for control characters and invalid bytes |

## Decisions

| Date | Decision |
|---|---|
| 2026-11-07 | Bundle JetBrains Mono as the default font. |
| 2026-11-07 | Shaping and glyph metrics come from harfrust (which also drives the ASCII fast path, so both paths agree); swash only rasterizes. |
| 2026-11-08 | `fontique` for font fallback, with fontconfig loaded at runtime (`fontconfig-dlopen`), so it isn't linked and costs nothing until a character needs it. |
| 2026-11-08 | harfrust shaping plans are cached per font and segment properties (building one compiles the font's features: 11× slower CJK layout without the cache), and glyphs and advances for characters below U+0300 are cached per face. |

[`Chip`]: src/chip.rs
[`LineLayout`]: src/layout.rs

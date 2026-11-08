use crate::TextStyle;
use crate::face::Face;
use fude_render::{Image, ImageFormat, ImageId, ImageStore};
use rustc_hash::FxHashMap;
use swash::scale::image::{Content, Image as SwashImage};
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Vector};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    font: u16,
    glyph: u32,
    px_bits: u32,
    subpixel: u8,
    hint: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct CachedGlyph {
    pub(crate) image: ImageId,
    /// Offset from the pen position to the image's left edge.
    pub(crate) left: i32,
    /// Distance from the baseline up to the image's top edge.
    pub(crate) top: i32,
    pub(crate) color: bool,
}

/// Rasterized glyphs. `None` entries remember glyphs with no ink (spaces).
pub(crate) struct GlyphCache {
    map: FxHashMap<GlyphKey, Option<CachedGlyph>>,
    scale: ScaleContext,
    scratch: SwashImage,
}

const SOURCES: &[Source] = &[
    Source::ColorOutline(0),
    Source::ColorBitmap(StrikeWith::BestFit),
    Source::Outline,
];

impl GlyphCache {
    pub(crate) fn new() -> GlyphCache {
        GlyphCache {
            map: FxHashMap::default(),
            scale: ScaleContext::new(),
            scratch: SwashImage::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }

    pub(crate) fn get(
        &mut self,
        face: &Face,
        style: &TextStyle,
        glyph: u32,
        subpixel: u8,
        images: &mut ImageStore,
    ) -> Option<CachedGlyph> {
        let key = GlyphKey {
            font: style.font.0,
            glyph,
            px_bits: style.px.to_bits(),
            subpixel,
            hint: style.hint,
        };
        if let Some(hit) = self.map.get(&key) {
            return *hit;
        }
        let entry = self.rasterize(face, style, glyph, subpixel, images);
        self.map.insert(key, entry);
        entry
    }

    fn rasterize(
        &mut self,
        face: &Face,
        style: &TextStyle,
        glyph: u32,
        subpixel: u8,
        images: &mut ImageStore,
    ) -> Option<CachedGlyph> {
        let glyph = u16::try_from(glyph).ok()?;
        let mut scaler = self
            .scale
            .builder(face.swash())
            .size(style.px)
            .hint(style.hint)
            .normalized_coords(&face.coords)
            .build();
        // swash resizes the buffer but keeps its old contents, and rasterizes by
        // accumulating coverage, so the scratch image must start empty.
        self.scratch.clear();
        let rendered = Render::new(SOURCES)
            .format(Format::Alpha)
            .offset(Vector::new(subpixel as f32 / 4.0, 0.0))
            .render_into(&mut scaler, glyph, &mut self.scratch);
        let p = self.scratch.placement;
        if !rendered || p.width == 0 || p.height == 0 {
            return None;
        }
        let (format, data, color) = match self.scratch.content {
            Content::Mask => (ImageFormat::Alpha, self.scratch.data.clone(), false),
            Content::Color => (ImageFormat::Rgba, premultiply(&self.scratch.data), true),
            // We request alpha masks, so subpixel masks shouldn't appear; reduce to alpha.
            Content::SubpixelMask => (
                ImageFormat::Alpha,
                self.scratch.data.chunks_exact(4).map(|c| c[1]).collect(),
                false,
            ),
        };
        let image = images.insert(Image {
            width: p.width,
            height: p.height,
            format,
            data,
        });
        Some(CachedGlyph {
            image,
            left: p.left,
            top: p.top,
            color,
        })
    }
}

/// swash produces straight-alpha RGBA; the renderer wants premultiplied.
fn premultiply(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .flat_map(|c| {
            let a = c[3] as u32;
            let m = |v: u8| ((v as u32 * a + 127) / 255) as u8;
            [m(c[0]), m(c[1]), m(c[2]), c[3]]
        })
        .collect()
}

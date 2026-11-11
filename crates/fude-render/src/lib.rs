//! Display list and CPU rasterizer.
//!
//! The UI describes a frame as a flat [`DrawList`] of rectangles and images (glyph masks,
//! color glyphs). [`rasterize`] turns that list into pixels on a [`Canvas`]. The same list
//! is what a GPU backend would consume, so a future backend can replace the rasterizer
//! without touching the UI (see `ARCHITECTURE.md`, "Rendering").
//!
//! Pixels are `0x00RRGGBB`, the format `softbuffer` presents on every platform.

mod blend;
mod canvas;
mod geom;
mod image;
mod list;
pub mod png;

pub use canvas::Canvas;
pub use geom::Rect;
pub use image::{Image, ImageFormat, ImageId, ImageStore};
pub use list::{DrawCmd, DrawList};

/// A color with straight (non-premultiplied) alpha, stored as `0xAARRGGBB`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Color(pub u32);

impl Color {
    pub const TRANSPARENT: Color = Color(0);

    /// An opaque color from `0xRRGGBB`.
    pub const fn rgb(rgb: u32) -> Color {
        Color(0xFF00_0000 | (rgb & 0x00FF_FFFF))
    }

    /// A color from `0xRRGGBB` and an alpha.
    pub const fn rgba(rgb: u32, alpha: u8) -> Color {
        Color(((alpha as u32) << 24) | (rgb & 0x00FF_FFFF))
    }

    pub const fn alpha(self) -> u8 {
        (self.0 >> 24) as u8
    }

    /// The color as an opaque `0x00RRGGBB` pixel, ignoring alpha.
    pub const fn pixel(self) -> u32 {
        self.0 & 0x00FF_FFFF
    }
}

/// Draws `list` onto `canvas`, touching only pixels inside `damage`.
///
/// Commands are applied in order. Clip rectangles nest and are always intersected with
/// `damage`, so a partial redraw costs only the area being redrawn.
pub fn rasterize(list: &DrawList, images: &ImageStore, canvas: &mut Canvas<'_>, damage: Rect) {
    let mut clips: Vec<Rect> = Vec::new();
    let mut clip = damage.intersect(canvas.bounds());
    for cmd in list.commands() {
        match *cmd {
            DrawCmd::Rect { rect, color } => canvas.fill_rect(rect, color, clip),
            DrawCmd::Mask { image, x, y, color } => {
                if let Some(img) = images.get(image) {
                    canvas.draw_image(img, x, y, color, clip);
                }
            }
            DrawCmd::Image { image, x, y } => {
                if let Some(img) = images.get(image) {
                    canvas.draw_image(img, x, y, Color::rgb(0xFFFFFF), clip);
                }
            }
            DrawCmd::PushClip(rect) => {
                clips.push(clip);
                clip = clip.intersect(rect);
            }
            DrawCmd::PopClip => {
                clip = clips.pop().unwrap_or(damage.intersect(canvas.bounds()));
            }
        }
    }
}

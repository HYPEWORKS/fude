//! Drawing helpers shared by every widget: the text system, the draw list, and sizes in
//! logical pixels converted to physical ones.

use fude_render::{Canvas, Color, DrawList, ImageStore, Rect, rasterize};
use fude_text::{FontId, LineLayout, LineMetrics, TextStyle, TextSystem};

use crate::theme::Theme;

pub struct Painter {
    pub text: TextSystem,
    pub images: ImageStore,
    pub list: DrawList,
    pub theme: &'static Theme,
    /// Physical pixels per logical pixel for the frame being drawn.
    pub scale: f32,
    scratch: LineLayout,
}

impl Painter {
    pub fn new(theme: &'static Theme) -> Painter {
        Painter {
            text: TextSystem::new(),
            images: ImageStore::new(),
            list: DrawList::new(),
            theme,
            scale: 1.0,
            scratch: LineLayout::default(),
        }
    }

    pub fn begin(&mut self, scale: f32) {
        self.list.clear();
        self.scale = scale;
    }

    pub fn finish(&mut self, canvas: &mut Canvas<'_>) {
        rasterize(&self.list, &self.images, canvas, canvas.bounds());
    }

    /// Logical pixels to physical, rounded.
    pub fn px(&self, logical: f32) -> i32 {
        (logical * self.scale).round() as i32
    }

    /// The UI font at a logical size.
    pub fn style(&self, logical_size: f32) -> TextStyle {
        TextStyle::new(FontId::MONO, (logical_size * self.scale).round().max(1.0))
    }

    pub fn metrics(&self, style: &TextStyle) -> LineMetrics {
        self.text.metrics(style.font, style.px)
    }

    pub fn rect(&mut self, rect: Rect, color: Color) {
        self.list.rect(rect, color);
    }

    pub fn measure(&mut self, s: &str, style: &TextStyle) -> f32 {
        let mut scratch = std::mem::take(&mut self.scratch);
        self.text.layout_line(s, style, &mut scratch);
        let w = scratch.width;
        self.scratch = scratch;
        w
    }

    /// Draws a single-line string with its baseline at `baseline`. Returns its width.
    pub fn text(&mut self, s: &str, style: &TextStyle, x: f32, baseline: i32, color: Color) -> f32 {
        let mut scratch = std::mem::take(&mut self.scratch);
        self.text.layout_line(s, style, &mut scratch);
        self.text.draw_line(
            &scratch,
            style,
            x,
            baseline,
            color,
            (f32::MIN, f32::MAX),
            &mut self.list,
            &mut self.images,
        );
        let w = scratch.width;
        self.scratch = scratch;
        w
    }

    /// The baseline that vertically centers `style`'s line box in `height` pixels at `top`.
    pub fn center_baseline(&self, top: i32, height: i32, style: &TextStyle) -> i32 {
        let m = self.metrics(style);
        top + ((height as f32 - (m.ascent + m.descent)) / 2.0 + m.ascent).round() as i32
    }
}

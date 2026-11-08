//! The status bar below the editor.

use fude_render::Rect;

use crate::painter::Painter;

/// Logical height of the status bar.
pub const STATUS_BAR_HEIGHT: f32 = 26.0;
const CHROME_FONT: f32 = 13.0;

/// A message shown in the status bar until replaced or dismissed.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusMessage {
    pub text: String,
    pub error: bool,
}

/// `left` is the cursor position; `items` are shown right-aligned, in order.
pub fn status_bar(
    p: &mut Painter,
    area: Rect,
    left: &str,
    message: Option<&StatusMessage>,
    items: &[String],
) {
    let t = p.theme;
    let style = p.style(CHROME_FONT);
    p.rect(area, t.chrome_bg);
    p.rect(Rect::new(area.x, area.y, area.w, p.px(1.0)), t.border);
    let baseline = p.center_baseline(area.y, area.h, &style);
    let pad = p.px(16.0) as f32;
    let left_w = p.text(left, &style, area.x as f32 + pad, baseline, t.chrome_text);
    if let Some(m) = message {
        let color = if m.error { t.error } else { t.chrome_text };
        p.text(
            &m.text,
            &style,
            area.x as f32 + pad * 2.5 + left_w,
            baseline,
            color,
        );
    }
    let gap = p.px(24.0) as f32;
    let mut x = area.right() as f32 - pad;
    for item in items.iter().rev() {
        let w = p.measure(item, &style);
        x -= w;
        p.text(item, &style, x, baseline, t.chrome_text_muted);
        x -= gap;
    }
}

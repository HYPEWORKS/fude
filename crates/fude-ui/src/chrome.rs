//! The window's chrome: the tab strip above the editor and the status bar below it.

use fude_render::Rect;

use crate::painter::Painter;

/// Logical heights.
pub const TAB_STRIP_HEIGHT: f32 = 36.0;
pub const STATUS_BAR_HEIGHT: f32 = 26.0;
const CHROME_FONT: f32 = 13.0;

/// One tab for now (tabs arrive in phase 3). A modified document shows a dot.
pub fn tab_strip(p: &mut Painter, area: Rect, title: &str, modified: bool) {
    let t = p.theme;
    let style = p.style(CHROME_FONT);
    p.rect(area, t.chrome_bg);
    let pad = p.px(16.0);
    let title_w = p.measure(title, &style).ceil() as i32;
    let dot_w = if modified { p.px(14.0) } else { 0 };
    let tab_w = (title_w + dot_w + pad * 2).max(p.px(120.0)).min(area.w);
    p.rect(Rect::new(area.x, area.y, tab_w, area.h), t.editor_bg);
    p.rect(Rect::new(area.x, area.y, tab_w, p.px(2.0)), t.accent);
    p.rect(
        Rect::new(
            area.x + tab_w,
            area.bottom() - p.px(1.0),
            area.w - tab_w,
            p.px(1.0),
        ),
        t.border,
    );
    let baseline = p.center_baseline(area.y, area.h, &style);
    p.text(
        title,
        &style,
        (area.x + pad) as f32,
        baseline,
        t.chrome_text,
    );
    if modified {
        let d = p.px(7.0);
        let x = area.x + pad + title_w + p.px(8.0);
        let y = area.y + (area.h - d) / 2;
        p.rect(Rect::new(x, y, d, d), t.chrome_text_muted);
    }
}

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

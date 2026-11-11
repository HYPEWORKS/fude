//! The status bar below the editor.

use accesskit::{Live, NodeId, Role};
use fude_render::Rect;

use crate::access::{self, AccessTree, Part};
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

/// Where the status bar drew its parts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatusBarRects {
    pub area: Rect,
    pub position: Rect,
    pub message: Rect,
    pub items: Vec<Rect>,
}

/// `left` is the cursor position; `items` are shown right-aligned, in order.
pub fn status_bar(
    p: &mut Painter,
    area: Rect,
    left: &str,
    message: Option<&StatusMessage>,
    items: &[String],
) -> StatusBarRects {
    let t = p.theme;
    let style = p.style(CHROME_FONT);
    p.rect(area, t.chrome_bg);
    p.rect(Rect::new(area.x, area.y, area.w, p.px(1.0)), t.border);
    let baseline = p.center_baseline(area.y, area.h, &style);
    let pad = p.px(16.0) as f32;
    let left_w = p.text(left, &style, area.x as f32 + pad, baseline, t.chrome_text);
    let span = |x: f32, w: f32| Rect::new(x as i32, area.y, w.ceil() as i32, area.h);
    let mut rects = StatusBarRects {
        area,
        position: span(area.x as f32 + pad, left_w),
        ..StatusBarRects::default()
    };
    if let Some(m) = message {
        let color = if m.error { t.error } else { t.chrome_text };
        let x = area.x as f32 + pad * 2.5 + left_w;
        let w = p.text(&m.text, &style, x, baseline, color);
        rects.message = span(x, w);
    }
    let gap = p.px(24.0) as f32;
    let mut x = area.right() as f32 - pad;
    for item in items.iter().rev() {
        let w = p.measure(item, &style);
        x -= w;
        p.text(item, &style, x, baseline, t.chrome_text_muted);
        rects.items.push(span(x, w));
        x -= gap;
    }
    rects.items.reverse();
    rects
}

/// Describes the status bar for assistive technologies. A message is read out when it
/// appears (an error at once), unless `announce` is false (progress that changes too
/// often to read).
pub fn status_bar_access(
    tree: &mut AccessTree,
    rects: &StatusBarRects,
    left: &str,
    message: Option<&StatusMessage>,
    announce: bool,
    items: &[String],
) -> NodeId {
    let mut children =
        vec![tree.push(Part::StatusItem(0), access::text_node(left, rects.position))];
    // Always there, so a message appearing is a change to it, which is what's read out.
    let mut node = access::text_node(message.map_or("", |m| &m.text), rects.message);
    node.set_live(match message {
        Some(m) if m.error => Live::Assertive,
        _ if announce => Live::Polite,
        _ => Live::Off,
    });
    children.push(tree.push(Part::StatusItem(1), node));
    for (i, (item, rect)) in items.iter().zip(&rects.items).enumerate() {
        children.push(tree.push(Part::StatusItem(i + 2), access::text_node(item, *rect)));
    }
    let mut bar = access::labeled(Role::Status, "Status bar", rects.area);
    bar.set_children(children);
    tree.push(Part::StatusBar, bar)
}

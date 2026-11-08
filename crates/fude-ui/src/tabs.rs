//! The tab strip: one tab per open document.
//!
//! Click a tab to switch to it, middle-click or click its × to close it, drag it to
//! reorder, double-click the empty strip for a new tab, and use the wheel when there are
//! more tabs than fit.

use fude_platform::MouseButton;
use fude_render::Rect;

use crate::painter::Painter;

/// Logical height of the strip.
pub const TAB_STRIP_HEIGHT: f32 = 36.0;
const FONT: f32 = 13.0;
const MIN_W: f32 = 96.0;
const MAX_W: f32 = 240.0;
const PAD: f32 = 14.0;
const CLOSE: f32 = 18.0;
/// How far a press must move before it becomes a drag.
const DRAG_START: f32 = 5.0;

pub struct TabInfo<'a> {
    pub title: &'a str,
    pub modified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabEvent {
    Activate(usize),
    Close(usize),
    /// A dragged tab moved from one position to another.
    Move {
        from: usize,
        to: usize,
    },
    New,
}

#[derive(Clone, Copy, Debug)]
struct Press {
    index: usize,
    /// Pressed on the close button: closes on release if still over it.
    on_close: bool,
    x0: f32,
    dragging: bool,
}

#[derive(Clone, Copy, Debug)]
struct TabRect {
    tab: Rect,
    close: Rect,
}

#[derive(Default)]
pub struct TabStrip {
    /// Horizontal scroll, physical pixels.
    scroll: f32,
    hover: Option<usize>,
    hover_close: bool,
    press: Option<Press>,
    rects: Vec<TabRect>,
    area: Rect,
    content_w: i32,
    /// Scroll the active tab into view at the next draw.
    reveal: bool,
}

impl TabStrip {
    pub fn new() -> TabStrip {
        TabStrip::default()
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.area.contains(x as i32, y as i32)
    }

    /// Makes sure the active tab is visible at the next draw.
    pub fn reveal_active(&mut self) {
        self.reveal = true;
    }

    fn tab_at(&self, x: f32, y: f32) -> Option<(usize, bool)> {
        let (x, y) = (x as i32, y as i32);
        if !self.area.contains(x, y) {
            return None;
        }
        self.rects
            .iter()
            .position(|r| r.tab.contains(x, y))
            .map(|i| (i, self.rects[i].close.contains(x, y)))
    }

    pub fn mouse_down(
        &mut self,
        x: f32,
        y: f32,
        button: MouseButton,
        clicks: u8,
    ) -> Option<TabEvent> {
        if !self.contains(x, y) {
            return None;
        }
        match (self.tab_at(x, y), button) {
            (Some((i, _)), MouseButton::Middle) => Some(TabEvent::Close(i)),
            (Some((i, on_close)), MouseButton::Left) => {
                self.press = Some(Press {
                    index: i,
                    on_close,
                    x0: x,
                    dragging: false,
                });
                (!on_close).then_some(TabEvent::Activate(i))
            }
            (None, MouseButton::Left) if clicks == 2 => Some(TabEvent::New),
            _ => None,
        }
    }

    /// Returns whether to redraw, and a move if a drag crossed into another tab.
    pub fn mouse_move(&mut self, x: f32, y: f32) -> (bool, Option<TabEvent>) {
        let hit = self.tab_at(x, y);
        let (hover, hover_close) = (hit.map(|h| h.0), hit.is_some_and(|h| h.1));
        let mut redraw = hover != self.hover || hover_close != self.hover_close;
        self.hover = hover;
        self.hover_close = hover_close;
        let mut event = None;
        if let Some(press) = &mut self.press
            && !press.on_close
        {
            if !press.dragging && (x - press.x0).abs() >= DRAG_START {
                press.dragging = true;
            }
            if press.dragging
                && let Some((i, _)) = hit
                && i != press.index
            {
                event = Some(TabEvent::Move {
                    from: press.index,
                    to: i,
                });
                press.index = i;
                redraw = true;
            }
        }
        (redraw, event)
    }

    pub fn mouse_up(&mut self, x: f32, y: f32) -> Option<TabEvent> {
        let press = self.press.take()?;
        match self.tab_at(x, y) {
            Some((i, true)) if press.on_close && i == press.index => Some(TabEvent::Close(i)),
            _ => None,
        }
    }

    pub fn mouse_leave(&mut self) -> bool {
        let changed = self.hover.is_some();
        self.hover = None;
        self.hover_close = false;
        changed
    }

    /// Wheel over the strip scrolls it sideways.
    pub fn scroll(&mut self, delta: f32) {
        let max = (self.content_w - self.area.w).max(0) as f32;
        self.scroll = (self.scroll + delta).clamp(0.0, max);
    }

    pub fn draw(&mut self, p: &mut Painter, area: Rect, tabs: &[TabInfo<'_>], active: usize) {
        let t = p.theme;
        let style = p.style(FONT);
        self.area = area;
        p.rect(area, t.chrome_bg);
        let pad = p.px(PAD);
        let close_w = p.px(CLOSE);
        let (min_w, max_w) = (p.px(MIN_W), p.px(MAX_W));
        let widths: Vec<i32> = tabs
            .iter()
            .map(|tab| {
                (p.measure(tab.title, &style).ceil() as i32 + 2 * pad + close_w).clamp(min_w, max_w)
            })
            .collect();
        self.content_w = widths.iter().sum();
        let max_scroll = (self.content_w - area.w).max(0) as f32;
        if std::mem::take(&mut self.reveal) {
            let start: i32 = widths[..active.min(widths.len())].iter().sum();
            let end = start + widths.get(active).copied().unwrap_or(0);
            if (start as f32) < self.scroll {
                self.scroll = start as f32;
            } else if end as f32 > self.scroll + area.w as f32 {
                self.scroll = (end - area.w) as f32;
            }
        }
        self.scroll = self.scroll.clamp(0.0, max_scroll);

        p.list.push_clip(area);
        self.rects.clear();
        let mut x = area.x - self.scroll.round() as i32;
        let baseline = p.center_baseline(area.y, area.h, &style);
        for (i, (tab, w)) in tabs.iter().zip(widths).enumerate() {
            let r = Rect::new(x, area.y, w, area.h);
            let close = Rect::new(
                x + w - pad / 2 - close_w,
                area.y + (area.h - close_w) / 2,
                close_w,
                close_w,
            );
            self.rects.push(TabRect { tab: r, close });
            let is_active = i == active;
            let hovered = self.hover == Some(i);
            if is_active {
                p.rect(r, t.editor_bg);
                p.rect(Rect::new(r.x, r.y, r.w, p.px(2.0)), t.accent);
            } else if hovered {
                p.rect(r, t.line_highlight);
            }
            // Separator on the right, and the strip's bottom border outside the active tab.
            p.rect(
                Rect::new(r.right() - 1, r.y + p.px(8.0), 1, r.h - 2 * p.px(8.0)),
                t.border,
            );
            if !is_active {
                p.rect(
                    Rect::new(r.x, r.bottom() - p.px(1.0), r.w, p.px(1.0)),
                    t.border,
                );
            }
            let room = (w - 2 * pad - close_w) as f32;
            let title = p.fit(tab.title, &style, room);
            let color = if is_active {
                t.chrome_text
            } else {
                t.chrome_text_muted
            };
            p.text(&title, &style, (x + pad) as f32, baseline, color);

            // The close button shows on the active and hovered tabs; a modified tab shows
            // a dot there instead, until the pointer is over it.
            let over_close = hovered && self.hover_close;
            if over_close {
                p.rect(close, t.button_bg);
            }
            if tab.modified && !over_close {
                let d = p.px(8.0);
                p.rect(
                    Rect::new(
                        close.x + (close.w - d) / 2,
                        close.y + (close.h - d) / 2,
                        d,
                        d,
                    ),
                    if is_active {
                        t.chrome_text
                    } else {
                        t.chrome_text_muted
                    },
                );
            } else if is_active || hovered {
                let cross = p.style(FONT + 1.0);
                let cw = p.measure("×", &cross);
                let cb = p.center_baseline(close.y, close.h, &cross);
                p.text(
                    "×",
                    &cross,
                    close.x as f32 + (close.w as f32 - cw) / 2.0,
                    cb,
                    color,
                );
            }
            x += w;
        }
        if x < area.right() {
            p.rect(
                Rect::new(x, area.bottom() - p.px(1.0), area.right() - x, p.px(1.0)),
                t.border,
            );
        }
        p.list.pop_clip();
    }
}

//! A modal question in the window ("Save changes?"), drawn like the rest of the UI rather
//! than as a native message box, so it looks the same everywhere and needs no helper
//! programs. Keyboard first: Enter picks the default, Escape cancels, a button's first
//! letter picks it, Tab and the arrow keys move focus.

use accesskit::{Action, ActionRequest, NodeId, Role};
use fude_platform::{Key, KeyInput};
use fude_render::Rect;

use crate::access::{self, AccessTree, Part};
use crate::painter::Painter;

pub struct Prompt {
    pub title: String,
    pub message: String,
    pub buttons: Vec<String>,
    /// Chosen by Enter, and drawn as the primary button.
    pub default: usize,
    /// Chosen by Escape.
    pub cancel: usize,
    focus: usize,
    hover: Option<usize>,
    /// Button rectangles from the last draw.
    rects: Vec<Rect>,
    /// The dialog and its message, from the last draw.
    area: Rect,
    message_rect: Rect,
}

/// What a key or click did to a prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptInput {
    Chose(usize),
    /// Handled (focus moved), nothing chosen.
    Handled,
    Ignored,
}

impl Prompt {
    pub fn new(
        title: &str,
        message: &str,
        buttons: &[&str],
        default: usize,
        cancel: usize,
    ) -> Prompt {
        Prompt {
            title: title.to_owned(),
            message: message.to_owned(),
            buttons: buttons.iter().map(|&b| b.to_owned()).collect(),
            default,
            cancel,
            focus: default,
            hover: None,
            rects: Vec::new(),
            area: Rect::default(),
            message_rect: Rect::default(),
        }
    }

    /// Describes the dialog for assistive technologies: a modal alert, its title as its
    /// name, its message, and the buttons, the focused one having focus.
    pub fn access(&self, tree: &mut AccessTree) -> NodeId {
        let message = tree.push(
            Part::PromptMessage,
            access::text_node(&self.message, self.message_rect),
        );
        let mut children = vec![message];
        for (i, (label, rect)) in self.buttons.iter().zip(&self.rects).enumerate() {
            let role = if i == self.default {
                Role::DefaultButton
            } else {
                Role::Button
            };
            let mut button = access::clickable(role, label, *rect);
            button.add_action(Action::Focus);
            children.push(tree.push(Part::PromptButton(i), button));
        }
        let mut dialog = access::labeled(Role::AlertDialog, &self.title, self.area);
        dialog.set_modal();
        dialog.set_described_by(vec![message]);
        dialog.set_children(children);
        tree.push(Part::Prompt, dialog)
    }

    /// What has focus in the dialog.
    pub fn access_focus(&self) -> Part {
        Part::PromptButton(self.focus)
    }

    /// Does what an assistive technology asks: clicks or focuses a button.
    pub fn access_action(&mut self, part: Part, request: &ActionRequest) -> PromptInput {
        let Part::PromptButton(i) = part else {
            return PromptInput::Ignored;
        };
        if i >= self.buttons.len() {
            return PromptInput::Ignored;
        }
        match request.action {
            Action::Click => PromptInput::Chose(i),
            Action::Focus => {
                self.focus = i;
                PromptInput::Handled
            }
            _ => PromptInput::Ignored,
        }
    }

    pub fn key(&mut self, key: &KeyInput) -> PromptInput {
        if !key.pressed {
            return PromptInput::Ignored;
        }
        let n = self.buttons.len();
        match &key.key {
            Key::Enter => PromptInput::Chose(self.focus),
            Key::Escape => PromptInput::Chose(self.cancel),
            Key::Tab | Key::Right if !key.modifiers.shift => {
                self.focus = (self.focus + 1) % n;
                PromptInput::Handled
            }
            Key::Tab | Key::Left => {
                self.focus = (self.focus + n - 1) % n;
                PromptInput::Handled
            }
            Key::Character(c) if c == " " => PromptInput::Chose(self.focus),
            Key::Character(c) => {
                let c = c.to_lowercase();
                match self
                    .buttons
                    .iter()
                    .position(|b| b.to_lowercase().starts_with(&c))
                {
                    Some(i) => PromptInput::Chose(i),
                    None => PromptInput::Handled,
                }
            }
            // A dialog swallows everything else.
            _ => PromptInput::Handled,
        }
    }

    pub fn mouse_move(&mut self, x: f32, y: f32) -> bool {
        let hover = self.button_at(x, y);
        std::mem::replace(&mut self.hover, hover) != hover
    }

    pub fn mouse_down(&mut self, x: f32, y: f32) -> Option<usize> {
        self.button_at(x, y)
    }

    fn button_at(&self, x: f32, y: f32) -> Option<usize> {
        self.rects
            .iter()
            .position(|r| r.contains(x as i32, y as i32))
    }

    pub fn draw(&mut self, p: &mut Painter, window: Rect) {
        let t = p.theme;
        p.rect(window, t.scrim);
        let body = p.style(13.0);
        let title_style = fude_text::TextStyle {
            font: fude_text::FontId::MONO_BOLD,
            ..p.style(14.0)
        };
        let pad = p.px(20.0);
        let width = p.px(480.0).min(window.w - p.px(32.0)).max(p.px(200.0));
        let line_h = p.px(20.0);
        let column = p.metrics(&body).space_advance.max(1.0);
        let lines = wrap(&self.message, ((width - 2 * pad) as f32 / column) as usize);
        let button_h = p.px(30.0);
        let height =
            pad + line_h + p.px(8.0) + lines.len() as i32 * line_h + p.px(20.0) + button_h + pad;
        let x = window.x + (window.w - width) / 2;
        let y = window.y + (window.h - height) / 2;
        self.area = Rect::new(x, y, width, height);
        p.rect(Rect::new(x - 1, y - 1, width + 2, height + 2), t.border);
        p.rect(Rect::new(x, y, width, height), t.dialog_bg);

        let mut ty = y + pad;
        let baseline = p.center_baseline(ty, line_h, &title_style);
        p.text(
            &self.title,
            &title_style,
            (x + pad) as f32,
            baseline,
            t.text,
        );
        ty += line_h + p.px(8.0);
        self.message_rect = Rect::new(x + pad, ty, width - 2 * pad, lines.len() as i32 * line_h);
        for line in &lines {
            let baseline = p.center_baseline(ty, line_h, &body);
            p.text(line, &body, (x + pad) as f32, baseline, t.chrome_text);
            ty += line_h;
        }

        // Buttons, right-aligned, in order.
        let gap = p.px(8.0);
        let bpad = p.px(14.0);
        let widths: Vec<i32> = self
            .buttons
            .clone()
            .iter()
            .map(|b| p.measure(b, &body).ceil() as i32 + 2 * bpad)
            .collect();
        let total: i32 = widths.iter().sum::<i32>() + gap * (widths.len() as i32 - 1);
        let mut bx = x + width - pad - total;
        let by = y + height - pad - button_h;
        self.rects.clear();
        for (i, (label, w)) in self.buttons.clone().iter().zip(widths).enumerate() {
            let r = Rect::new(bx, by, w, button_h);
            let (bg, fg) = if i == self.default {
                (t.accent, t.accent_text)
            } else {
                (t.button_bg, t.button_text)
            };
            if i == self.focus {
                let ring = p.px(2.0).max(1);
                let o = ring + p.px(1.0);
                p.rect(
                    Rect::new(r.x - o, r.y - o, r.w + 2 * o, r.h + 2 * o),
                    t.text,
                );
                p.rect(
                    Rect::new(
                        r.x - o + ring,
                        r.y - o + ring,
                        r.w + 2 * (o - ring),
                        r.h + 2 * (o - ring),
                    ),
                    t.dialog_bg,
                );
            }
            p.rect(r, bg);
            if self.hover == Some(i) {
                p.rect(r, t.scrollbar);
            }
            let baseline = p.center_baseline(by, button_h, &body);
            p.text(label, &body, (bx + bpad) as f32, baseline, fg);
            self.rects.push(r);
            bx += w + gap;
        }
    }
}

/// Wraps text at word boundaries to `columns` characters per line.
fn wrap(text: &str, columns: usize) -> Vec<String> {
    let columns = columns.max(10);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let needed =
                line.chars().count() + usize::from(!line.is_empty()) + word.chars().count();
            if !line.is_empty() && needed > columns {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

use crate::{Color, ImageId, Rect};

/// One drawing operation. Coordinates are physical pixels.
#[derive(Clone, Copy, Debug)]
pub enum DrawCmd {
    Rect {
        rect: Rect,
        color: Color,
    },
    /// An alpha mask tinted with `color`, top-left at (`x`, `y`).
    Mask {
        image: ImageId,
        x: i32,
        y: i32,
        color: Color,
    },
    /// An RGBA image, top-left at (`x`, `y`).
    Image {
        image: ImageId,
        x: i32,
        y: i32,
    },
    PushClip(Rect),
    PopClip,
}

/// A frame's worth of drawing operations, in painter's order.
#[derive(Default)]
pub struct DrawList {
    cmds: Vec<DrawCmd>,
}

impl DrawList {
    pub fn new() -> DrawList {
        DrawList::default()
    }

    /// Empties the list, keeping its allocation for the next frame.
    pub fn clear(&mut self) {
        self.cmds.clear();
    }

    pub fn commands(&self) -> &[DrawCmd] {
        &self.cmds
    }

    pub fn len(&self) -> usize {
        self.cmds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }

    pub fn rect(&mut self, rect: Rect, color: Color) {
        if !rect.is_empty() {
            self.cmds.push(DrawCmd::Rect { rect, color });
        }
    }

    pub fn mask(&mut self, image: ImageId, x: i32, y: i32, color: Color) {
        self.cmds.push(DrawCmd::Mask { image, x, y, color });
    }

    pub fn image(&mut self, image: ImageId, x: i32, y: i32) {
        self.cmds.push(DrawCmd::Image { image, x, y });
    }

    pub fn push_clip(&mut self, rect: Rect) {
        self.cmds.push(DrawCmd::PushClip(rect));
    }

    pub fn pop_clip(&mut self) {
        self.cmds.push(DrawCmd::PopClip);
    }
}

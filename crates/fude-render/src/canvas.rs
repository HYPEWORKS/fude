use crate::blend::{blend, blend_premul};
use crate::{Color, Image, ImageFormat, Rect};

/// A borrowed `0x00RRGGBB` pixel buffer, rows packed with no padding.
///
/// Borrowing lets the rasterizer draw straight into the buffer the platform presents,
/// with no intermediate copy.
pub struct Canvas<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
}

impl<'a> Canvas<'a> {
    /// # Panics
    /// If `pixels.len() != width * height`.
    pub fn new(pixels: &'a mut [u32], width: usize, height: usize) -> Canvas<'a> {
        assert_eq!(pixels.len(), width * height, "canvas size mismatch");
        Canvas {
            pixels,
            width,
            height,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn pixels(&self) -> &[u32] {
        self.pixels
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }

    pub fn fill_rect(&mut self, rect: Rect, color: Color, clip: Rect) {
        let r = rect.intersect(clip).intersect(self.bounds());
        if r.is_empty() || color.alpha() == 0 {
            return;
        }
        let (x0, x1) = (r.x as usize, r.right() as usize);
        let a = color.alpha() as u32;
        let px = color.pixel();
        for y in r.y as usize..r.bottom() as usize {
            let row = &mut self.pixels[y * self.width + x0..y * self.width + x1];
            if a == 255 {
                row.fill(px);
            } else {
                for d in row {
                    *d = blend(*d, px, a);
                }
            }
        }
    }

    /// Draws `img` with its top-left corner at (`x`, `y`). Alpha masks are tinted with
    /// `color`; RGBA images are drawn as-is.
    pub fn draw_image(&mut self, img: &Image, x: i32, y: i32, color: Color, clip: Rect) {
        let dst = Rect::new(x, y, img.width as i32, img.height as i32)
            .intersect(clip)
            .intersect(self.bounds());
        if dst.is_empty() {
            return;
        }
        let (sx0, sy0) = ((dst.x - x) as usize, (dst.y - y) as usize);
        let w = dst.w as usize;
        let iw = img.width as usize;
        match img.format {
            ImageFormat::Alpha => {
                let ca = color.alpha() as u32;
                let px = color.pixel();
                for row in 0..dst.h as usize {
                    let src = &img.data[(sy0 + row) * iw + sx0..][..w];
                    let start = (dst.y as usize + row) * self.width + dst.x as usize;
                    let out = &mut self.pixels[start..start + w];
                    for (d, &m) in out.iter_mut().zip(src) {
                        let a = if ca == 255 {
                            m as u32
                        } else {
                            (m as u32 * ca + 127) / 255
                        };
                        if a == 255 {
                            *d = px;
                        } else if a != 0 {
                            *d = blend(*d, px, a);
                        }
                    }
                }
            }
            ImageFormat::Rgba => {
                for row in 0..dst.h as usize {
                    let src = &img.data[((sy0 + row) * iw + sx0) * 4..][..w * 4];
                    let start = (dst.y as usize + row) * self.width + dst.x as usize;
                    let out = &mut self.pixels[start..start + w];
                    for (d, s) in out.iter_mut().zip(src.chunks_exact(4)) {
                        let a = s[3] as u32;
                        if a != 0 {
                            let p = (s[0] as u32) << 16 | (s[1] as u32) << 8 | s[2] as u32;
                            *d = blend_premul(*d, p, a);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_rect_clips_to_canvas_and_clip() {
        let mut px = vec![0u32; 4 * 4];
        let mut c = Canvas::new(&mut px, 4, 4);
        c.fill_rect(
            Rect::new(-2, -2, 10, 10),
            Color::rgb(0xFF0000),
            Rect::new(1, 1, 2, 2),
        );
        let filled: Vec<usize> = px
            .iter()
            .enumerate()
            .filter(|(_, p)| **p != 0)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(filled, vec![5, 6, 9, 10]);
    }

    #[test]
    fn mask_blends_coverage() {
        let mut px = vec![0u32; 3];
        let mut c = Canvas::new(&mut px, 3, 1);
        let img = Image {
            width: 3,
            height: 1,
            format: ImageFormat::Alpha,
            data: vec![0, 128, 255],
        };
        c.draw_image(&img, 0, 0, Color::rgb(0xFFFFFF), Rect::new(0, 0, 3, 1));
        assert_eq!(px, vec![0, 0x808080, 0xFFFFFF]);
    }

    #[test]
    fn image_partially_offscreen() {
        let mut px = vec![0u32; 2 * 2];
        let mut c = Canvas::new(&mut px, 2, 2);
        let img = Image {
            width: 2,
            height: 2,
            format: ImageFormat::Alpha,
            data: vec![255; 4],
        };
        c.draw_image(&img, -1, 1, Color::rgb(0x0000FF), Rect::new(0, 0, 2, 2));
        assert_eq!(px, vec![0, 0, 0xFF, 0]);
    }
}

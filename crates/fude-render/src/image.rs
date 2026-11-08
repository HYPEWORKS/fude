/// Pixel layout of an [`Image`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageFormat {
    /// One coverage byte per pixel, tinted when drawn (glyph masks).
    Alpha,
    /// Premultiplied RGBA, four bytes per pixel (color glyphs, icons).
    Rgba,
}

/// A small bitmap owned by an [`ImageStore`].
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub format: ImageFormat,
    pub data: Vec<u8>,
}

/// Handle to an image in an [`ImageStore`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ImageId(u32);

/// Storage for the bitmaps a [`crate::DrawList`] refers to.
///
/// On the CPU these are plain buffers; a GPU backend would mirror them into an atlas
/// texture keyed by the same ids.
#[derive(Default)]
pub struct ImageStore {
    images: Vec<Image>,
    bytes: usize,
}

impl ImageStore {
    pub fn new() -> ImageStore {
        ImageStore::default()
    }

    pub fn insert(&mut self, image: Image) -> ImageId {
        let id = ImageId(self.images.len() as u32);
        self.bytes += image.data.len();
        self.images.push(image);
        id
    }

    pub fn get(&self, id: ImageId) -> Option<&Image> {
        self.images.get(id.0 as usize)
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    /// Total bytes of pixel data held.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

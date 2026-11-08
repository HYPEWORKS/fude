//! How a document is stored on disk.

pub use encoding_rs::Encoding;

use crate::LineEnding;

/// Encoding, byte order mark and line ending style: what saving needs to know beyond the
/// text itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Format {
    pub encoding: &'static Encoding,
    /// Write a byte order mark (UTF-8 and UTF-16 only).
    pub bom: bool,
    /// The style for new line breaks. Existing ones are kept as they are.
    pub line_ending: LineEnding,
}

impl Default for Format {
    /// UTF-8 without a BOM, platform line endings.
    fn default() -> Format {
        Format {
            encoding: encoding_rs::UTF_8,
            bom: false,
            line_ending: LineEnding::PLATFORM,
        }
    }
}

impl Format {
    /// A short label for the status bar, e.g. "UTF-8", "UTF-8 BOM", "Shift_JIS".
    pub fn encoding_label(&self) -> String {
        let name = self.encoding.name();
        if self.bom && is_utf8(self.encoding) {
            format!("{name} BOM")
        } else {
            name.to_owned()
        }
    }
}

pub(crate) fn is_utf8(encoding: &Encoding) -> bool {
    encoding == encoding_rs::UTF_8
}

pub(crate) fn is_utf16(encoding: &Encoding) -> bool {
    encoding == encoding_rs::UTF_16LE || encoding == encoding_rs::UTF_16BE
}

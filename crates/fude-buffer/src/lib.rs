//! fude's text buffer: a persistent rope of raw bytes with snapshots and undo, plus
//! encoding detection, line endings, and safe loading and saving.
//!
//! The file's bytes are the truth: opening and saving an unedited file gives back
//! identical bytes, including invalid UTF-8 and mixed line endings.

mod buffer;
mod format;
mod line_ending;
mod load;
mod rope;
mod save;
mod utf8;

pub use buffer::{Bias, Buffer, Edit, Snapshot, Version, map_offset};
pub use format::{Encoding, Format};
pub use line_ending::{LineEnding, LineEndingCounts, convert_line_endings};
pub use load::{FileStamp, LoadObserver, Loaded, decode, load, load_observed, load_with, read};
pub use rope::{Chunks, LineSpan, Lines, Rope, RopeBuilder};
pub use save::{SaveError, encode, save};

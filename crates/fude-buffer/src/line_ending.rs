//! Line ending styles, detection and conversion.

use crate::{Rope, RopeBuilder};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

impl LineEnding {
    /// The style new documents use: CRLF on Windows, LF elsewhere.
    pub const PLATFORM: LineEnding = if cfg!(windows) {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    };

    pub fn as_bytes(self) -> &'static [u8] {
        match self {
            LineEnding::Lf => b"\n",
            LineEnding::CrLf => b"\r\n",
            LineEnding::Cr => b"\r",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
            LineEnding::Cr => "CR",
        }
    }
}

/// How many line breaks of each style a text has. O(1) to get from a [`Rope`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineEndingCounts {
    pub lf: usize,
    pub crlf: usize,
    pub cr: usize,
}

impl LineEndingCounts {
    pub fn total(&self) -> usize {
        self.lf + self.crlf + self.cr
    }

    /// More than one style is present.
    pub fn is_mixed(&self) -> bool {
        [self.lf, self.crlf, self.cr]
            .iter()
            .filter(|&&n| n > 0)
            .count()
            > 1
    }

    /// The most common style, or `None` if there are no line breaks. Ties go to the
    /// platform style, then LF, CRLF, CR.
    pub fn dominant(&self) -> Option<LineEnding> {
        let count = |le| match le {
            LineEnding::Lf => self.lf,
            LineEnding::CrLf => self.crlf,
            LineEnding::Cr => self.cr,
        };
        let max = self.lf.max(self.crlf).max(self.cr);
        if max == 0 {
            return None;
        }
        [
            LineEnding::PLATFORM,
            LineEnding::Lf,
            LineEnding::CrLf,
            LineEnding::Cr,
        ]
        .into_iter()
        .find(|&le| count(le) == max)
    }
}

/// `text` with every line break replaced by `to`. O(n), so run it on a worker for large
/// texts; returns a clone in O(1) when there is nothing to change.
pub fn convert_line_endings(text: &Rope, to: LineEnding) -> Rope {
    let counts = text.line_ending_counts();
    let already = match to {
        LineEnding::Lf => counts.lf,
        LineEnding::CrLf => counts.crlf,
        LineEnding::Cr => counts.cr,
    };
    if already == counts.total() {
        return text.clone();
    }
    let mut builder = RopeBuilder::new();
    let mut out = Vec::with_capacity(64 * 1024);
    let mut after_cr = false;
    for chunk in text.chunks() {
        let mut rest = chunk;
        if after_cr && rest.first() == Some(&b'\n') {
            // The `\n` of a CRLF pair split across chunks; the `\r` was already converted.
            rest = &rest[1..];
        }
        after_cr = false;
        while let Some(i) = rest.iter().position(|&b| b == b'\r' || b == b'\n') {
            out.extend_from_slice(&rest[..i]);
            out.extend_from_slice(to.as_bytes());
            let crlf = rest[i] == b'\r' && rest.get(i + 1) == Some(&b'\n');
            if rest[i] == b'\r' && i + 1 == rest.len() {
                after_cr = true;
            }
            rest = &rest[i + 1 + usize::from(crlf)..];
        }
        out.extend_from_slice(rest);
        if out.len() >= 60 * 1024 {
            builder.push(&out);
            out.clear();
        }
    }
    builder.push(&out);
    builder.finish()
}

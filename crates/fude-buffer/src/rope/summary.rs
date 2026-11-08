//! Per-child summaries: byte length and line breaks.

/// Cached facts about a run of bytes.
///
/// Joining the summaries of two adjacent runs gives the summary of both together, including
/// a CRLF pair split across the seam, so parents can cache one per child and line lookups
/// never rescan more than one leaf.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Summary {
    pub bytes: usize,
    /// `\n` not preceded by `\r`.
    pub lf: usize,
    /// `\r\n` pairs.
    pub crlf: usize,
    /// `\r` not followed by `\n`. A trailing `\r` counts here until a join pairs it up.
    pub cr: usize,
    pub starts_with_lf: bool,
    pub ends_with_cr: bool,
}

impl Summary {
    pub fn of(bytes: &[u8]) -> Summary {
        let lf = count(bytes, b'\n');
        let cr = count(bytes, b'\r');
        let crlf = if lf == 0 || cr == 0 {
            0
        } else {
            count_crlf(bytes)
        };
        Summary {
            bytes: bytes.len(),
            lf: lf - crlf,
            crlf,
            cr: cr - crlf,
            starts_with_lf: bytes.first() == Some(&b'\n'),
            ends_with_cr: bytes.last() == Some(&b'\r'),
        }
    }

    /// Line breaks, counting CRLF as one. Equivalently: bytes that start a line break.
    pub fn breaks(&self) -> usize {
        self.lf + self.crlf + self.cr
    }

    /// The summary of `self` followed by `next`.
    pub fn join(self, next: Summary) -> Summary {
        if self.bytes == 0 {
            return next;
        }
        if next.bytes == 0 {
            return self;
        }
        let seam = usize::from(self.ends_with_cr && next.starts_with_lf);
        Summary {
            bytes: self.bytes + next.bytes,
            lf: self.lf + next.lf - seam,
            crlf: self.crlf + next.crlf + seam,
            cr: self.cr + next.cr - seam,
            starts_with_lf: self.starts_with_lf,
            ends_with_cr: next.ends_with_cr,
        }
    }
}

// Both counters sum blocks of 255 bytes into a `u8`, which can't overflow and lets the
// compiler vectorize the inner loop with byte-wide lanes.

fn count(bytes: &[u8], needle: u8) -> usize {
    bytes
        .chunks(255)
        .map(|block| block.iter().fold(0u8, |n, &b| n + u8::from(b == needle)) as usize)
        .sum()
}

fn count_crlf(bytes: &[u8]) -> usize {
    let (cr, lf) = (&bytes[..bytes.len() - 1], &bytes[1..]);
    cr.chunks(255)
        .zip(lf.chunks(255))
        .map(|(a, b)| {
            a.iter()
                .zip(b)
                .fold(0u8, |n, (&x, &y)| n + u8::from(x == b'\r' && y == b'\n'))
                as usize
        })
        .sum()
}

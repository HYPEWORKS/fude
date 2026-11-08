//! UTF-8 over chunked bytes.

use std::str;

use crate::Rope;

/// Length of the sequence a lead byte starts, or `None` for a byte that can't start one.
fn sequence_len(lead: u8) -> Option<usize> {
    match lead {
        0x00..=0x7F => Some(1),
        0xC2..=0xDF => Some(2),
        0xE0..=0xEF => Some(3),
        0xF0..=0xF4 => Some(4),
        _ => None,
    }
}

/// Where a trailing incomplete UTF-8 sequence starts, or `bytes.len()` if there is none.
/// Processing `bytes[..cut]` now and the rest with the next chunk keeps sequences whole.
pub(crate) fn complete_prefix_len(bytes: &[u8]) -> usize {
    let n = bytes.len();
    for back in 1..=n.min(3) {
        let b = bytes[n - back];
        if b & 0xC0 == 0x80 {
            continue; // continuation byte: keep looking for the lead
        }
        return match sequence_len(b) {
            Some(len) if len > back => n - back,
            _ => n,
        };
    }
    n
}

/// Counts in a stream of bytes, for deciding whether it is (mostly) UTF-8.
#[derive(Default, Debug)]
pub(crate) struct Utf8Stats {
    /// Valid multi-byte sequences.
    pub multibyte: usize,
    /// Invalid sequences.
    pub invalid: usize,
}

impl Utf8Stats {
    /// Counts `bytes`, which should end on a sequence boundary (see [`complete_prefix_len`]).
    pub fn scan(&mut self, mut bytes: &[u8]) {
        if bytes.is_ascii() {
            return;
        }
        loop {
            match str::from_utf8(bytes) {
                Ok(s) => {
                    self.multibyte += count_leads(s.as_bytes());
                    return;
                }
                Err(e) => {
                    let (valid, rest) = bytes.split_at(e.valid_up_to());
                    self.multibyte += count_leads(valid);
                    self.invalid += 1;
                    bytes = &rest[e.error_len().unwrap_or(rest.len())..];
                }
            }
        }
    }
}

fn count_leads(valid: &[u8]) -> usize {
    valid.iter().filter(|&&b| b >= 0xC0).count()
}

/// The offset of the first byte that isn't part of valid UTF-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InvalidUtf8 {
    pub offset: usize,
}

/// Calls `f` with the text of `rope` as `&str` pieces (borrowed from the rope where
/// possible) and the offset of each, in order. Stops at the first invalid byte.
pub(crate) fn for_each_str<E: From<InvalidUtf8>>(
    rope: &Rope,
    mut f: impl FnMut(&str, usize) -> Result<(), E>,
) -> Result<(), E> {
    // A sequence split across chunks is assembled here.
    let mut carry = [0u8; 4];
    let mut carried = 0;
    let mut offset = 0;
    for chunk in rope.chunks() {
        let mut rest = chunk;
        if carried > 0 {
            let need = sequence_len(carry[0]).expect("only lead bytes are carried");
            let take = (need - carried).min(rest.len());
            carry[carried..carried + take].copy_from_slice(&rest[..take]);
            carried += take;
            rest = &rest[take..];
            if carried < need {
                continue;
            }
            let s = str::from_utf8(&carry[..need]).map_err(|_| InvalidUtf8 { offset })?;
            f(s, offset)?;
            offset += need;
        }
        let cut = complete_prefix_len(rest);
        let s = str::from_utf8(&rest[..cut]).map_err(|e| InvalidUtf8 {
            offset: offset + e.valid_up_to(),
        })?;
        if !s.is_empty() {
            f(s, offset)?;
        }
        offset += cut;
        carry[..rest.len() - cut].copy_from_slice(&rest[cut..]);
        carried = rest.len() - cut;
    }
    if carried > 0 {
        return Err(InvalidUtf8 { offset }.into());
    }
    Ok(())
}

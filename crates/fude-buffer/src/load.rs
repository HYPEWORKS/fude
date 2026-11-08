//! Reading files: encoding detection, decoding, line ending detection.
//!
//! Detection order: a byte order mark; then UTF-16 without one (lots of zero bytes on one
//! side); then UTF-8, which is checked while the file streams in; and only if that fails,
//! `chardetng` (Firefox's detector) for legacy encodings such as Shift_JIS or windows-1252.
//!
//! UTF-8 is stored raw, invalid bytes and all. Other encodings are decoded to UTF-8, and
//! [`Loaded::exact`] says whether saving the unedited text would reproduce the file.

use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::path::Path;
use std::time::SystemTime;

use chardetng::{EncodingDetector, Iso2022JpDetection, Utf8Detection};
use encoding_rs::{CoderResult, Decoder, Encoding, UTF_8, UTF_16BE, UTF_16LE};

use crate::format::{is_utf8, is_utf16};
use crate::utf8::{Utf8Stats, complete_prefix_len};
use crate::{Format, LineEnding, Rope, RopeBuilder};

/// Read size. Also how much the detectors see before deciding on UTF-16.
const CHUNK: usize = 1 << 20;
/// How much of the start is checked for zero bytes (binary files, BOM-less UTF-16).
const SNIFF: usize = 64 * 1024;
/// How much non-UTF-8 text `chardetng` sees at most.
const DETECT_SAMPLE: usize = 1 << 20;

/// A file's text and how it was stored.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// UTF-8 (possibly with invalid bytes, if the file was "UTF-8 with errors").
    pub text: Rope,
    pub format: Format,
    /// Saving the unedited text in `format` reproduces the file byte for byte. False when
    /// decoding replaced malformed bytes, or the encoding has several byte sequences for
    /// one character.
    pub exact: bool,
    /// The start of the file contains zero bytes, which text almost never does.
    pub binary: bool,
    /// Size and modification time when the file was opened, for noticing outside changes.
    pub stamp: Option<FileStamp>,
}

/// What a file looked like at some point, to tell whether it changed since.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

impl FileStamp {
    pub fn of(path: &Path) -> io::Result<FileStamp> {
        Ok(FileStamp::from_metadata(&std::fs::metadata(path)?))
    }

    pub(crate) fn from_metadata(meta: &Metadata) -> FileStamp {
        FileStamp {
            len: meta.len(),
            modified: meta.modified().ok(),
        }
    }
}

/// Opens a file, detecting its encoding.
pub fn load(path: &Path) -> io::Result<Loaded> {
    load_with(path, None)
}

/// Opens a file, in `encoding` if given (a BOM for that encoding is still recognized),
/// otherwise detecting it.
pub fn load_with(path: &Path, encoding: Option<&'static Encoding>) -> io::Result<Loaded> {
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::IsADirectory,
            "is a directory",
        ));
    }
    let mut loaded = read(file, encoding)?;
    loaded.stamp = Some(FileStamp::from_metadata(&meta));
    Ok(loaded)
}

/// Decodes bytes already in memory, like [`load_with`].
pub fn decode(bytes: &[u8], encoding: Option<&'static Encoding>) -> Loaded {
    read(bytes, encoding).expect("reading from memory doesn't fail")
}

/// Reads and decodes a stream, like [`load_with`].
pub fn read(mut input: impl Read, encoding: Option<&'static Encoding>) -> io::Result<Loaded> {
    let mut buf = vec![0u8; CHUNK + 4];
    let n = read_full(&mut input, &mut buf[..CHUNK])?;
    let first = &buf[..n];

    let bom = Encoding::for_bom(first);
    let (chosen, bom_len) = match (encoding, bom) {
        (Some(forced), Some((found, len))) if forced == found => (Some(forced), len),
        (Some(forced), _) => (Some(forced), 0),
        (None, Some((found, len))) => (Some(found), len),
        (None, None) => (guess_utf16(&first[..n.min(SNIFF)]), 0),
    };
    let binary = !chosen.is_some_and(is_utf16) && first[..n.min(SNIFF)].contains(&0);

    let mut reader = Reader {
        input,
        buf,
        filled: n,
        start: bom_len,
        eof: n < CHUNK,
    };
    let (text, encoding, exact) = match chosen {
        Some(enc) if is_utf16(enc) => {
            let (text, replaced) = reader.decode_stream(enc)?;
            (text, enc, !replaced)
        }
        Some(enc) if is_utf8(enc) => (reader.raw(None)?, UTF_8, true),
        Some(enc) => {
            let raw = reader.raw(None)?;
            let (text, exact) = transcode(&raw, enc);
            (text, enc, exact)
        }
        None => {
            let mut detect = Detect::default();
            let raw = reader.raw(Some(&mut detect))?;
            match detect.legacy_guess(binary) {
                Some(enc) => {
                    let (text, exact) = transcode(&raw, enc);
                    (text, enc, exact)
                }
                None => (raw, UTF_8, true),
            }
        }
    };

    let line_ending = text
        .line_ending_counts()
        .dominant()
        .unwrap_or(LineEnding::PLATFORM);
    Ok(Loaded {
        text,
        format: Format {
            encoding,
            bom: bom_len > 0,
            line_ending,
        },
        exact,
        binary,
        stamp: None,
    })
}

/// Reads until `buf` is full or the input ends.
fn read_full(input: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match input.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(m) => n += m,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// The input, with the first chunk already read into `buf`.
struct Reader<R> {
    input: R,
    buf: Vec<u8>,
    /// `buf[start..filled]` is unprocessed.
    filled: usize,
    start: usize,
    eof: bool,
}

impl<R: Read> Reader<R> {
    /// Calls `f` with each piece of input in order, holding back an incomplete UTF-8
    /// sequence at the end of a piece so it arrives whole with the next one.
    fn for_each(&mut self, mut f: impl FnMut(&[u8])) -> io::Result<()> {
        loop {
            let data = &self.buf[self.start..self.filled];
            let cut = if self.eof {
                data.len()
            } else {
                complete_prefix_len(data)
            };
            f(&data[..cut]);
            if self.eof {
                return Ok(());
            }
            // Move the held-back tail to the front and read after it.
            let tail = data.len() - cut;
            self.buf.copy_within(self.start + cut..self.filled, 0);
            let m = read_full(&mut self.input, &mut self.buf[tail..tail + CHUNK])?;
            self.start = 0;
            self.filled = tail + m;
            self.eof = m < CHUNK;
        }
    }

    /// The bytes as they are, optionally gathering statistics for encoding detection.
    fn raw(&mut self, mut detect: Option<&mut Detect>) -> io::Result<Rope> {
        let mut builder = RopeBuilder::new();
        self.for_each(|piece| {
            if let Some(d) = detect.as_deref_mut() {
                d.scan(piece);
            }
            builder.push(piece);
        })?;
        Ok(builder.finish())
    }

    /// Decodes as it reads. Returns the text and whether anything had to be replaced.
    fn decode_stream(&mut self, encoding: &'static Encoding) -> io::Result<(Rope, bool)> {
        let mut out = Decode::new(encoding);
        self.for_each(|piece| out.push(piece, false))?;
        Ok(out.finish())
    }
}

/// Statistics gathered from raw input to choose between UTF-8 and a legacy encoding.
#[derive(Default)]
struct Detect {
    stats: Utf8Stats,
    /// Started at the first invalid UTF-8 and fed a bounded sample from there.
    detector: Option<EncodingDetector>,
    fed: usize,
}

impl Detect {
    fn scan(&mut self, piece: &[u8]) {
        let invalid_before = self.stats.invalid;
        self.stats.scan(piece);
        if self.detector.is_none() && self.stats.invalid > invalid_before {
            self.detector = Some(EncodingDetector::new(Iso2022JpDetection::Deny));
        }
        if let Some(d) = &mut self.detector
            && self.fed < DETECT_SAMPLE
        {
            let n = piece.len().min(DETECT_SAMPLE - self.fed);
            d.feed(&piece[..n], false);
            self.fed += n;
        }
    }

    /// A legacy encoding to decode with, or `None` to keep the bytes as UTF-8.
    fn legacy_guess(self, binary: bool) -> Option<&'static Encoding> {
        let stats = &self.stats;
        // Valid UTF-8, or mostly UTF-8 with some damage (a log with stray bytes): keep it
        // raw, so the damage is visible and survives saving. Binary files stay raw too.
        if stats.invalid == 0 || stats.multibyte >= stats.invalid * 4 || binary {
            return None;
        }
        let mut detector = self.detector?;
        detector.feed(&[], true);
        let guess = detector.guess(None, Utf8Detection::Deny);
        (!is_utf8(guess)).then_some(guess)
    }
}

/// Decodes into a rope.
struct Decode {
    decoder: Decoder,
    builder: RopeBuilder,
    out: Vec<u8>,
    replaced: bool,
}

impl Decode {
    fn new(encoding: &'static Encoding) -> Decode {
        Decode {
            decoder: encoding.new_decoder_without_bom_handling(),
            builder: RopeBuilder::new(),
            out: vec![0; 64 * 1024],
            replaced: false,
        }
    }

    fn push(&mut self, mut input: &[u8], last: bool) {
        loop {
            let (result, read, written, replaced) =
                self.decoder.decode_to_utf8(input, &mut self.out, last);
            self.builder.push(&self.out[..written]);
            self.replaced |= replaced;
            input = &input[read..];
            if result == CoderResult::InputEmpty {
                return;
            }
        }
    }

    fn finish(mut self) -> (Rope, bool) {
        self.push(&[], true);
        (self.builder.finish(), self.replaced)
    }
}

/// Decodes raw bytes in a legacy encoding. Returns the text and whether it re-encodes to
/// exactly the original bytes.
fn transcode(raw: &Rope, encoding: &'static Encoding) -> (Rope, bool) {
    let mut decode = Decode::new(encoding);
    for chunk in raw.chunks() {
        decode.push(chunk, false);
    }
    let (text, replaced) = decode.finish();
    let exact = !replaced && crate::save::encodes_to(&text, encoding, raw);
    (text, exact)
}

/// BOM-less UTF-16: mostly-ASCII text has a zero in every other byte.
fn guess_utf16(sample: &[u8]) -> Option<&'static Encoding> {
    if sample.len() < 4 {
        return None;
    }
    let pairs = sample.len() / 2;
    let zeros = |parity: usize| {
        sample
            .iter()
            .skip(parity)
            .step_by(2)
            .filter(|&&b| b == 0)
            .count()
    };
    let (even, odd) = (zeros(0), zeros(1));
    let mostly = |n: usize| n * 10 >= pairs * 4;
    let rarely = |n: usize| n * 20 <= pairs;
    if mostly(odd) && rarely(even) {
        Some(UTF_16LE)
    } else if mostly(even) && rarely(odd) {
        Some(UTF_16BE)
    } else {
        None
    }
}

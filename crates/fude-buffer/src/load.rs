//! Reading files: encoding detection, decoding, line ending detection.
//!
//! Detection order: a byte order mark; then UTF-16 without one (lots of zero bytes on one
//! side); then UTF-8, which is checked while the file streams in; and only if that fails,
//! `chardetng` (Firefox's detector) for legacy encodings such as Shift_JIS or windows-1252.
//!
//! UTF-8 is stored raw, invalid bytes and all. Other encodings are decoded to UTF-8, and
//! [`Loaded::exact`] says whether saving the unedited text would reproduce the file.
//!
//! [`load_observed`] is for loading on a worker thread: it reports a preview of the start
//! of the file before reading the rest, then progress, and can be cancelled.

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
/// Invalid UTF-8 sequences in the first megabyte that settle it's in a legacy encoding.
const EARLY_INVALID: usize = 64;
/// How much of the start of a file a preview shows: a few screens.
const PREVIEW: usize = 64 * 1024;

/// Follows a load running on another thread, and can stop it.
pub trait LoadObserver {
    /// The start of the file, decoded, before the rest is read. Only for files longer than
    /// one read (1 MB). It ends at a line break when there is one, and its text is a prefix
    /// of the final text unless the rest of the file changes the encoding detected.
    fn preview(&mut self, _preview: Loaded) {}
    /// How many bytes of the file have been read so far.
    fn progress(&mut self, _read: u64) {}
    /// Checked between reads; true stops the load with [`io::ErrorKind::Interrupted`].
    fn cancelled(&self) -> bool {
        false
    }
}

/// Watches nothing: a plain load.
struct Unobserved;

impl LoadObserver for Unobserved {}

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
    load_observed(path, encoding, &mut Unobserved)
}

/// Like [`load_with`], reporting a preview and progress to `observer` as it goes.
pub fn load_observed(
    path: &Path,
    encoding: Option<&'static Encoding>,
    observer: &mut dyn LoadObserver,
) -> io::Result<Loaded> {
    let file = File::open(path)?;
    let meta = file.metadata()?;
    if meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::IsADirectory,
            "is a directory",
        ));
    }
    let mut loaded = read_observed(file, encoding, observer)?;
    loaded.stamp = Some(FileStamp::from_metadata(&meta));
    Ok(loaded)
}

/// Decodes bytes already in memory, like [`load_with`].
pub fn decode(bytes: &[u8], encoding: Option<&'static Encoding>) -> Loaded {
    read(bytes, encoding).expect("reading from memory doesn't fail")
}

/// Reads and decodes a stream, like [`load_with`].
pub fn read(input: impl Read, encoding: Option<&'static Encoding>) -> io::Result<Loaded> {
    read_observed(input, encoding, &mut Unobserved)
}

fn read_observed(
    mut input: impl Read,
    encoding: Option<&'static Encoding>,
    observer: &mut dyn LoadObserver,
) -> io::Result<Loaded> {
    let mut buf = vec![0u8; CHUNK + 4];
    let n = read_full(&mut input, &mut buf[..CHUNK])?;
    let first = &buf[..n];
    if n == CHUNK {
        observer.preview(preview(first, encoding));
    }
    observer.progress(n as u64);

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
        read: n as u64,
        observer,
    };
    let (text, encoding, exact) = match chosen {
        Some(enc) if is_utf8(enc) => (reader.raw()?, UTF_8, true),
        Some(enc) => {
            // UTF-16 round-trips whenever nothing was replaced; legacy encodings are
            // checked by encoding back.
            let (text, exact) = reader.decode_stream(enc, !is_utf16(enc))?;
            (text, enc, exact)
        }
        None => reader.detect(binary)?,
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

/// The first few screens of a file, from its first read: cut at a line break, so the
/// last line isn't cut short (unless the file starts with one very long line).
fn preview(first: &[u8], encoding: Option<&'static Encoding>) -> Loaded {
    let sample = &first[..first.len().min(PREVIEW)];
    let utf16 = encoding
        .or_else(|| Encoding::for_bom(first).map(|(e, _)| e))
        .or_else(|| guess_utf16(&first[..first.len().min(SNIFF)]))
        .is_some_and(is_utf16);
    // In UTF-8 and the legacy encodings, a 0x0A byte is always a line feed, never part
    // of a character; cutting there keeps characters whole for the detectors too.
    let sample = if utf16 {
        &sample[..sample.len() & !1]
    } else {
        match sample.iter().rposition(|&b| b == b'\n') {
            Some(i) => &sample[..=i],
            None => &sample[..complete_prefix_len(sample)],
        }
    };
    let mut preview = decode(sample, encoding);
    if utf16 && preview.text.line_count() > 1 {
        let last = preview.text.line_to_byte(preview.text.line_count() - 1);
        preview.text = preview.text.slice(0..last);
    }
    preview
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
struct Reader<'o, R> {
    input: R,
    buf: Vec<u8>,
    /// `buf[start..filled]` is unprocessed.
    filled: usize,
    start: usize,
    eof: bool,
    /// Bytes read so far.
    read: u64,
    observer: &'o mut dyn LoadObserver,
}

impl<R: Read> Reader<'_, R> {
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
            if self.observer.cancelled() {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let m = read_full(&mut self.input, &mut self.buf[tail..tail + CHUNK])?;
            self.start = 0;
            self.filled = tail + m;
            self.eof = m < CHUNK;
            self.read += m as u64;
            self.observer.progress(self.read);
        }
    }

    /// The bytes as they are.
    fn raw(&mut self) -> io::Result<Rope> {
        let mut builder = RopeBuilder::new();
        self.for_each(|piece| builder.push(piece))?;
        Ok(builder.finish())
    }

    /// Detects UTF-8 or a legacy encoding while reading. The text stays raw (UTF-8) unless
    /// the first megabyte is clearly in a legacy encoding; from then on it's decoded as it
    /// streams in, so the raw bytes are never all held at once. A file that only turns out
    /// not to be UTF-8 further in is decoded at the end.
    fn detect(&mut self, binary: bool) -> io::Result<(Rope, &'static Encoding, bool)> {
        let mut detect = Detect::default();
        let mut raw = RopeBuilder::new();
        let mut decode: Option<(Decode, &'static Encoding)> = None;
        self.for_each(|piece| {
            if let Some((d, _)) = &mut decode {
                d.push(piece, false);
                return;
            }
            detect.scan(piece);
            raw.push(piece);
            if detect.scanned >= CHUNK
                && let Some(enc) = detect.legacy_guess(binary, true)
            {
                let mut d = Decode::checked(enc);
                for chunk in std::mem::take(&mut raw).finish().chunks() {
                    d.push(chunk, false);
                }
                decode = Some((d, enc));
            }
        })?;
        if let Some((d, enc)) = decode {
            let (text, exact) = d.finish();
            return Ok((text, enc, exact));
        }
        let raw = raw.finish();
        Ok(match detect.legacy_guess(binary, false) {
            Some(enc) => {
                let (text, exact) = transcode(&raw, enc);
                (text, enc, exact)
            }
            None => (raw, UTF_8, true),
        })
    }

    /// Decodes as it reads. Returns the text and whether it round-trips exactly (see
    /// [`Decode::finish`]).
    fn decode_stream(
        &mut self,
        encoding: &'static Encoding,
        checked: bool,
    ) -> io::Result<(Rope, bool)> {
        let mut out = if checked {
            Decode::checked(encoding)
        } else {
            Decode::new(encoding)
        };
        self.for_each(|piece| out.push(piece, false))?;
        Ok(out.finish())
    }
}

/// Statistics gathered from raw input to choose between UTF-8 and a legacy encoding.
#[derive(Default)]
struct Detect {
    stats: Utf8Stats,
    /// Up to [`DETECT_SAMPLE`] bytes from around the first invalid UTF-8, for `chardetng`.
    sample: Option<Vec<u8>>,
    /// Bytes scanned so far.
    scanned: usize,
}

impl Detect {
    fn scan(&mut self, piece: &[u8]) {
        let invalid_before = self.stats.invalid;
        self.stats.scan(piece);
        let mut from = 0;
        if self.sample.is_none() && self.stats.invalid > invalid_before {
            // Start at a line, unless this is the start of the file: pieces can begin in
            // the middle of a character.
            if self.scanned > 0 {
                from = piece.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1);
            }
            self.sample = Some(Vec::with_capacity(DETECT_SAMPLE.min(piece.len())));
        }
        if let Some(sample) = &mut self.sample {
            let rest = &piece[from..];
            let n = rest.len().min(DETECT_SAMPLE - sample.len());
            sample.extend_from_slice(&rest[..n]);
        }
        self.scanned += piece.len();
    }

    /// A legacy encoding to decode with, or `None` to keep the bytes as UTF-8. `early`
    /// asks before the whole file is seen: only a clear case counts.
    fn legacy_guess(&self, binary: bool, early: bool) -> Option<&'static Encoding> {
        let stats = &self.stats;
        // Valid UTF-8, or mostly UTF-8 with some damage (a log with stray bytes): keep it
        // raw, so the damage is visible and survives saving. Binary files stay raw too.
        if stats.invalid == 0 || stats.multibyte >= stats.invalid * 4 || binary {
            return None;
        }
        if early && stats.invalid < EARLY_INVALID {
            return None;
        }
        let sample = self.sample.as_ref()?;
        // A sample cut short ends at a line break: one cut mid-character would count
        // against the encoding it's in. (0x0A is never part of a character in the legacy
        // encodings.) A sample that reaches the end of the file is fed whole.
        let end = if sample.len() == DETECT_SAMPLE {
            sample
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(sample.len(), |i| i + 1)
        } else {
            sample.len()
        };
        let mut detector = EncodingDetector::new(Iso2022JpDetection::Deny);
        detector.feed(&sample[..end], true);
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
    /// Checks the text encodes back to the input, as it goes.
    check: Option<ExactCheck>,
}

impl Decode {
    fn new(encoding: &'static Encoding) -> Decode {
        Decode {
            decoder: encoding.new_decoder_without_bom_handling(),
            builder: RopeBuilder::new(),
            out: vec![0; 64 * 1024],
            replaced: false,
            check: None,
        }
    }

    /// Also checks that saving the text unedited would give back the input exactly.
    fn checked(encoding: &'static Encoding) -> Decode {
        Decode {
            check: Some(ExactCheck::new(encoding)),
            ..Decode::new(encoding)
        }
    }

    fn push(&mut self, mut input: &[u8], last: bool) {
        if let Some(check) = &mut self.check {
            check.input(input);
        }
        loop {
            let (result, read, written, replaced) =
                self.decoder.decode_to_utf8(input, &mut self.out, last);
            self.builder.push(&self.out[..written]);
            self.replaced |= replaced;
            input = &input[read..];
            let done = result == CoderResult::InputEmpty;
            if let Some(check) = &mut self.check {
                check.decoded(&self.out[..written], last && done);
            }
            if done {
                return;
            }
        }
    }

    /// The text, and whether it round-trips (nothing was replaced; for a checked decode,
    /// it also encodes back exactly). An unchecked decode answers the first part only:
    /// `!replaced`.
    fn finish(mut self) -> (Rope, bool) {
        self.push(&[], true);
        let exact = !self.replaced && self.check.is_none_or(ExactCheck::finish);
        (self.builder.finish(), exact)
    }
}

/// Encodes decoded text again as it comes, and compares it with the input: some
/// encodings have two byte sequences for one character, and decoding can't tell which
/// the file used.
struct ExactCheck {
    encoder: encoding_rs::Encoder,
    /// Input whose encoding hasn't been compared yet.
    pending: Vec<u8>,
    out: Vec<u8>,
    exact: bool,
}

impl ExactCheck {
    fn new(encoding: &'static Encoding) -> ExactCheck {
        ExactCheck {
            encoder: encoding.new_encoder(),
            pending: Vec::new(),
            out: vec![0; 64 * 1024],
            exact: true,
        }
    }

    fn input(&mut self, bytes: &[u8]) {
        if self.exact {
            self.pending.extend_from_slice(bytes);
        }
    }

    fn decoded(&mut self, utf8: &[u8], last: bool) {
        if !self.exact {
            return;
        }
        // The decoder writes whole characters.
        let Ok(mut s) = std::str::from_utf8(utf8) else {
            self.exact = false;
            return;
        };
        loop {
            let (result, read, written, unmappable) =
                self.encoder.encode_from_utf8(s, &mut self.out, last);
            let n = written.min(self.pending.len());
            if unmappable || written > self.pending.len() || self.out[..n] != self.pending[..n] {
                self.exact = false;
                self.pending = Vec::new();
                return;
            }
            self.pending.drain(..n);
            s = &s[read..];
            if result == CoderResult::InputEmpty {
                return;
            }
        }
    }

    fn finish(self) -> bool {
        self.exact && self.pending.is_empty()
    }
}

/// Decodes raw bytes in a legacy encoding. Returns the text and whether it re-encodes to
/// exactly the original bytes.
fn transcode(raw: &Rope, encoding: &'static Encoding) -> (Rope, bool) {
    let mut decode = Decode::new(encoding);
    for chunk in raw.chunks() {
        decode.push(chunk, false);
    }
    let (text, round_trips) = decode.finish();
    let exact = round_trips && crate::save::encodes_to(&text, encoding, raw);
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

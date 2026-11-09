//! The large-file corpus (ARCHITECTURE.md §16): generated, never committed.
//!
//! Every file is made from a fixed seed, so runs compare like with like. `small` makes
//! each file a tenth of the size, for quick runs.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

/// Bump when a generator changes, so old corpora are regenerated.
const VERSION: u32 = 1;

pub struct Spec {
    pub name: &'static str,
    /// Full-size length in MB.
    mb: u64,
    pub what: &'static str,
    generate: fn(&mut dyn Write, u64) -> io::Result<()>,
}

pub const FILES: &[Spec] = &[
    Spec {
        name: "log.log",
        mb: 500,
        what: "a log: millions of short lines",
        generate: log,
    },
    Spec {
        name: "minified.json",
        mb: 100,
        what: "minified JSON: one huge line",
        generate: minified_json,
    },
    Spec {
        name: "long-lines.csv",
        mb: 100,
        what: "CSV rows of 50 KB each",
        generate: long_lines,
    },
    Spec {
        name: "japanese.txt",
        mb: 50,
        what: "Japanese prose, UTF-8",
        generate: japanese,
    },
    Spec {
        name: "shift_jis.txt",
        mb: 50,
        what: "Japanese prose in Shift_JIS, no BOM",
        generate: shift_jis,
    },
    Spec {
        name: "utf16le.txt",
        mb: 50,
        what: "mixed-script text, UTF-16LE with BOM",
        generate: utf16le,
    },
    Spec {
        name: "invalid-utf8.log",
        mb: 100,
        what: "a UTF-8 log with stray invalid bytes",
        generate: invalid_utf8,
    },
    Spec {
        name: "mixed-endings.txt",
        mb: 50,
        what: "LF, CRLF and CR line endings mixed",
        generate: mixed_endings,
    },
    Spec {
        name: "code.rs",
        mb: 20,
        what: "Rust, above the whole-file highlighting limit (16 MB)",
        generate: code,
    },
];

/// Generates whatever is missing or stale; returns the files' paths.
pub fn ensure(dir: &Path, small: bool) -> Result<Vec<PathBuf>, String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stamp = dir.join(".version");
    let current = fs::read_to_string(&stamp)
        .ok()
        .and_then(|s| s.trim().parse().ok());
    let stale = current != Some(VERSION);
    let mut paths = Vec::new();
    for spec in FILES {
        let path = dir.join(spec.name);
        let len = (spec.mb << 20) / if small { 10 } else { 1 };
        if stale || !path.exists() {
            eprintln!("generating {} ({} MB)…", path.display(), len >> 20);
            let tmp = path.with_extension("partial");
            let mut out =
                BufWriter::with_capacity(1 << 20, File::create(&tmp).map_err(|e| e.to_string())?);
            (spec.generate)(&mut out, len)
                .and_then(|()| out.flush())
                .map_err(|e| format!("{}: {e}", tmp.display()))?;
            drop(out);
            fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        }
        paths.push(path);
    }
    fs::write(&stamp, format!("{VERSION}\n")).map_err(|e| e.to_string())?;
    Ok(paths)
}

/// xorshift64*: deterministic and fast.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

/// Writes lines from `line` until `len` bytes are written.
fn lines(
    out: &mut dyn Write,
    len: u64,
    mut line: impl FnMut(&mut Rng, &mut Vec<u8>),
) -> io::Result<()> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut buf = Vec::with_capacity(1 << 16);
    let mut written = 0u64;
    while written < len {
        buf.clear();
        line(&mut rng, &mut buf);
        out.write_all(&buf)?;
        written += buf.len() as u64;
    }
    Ok(())
}

const WORDS: &[&str] = &[
    "the",
    "quick",
    "brown",
    "fox",
    "jumps",
    "over",
    "lazy",
    "dog",
    "request",
    "worker",
    "cache",
    "miss",
    "retry",
    "timeout",
    "connection",
    "session",
    "token",
    "user",
    "parse",
    "error",
    "value",
    "index",
    "buffer",
    "stream",
    "window",
    "render",
    "frame",
];

const JAPANESE: &[&str] = &[
    "吾輩は猫である。",
    "名前はまだ無い。",
    "どこで生れたかとんと見当がつかぬ。",
    "何でも薄暗いじめじめした所でニャーニャー泣いていた事だけは記憶している。",
    "春はあけぼの。",
    "やうやう白くなりゆく山ぎは、",
    "すこしあかりて、",
    "紫だちたる雲のほそくたなびきたる。",
    "東京都の天気は晴れ、",
    "最高気温は二十三度です。",
    "テキストエディタ「筆」は速くて軽い。",
    "日本語の文章を表示するテスト。",
];

fn words(rng: &mut Rng, n: u64, out: &mut Vec<u8>) {
    for i in 0..n {
        if i > 0 {
            out.push(b' ');
        }
        out.extend_from_slice(rng.pick(WORDS).as_bytes());
    }
}

fn log(out: &mut dyn Write, len: u64) -> io::Result<()> {
    let levels = ["INFO ", "DEBUG", "WARN ", "ERROR"];
    lines(out, len, |rng, b| {
        let n = rng.below(1_000_000);
        let _ = write!(
            b,
            "2026-11-08T03:{:02}:{:02}.{:03}Z {} worker-{} request {} took {} ms: ",
            n % 60,
            (n / 60) % 60,
            n % 1000,
            levels[(n % 4) as usize],
            n % 32,
            n,
            n % 977,
        );
        let count = rng.below(8);
        words(rng, count, b);
        b.push(b'\n');
    })
}

fn invalid_utf8(out: &mut dyn Write, len: u64) -> io::Result<()> {
    let mut i = 0u64;
    lines(out, len, |rng, b| {
        i += 1;
        let _ = write!(b, "{i:08} café naïve résumé ");
        let count = 2 + rng.below(6);
        words(rng, count, b);
        // A stray byte now and then, as from a crash mid-write or a wrong encoding.
        if i.is_multiple_of(400) {
            b.extend_from_slice(b" \xff\xfe broken");
        }
        b.push(b'\n');
    })
}

fn mixed_endings(out: &mut dyn Write, len: u64) -> io::Result<()> {
    let endings: [&[u8]; 3] = [b"\n", b"\r\n", b"\r"];
    lines(out, len, |rng, b| {
        let count = 3 + rng.below(10);
        words(rng, count, b);
        b.extend_from_slice(rng.pick(&endings));
    })
}

fn minified_json(out: &mut dyn Write, len: u64) -> io::Result<()> {
    let mut rng = Rng(7);
    let mut written = 1u64;
    out.write_all(b"[")?;
    let mut buf = Vec::new();
    let mut id = 0u64;
    while written + 2 < len {
        buf.clear();
        id += 1;
        let _ = write!(
            buf,
            "{}{{\"id\":{id},\"name\":\"{}\",\"tags\":[\"{}\",\"{}\"],\"score\":{}.{:02},\"text\":\"",
            if id > 1 { "," } else { "" },
            rng.pick(WORDS),
            rng.pick(WORDS),
            rng.pick(WORDS),
            rng.below(100),
            rng.below(100),
        );
        let count = 3 + rng.below(12);
        words(&mut rng, count, &mut buf);
        buf.extend_from_slice(b"\"}");
        out.write_all(&buf)?;
        written += buf.len() as u64;
    }
    out.write_all(b"]\n")
}

fn long_lines(out: &mut dyn Write, len: u64) -> io::Result<()> {
    let per_line = 50 * 1024;
    let mut row = 0u64;
    lines(out, len, |rng, b| {
        row += 1;
        let _ = write!(b, "{row}");
        while b.len() < per_line {
            let _ = write!(
                b,
                ",{},{}.{:03}",
                rng.pick(WORDS),
                rng.below(10_000),
                rng.below(1000)
            );
        }
        b.push(b'\n');
    })
}

fn japanese(out: &mut dyn Write, len: u64) -> io::Result<()> {
    lines(out, len, |rng, b| {
        let count = 1 + rng.below(5);
        for _ in 0..count {
            b.extend_from_slice(rng.pick(JAPANESE).as_bytes());
        }
        b.push(b'\n');
    })
}

fn shift_jis(out: &mut dyn Write, len: u64) -> io::Result<()> {
    let mut encoder = encoding_rs::SHIFT_JIS.new_encoder();
    let mut text = Vec::new();
    japanese(&mut text, len * 3 / 2)?;
    let text = String::from_utf8(text).expect("generated UTF-8");
    let mut encoded = vec![0u8; text.len() + 16];
    let (_, _, written, _) = encoder.encode_from_utf8(&text, &mut encoded, true);
    // Cut at a line break, so no character is cut in half.
    let cut = &encoded[..written.min(len as usize)];
    let end = cut
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(cut.len(), |i| i + 1);
    out.write_all(&cut[..end])
}

fn utf16le(out: &mut dyn Write, len: u64) -> io::Result<()> {
    out.write_all(&[0xFF, 0xFE])?;
    let mut rng = Rng(11);
    let mut written = 2u64;
    let mut line = String::new();
    while written < len {
        line.clear();
        let count = 2 + rng.below(8);
        for i in 0..count {
            if i > 0 {
                line.push(' ');
            }
            line.push_str(match rng.below(4) {
                0 => rng.pick(JAPANESE),
                1 => "Ελληνικά καλημέρα",
                _ => rng.pick(WORDS),
            });
        }
        line.push_str("\r\n");
        let bytes: Vec<u8> = line.encode_utf16().flat_map(u16::to_le_bytes).collect();
        out.write_all(&bytes)?;
        written += bytes.len() as u64;
    }
    Ok(())
}

fn code(out: &mut dyn Write, len: u64) -> io::Result<()> {
    let mut n = 0u64;
    lines(out, len, |rng, b| {
        n += 1;
        let name = rng.pick(WORDS);
        let _ = write!(
            b,
            "/// Computes the {name} for item {n}.\n\
             pub fn {name}_{n}(input: &[u8], limit: usize) -> Result<Vec<u32>, String> {{\n    \
                 let mut out = Vec::with_capacity(limit);\n    \
                 for (i, &b) in input.iter().enumerate().take(limit) {{\n        \
                     if b == b'\\n' {{\n            \
                         return Err(format!(\"line break at {{i}} in {name}\"));\n        \
                     }}\n        \
                     out.push(u32::from(b) * {} + 0x{:x});\n    \
                 }}\n    \
                 Ok(out)\n\
             }}\n\n",
            rng.below(100),
            rng.below(0xffff),
        );
    })
}

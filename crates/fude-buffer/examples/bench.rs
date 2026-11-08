//! Large-file benchmark for the buffer, against the budgets in ARCHITECTURE.md §3.
//!
//!     cargo run --release -p fude-buffer --example bench -- [--size-mb 500]
//!
//! Generates a log-like file (once; it is reused) in target/bench-buffer, then measures
//! loading, memory, line lookups, edits with undo history, snapshots and saving.

use std::fs::{self, File};
use std::hint::black_box;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fude_buffer::{Buffer, Edit, load, save};

fn main() {
    let mut size_mb = 500u64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--size-mb" => {
                size_mb = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--size-mb N")
            }
            other => panic!("unknown argument {other:?}"),
        }
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/bench-buffer");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("log-{size_mb}mb.txt"));
    generate(&path, size_mb << 20);
    let file_len = fs::metadata(&path).unwrap().len();
    println!(
        "fude-buffer benchmark: {} ({:.0} MB)",
        path.display(),
        mb(file_len)
    );
    println!();

    // Loading, in a fresh process so memory comes from the OS like it would for a user.
    // (Run the benchmark twice for a warm page cache.)
    let rss_before = rss();
    let t = Instant::now();
    let loaded = load(&path).unwrap();
    let load_time = t.elapsed();
    let rss_after = rss();
    let text = &loaded.text;
    println!(
        "  load               {:>9}   {:.1} GB/s, {} lines, {}",
        ms(load_time),
        file_len as f64 / load_time.as_secs_f64() / 1e9,
        text.line_count(),
        loaded.format.encoding_label()
    );
    if let (Some(a), Some(b)) = (rss_before, rss_after) {
        println!(
            "  RAM for the text   {:>6.0} MB   {:.3}× the file size",
            mb(b - a),
            (b - a) as f64 / file_len as f64
        );
    }

    // Lookups at pseudo-random positions.
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    const LOOKUPS: u32 = 200_000;
    let lines = text.line_count() as u64;
    let t = Instant::now();
    for _ in 0..LOOKUPS {
        black_box(text.line_to_byte(rng.below(lines) as usize));
    }
    let line_to_byte = t.elapsed() / LOOKUPS;
    let t = Instant::now();
    for _ in 0..LOOKUPS {
        black_box(text.byte_to_line(rng.below(file_len) as usize));
    }
    let byte_to_line = t.elapsed() / LOOKUPS;
    println!("  line → byte        {:>9}", ns(line_to_byte));
    println!("  byte → line        {:>9}", ns(byte_to_line));

    // A screenful: 60 lines from a random spot, copied out as the view does.
    let t = Instant::now();
    let mut scratch = Vec::new();
    for _ in 0..1000 {
        let first = rng.below(lines.saturating_sub(60)) as usize;
        for span in text.lines_from(first).take(60) {
            scratch.clear();
            text.copy_range(span.start..span.end, &mut scratch);
            black_box(&scratch);
        }
    }
    println!("  visible lines (60) {:>9}", us(t.elapsed() / 1000));

    // Typing at random places, with full undo history kept. Each burst of 8 keys is one
    // undo step, like typing a word.
    let mut buffer: Buffer<()> = Buffer::new(loaded.text.clone(), loaded.format);
    drop(loaded);
    let rss_before = rss();
    const KEYS: u32 = 100_000;
    let mut worst = Duration::ZERO;
    let t = Instant::now();
    let mut at = 0;
    for i in 0..KEYS {
        if i % 8 == 0 {
            buffer.seal();
            at = rng.below(buffer.text().len() as u64) as usize;
        }
        let key = Instant::now();
        buffer.edit(&[Edit::insert(at, b"x")], (), (), true);
        worst = worst.max(key.elapsed());
        at += 1;
    }
    let per_key = t.elapsed() / KEYS;
    let history = rss().zip(rss_before).map(|(a, b)| a.saturating_sub(b));
    println!(
        "  type a key         {:>9}   worst {}",
        us(per_key),
        us(worst)
    );
    if let Some(bytes) = history {
        println!(
            "  undo history       {:>6.1} KB   per step ({} steps)",
            bytes as f64 / 1024.0 / f64::from(KEYS / 8),
            KEYS / 8
        );
    }

    let t = Instant::now();
    for _ in 0..1000 {
        black_box(buffer.snapshot());
    }
    println!("  snapshot           {:>9}", ns(t.elapsed() / 1000));

    let t = Instant::now();
    let mut undone = 0;
    while buffer.undo().is_some() {
        undone += 1;
    }
    println!(
        "  undo everything    {:>9}   {undone} steps",
        ms(t.elapsed())
    );

    let out = dir.join("saved.txt");
    let t = Instant::now();
    save(&out, buffer.text(), buffer.format()).unwrap();
    let save_time = t.elapsed();
    println!(
        "  save (fsync)       {:>9}   {:.1} GB/s",
        ms(save_time),
        file_len as f64 / save_time.as_secs_f64() / 1e9
    );
    assert_eq!(fs::metadata(&out).unwrap().len(), file_len);
    fs::remove_file(out).unwrap();
}

/// Writes a log-like file of about `len` bytes, unless it already exists.
fn generate(path: &Path, len: u64) {
    if fs::metadata(path).is_ok_and(|m| m.len() >= len) {
        return;
    }
    eprintln!("generating {}…", path.display());
    let mut out = BufWriter::with_capacity(1 << 20, File::create(path).unwrap());
    let mut rng = Rng(1);
    let mut written = 0u64;
    let levels = ["INFO", "DEBUG", "WARN", "ERROR"];
    let mut line = String::new();
    while written < len {
        line.clear();
        use std::fmt::Write as _;
        let n = rng.below(1_000_000);
        writeln!(
            line,
            "2026-11-08T03:{:02}:{:02}.{:03}Z {:5} worker-{} request {} took {} ms: {}",
            n % 60,
            (n / 60) % 60,
            n % 1000,
            levels[(n % 4) as usize],
            n % 32,
            n,
            n % 977,
            &"the quick brown fox jumps over the lazy dog"[..(n % 44) as usize]
        )
        .unwrap();
        out.write_all(line.as_bytes()).unwrap();
        written += line.len() as u64;
    }
    out.flush().unwrap();
}

/// Resident memory in bytes (Linux only).
fn rss() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// xorshift64*: deterministic and fast.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n.max(1)
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn ms(d: Duration) -> String {
    format!("{:.1} ms", d.as_secs_f64() * 1e3)
}

fn us(d: Duration) -> String {
    format!("{:.2} µs", d.as_secs_f64() * 1e6)
}

fn ns(d: Duration) -> String {
    format!("{} ns", d.as_nanos())
}

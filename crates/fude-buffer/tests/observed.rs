//! Loading on a worker: the preview, progress and cancelling.

use std::io::ErrorKind;
use std::path::PathBuf;

use encoding_rs::{SHIFT_JIS, UTF_16LE};
use fude_buffer::{LoadObserver, Loaded, load_observed};

#[derive(Default)]
struct Watch {
    preview: Option<Loaded>,
    progress: Vec<u64>,
    cancel_after: Option<u64>,
}

impl LoadObserver for Watch {
    fn preview(&mut self, preview: Loaded) {
        assert!(self.progress.is_empty(), "the preview comes first");
        self.preview = Some(preview);
    }

    fn progress(&mut self, read: u64) {
        self.progress.push(read);
    }

    fn cancelled(&self) -> bool {
        self.cancel_after
            .is_some_and(|n| self.progress.last().is_some_and(|&r| r >= n))
    }
}

fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
    let path = std::env::temp_dir().join(format!("fude-observed-{}-{name}", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Loads `bytes` and checks the preview is a line-aligned prefix of the result.
fn check(name: &str, bytes: &[u8]) -> (Loaded, Loaded) {
    let path = temp_file(name, bytes);
    let mut watch = Watch::default();
    let loaded = load_observed(&path, None, &mut watch).unwrap();
    std::fs::remove_file(&path).unwrap();
    let preview = watch.preview.expect("a file over 1 MB gets a preview");
    let (p, full) = (preview.text.to_vec(), loaded.text.to_vec());
    // 64 KB of the file; decoding can make it longer (Shift_JIS: up to 1.5×).
    assert!(
        !p.is_empty() && p.len() <= 2 * 64 * 1024,
        "{name}: {} bytes",
        p.len()
    );
    assert_eq!(preview.format, loaded.format, "{name}");
    assert!(full.starts_with(&p), "{name}: the preview is a prefix");
    assert!(
        p.ends_with(b"\n"),
        "{name}: the preview ends at a line break"
    );
    assert_eq!(preview.format, loaded.format, "{name}");
    assert_eq!(watch.progress.last(), Some(&(bytes.len() as u64)), "{name}");
    assert!(watch.progress.is_sorted(), "{name}");
    (preview, loaded)
}

#[test]
fn previews_are_prefixes_in_any_encoding() {
    let line = "2026-11-08 筆 fude: a line of text, café\n";
    let text = line.repeat(3 << 20 >> 6);
    check("utf8.txt", text.as_bytes());

    let japanese = "2026-11-08 筆はブラシという意味です。テキストエディタ。\n".repeat(3 << 20 >> 6);
    let (sjis, _, unmappable) = SHIFT_JIS.encode(&japanese);
    assert!(!unmappable);
    let (_, loaded) = check("sjis.txt", &sjis);
    assert_eq!(loaded.format.encoding, SHIFT_JIS);

    let utf16: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    let (_, loaded) = check("utf16.txt", &utf16);
    assert_eq!(loaded.format.encoding, UTF_16LE);
    assert!(loaded.format.bom);
}

#[test]
fn a_long_first_line_previews_its_start() {
    let text = "x".repeat(3 << 20);
    let path = temp_file("long.txt", text.as_bytes());
    let mut watch = Watch::default();
    let loaded = load_observed(&path, None, &mut watch).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(watch.preview.unwrap().text.len(), 64 * 1024);
    assert_eq!(loaded.text.len(), 3 << 20);
}

#[test]
fn small_files_have_no_preview() {
    let path = temp_file("small.txt", b"hello\n");
    let mut watch = Watch::default();
    load_observed(&path, None, &mut watch).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(watch.preview.is_none());
    assert_eq!(watch.progress, [6]);
}

#[test]
fn cancelling_stops_the_load() {
    let path = temp_file("cancel.txt", &vec![b'a'; 5 << 20]);
    let mut watch = Watch {
        cancel_after: Some(2 << 20),
        ..Watch::default()
    };
    let err = load_observed(&path, None, &mut watch).unwrap_err();
    std::fs::remove_file(&path).unwrap();
    assert_eq!(err.kind(), ErrorKind::Interrupted);
    assert!(*watch.progress.last().unwrap() < 5 << 20);
}

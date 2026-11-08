//! Encoding detection, decoding and round trips, with real node sizes.

use encoding_rs::{
    BIG5, EUC_KR, Encoding, GBK, ISO_2022_JP, SHIFT_JIS, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252,
};
use fude_buffer::{
    Format, LineEnding, LineEndingCounts, Rope, SaveError, convert_line_endings, decode, encode,
};

fn encoded(text: &str, encoding: &'static Encoding) -> Vec<u8> {
    if encoding == UTF_16LE {
        return text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    }
    if encoding == UTF_16BE {
        return text.encode_utf16().flat_map(u16::to_be_bytes).collect();
    }
    let (bytes, _, unmappable) = encoding.encode(text);
    assert!(!unmappable, "test text fits {}", encoding.name());
    bytes.into_owned()
}

/// Saving the unedited text reproduces the input.
fn assert_round_trip(bytes: &[u8]) {
    let loaded = decode(bytes, None);
    assert!(loaded.exact, "expected an exact round trip");
    let mut out = Vec::new();
    encode(&loaded.text, &loaded.format, &mut out).unwrap();
    assert!(out == bytes, "re-encoded bytes differ");
}

const JAPANESE: &str = "日本語のテキストファイルです。筆はブラシという意味です。\
    エディタは速くて軽いのが一番です。";
const FRENCH: &str = "Le cœur de l'éditeur est très rapide : déjà prêt, même pour les \
    fichiers énormes. Où est la clé ? À côté de la fenêtre, près du café. ";
const CHINESE: &str =
    "这是一个文本编辑器。它的启动速度非常快，内存占用也很低。打开大文件时也不会卡顿。";
const KOREAN: &str =
    "이것은 텍스트 편집기입니다. 시작 속도가 매우 빠르고 메모리 사용량도 적습니다.";
const TRADITIONAL: &str =
    "這是一個文字編輯器。它的啟動速度非常快，記憶體用量也很低。開啟大型檔案時也不會卡住。";

#[test]
fn plain_utf8() {
    let loaded = decode("héllo 筆\nworld\n".as_bytes(), None);
    assert_eq!(loaded.format.encoding, UTF_8);
    assert!(!loaded.format.bom && loaded.exact && !loaded.binary);
    assert_eq!(loaded.text.to_vec(), "héllo 筆\nworld\n".as_bytes());
    assert_eq!(loaded.format.line_ending, LineEnding::Lf);
}

#[test]
fn byte_order_marks() {
    let text = "BOM 筆 😀\r\nline two\r\n";
    for (encoding, bom) in [
        (UTF_8, &b"\xEF\xBB\xBF"[..]),
        (UTF_16LE, b"\xFF\xFE"),
        (UTF_16BE, b"\xFE\xFF"),
    ] {
        let bytes = [bom, &encoded(text, encoding)].concat();
        let loaded = decode(&bytes, None);
        assert_eq!(loaded.format.encoding, encoding);
        assert!(loaded.format.bom);
        assert_eq!(loaded.text.to_vec(), text.as_bytes(), "{}", encoding.name());
        assert_eq!(loaded.format.line_ending, LineEnding::CrLf);
        assert_round_trip(&bytes);
    }
}

#[test]
fn utf16_without_bom() {
    let text = "plain ASCII mostly, with one 筆.\n".repeat(20);
    for encoding in [UTF_16LE, UTF_16BE] {
        let bytes = encoded(&text, encoding);
        let loaded = decode(&bytes, None);
        assert_eq!(loaded.format.encoding, encoding);
        assert!(!loaded.format.bom && !loaded.binary);
        assert_eq!(loaded.text.to_vec(), text.as_bytes());
        assert_round_trip(&bytes);
    }
}

#[test]
fn legacy_encodings_are_detected() {
    for (text, encoding) in [
        (JAPANESE, SHIFT_JIS),
        (FRENCH, WINDOWS_1252),
        (CHINESE, GBK),
        (KOREAN, EUC_KR),
        (TRADITIONAL, BIG5),
    ] {
        let text = text.repeat(4);
        let bytes = encoded(&text, encoding);
        let loaded = decode(&bytes, None);
        assert_eq!(
            loaded.format.encoding.name(),
            encoding.name(),
            "detecting {}",
            encoding.name()
        );
        assert_eq!(String::from_utf8(loaded.text.to_vec()).unwrap(), text);
        assert_round_trip(&bytes);
    }
}

#[test]
fn mostly_utf8_with_stray_bytes_stays_raw() {
    let mut bytes = "ログ: everything is fine ✓\n".repeat(10).into_bytes();
    bytes.extend_from_slice(b"garbage \xFF\xFE here\n");
    let loaded = decode(&bytes, None);
    assert_eq!(loaded.format.encoding, UTF_8);
    assert_eq!(
        loaded.text.to_vec(),
        bytes,
        "invalid bytes kept as they are"
    );
    assert_round_trip(&bytes);
}

#[test]
fn binary_stays_raw() {
    let bytes: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
    let loaded = decode(&bytes, None);
    assert!(loaded.binary);
    assert_eq!(loaded.format.encoding, UTF_8);
    assert_round_trip(&bytes);
}

#[test]
fn forced_encoding() {
    let bytes = encoded("日本", SHIFT_JIS);
    let loaded = decode(&bytes, Some(SHIFT_JIS));
    assert_eq!(loaded.format.encoding, SHIFT_JIS);
    assert_eq!(loaded.text.to_vec(), "日本".as_bytes());
    // Forcing UTF-8 keeps the bytes raw.
    let raw = decode(&bytes, Some(UTF_8));
    assert_eq!(raw.text.to_vec(), bytes);
    assert!(raw.exact);
}

#[test]
fn inexact_decodes_are_flagged() {
    // UTF-16 with a lone surrogate, and an odd trailing byte.
    let mut bytes = b"\xFF\xFE".to_vec();
    bytes.extend_from_slice(&encoded("ok", UTF_16LE));
    bytes.extend_from_slice(&[0x00, 0xD8, b'x']);
    assert!(!decode(&bytes, None).exact);

    // Shift_JIS has two byte sequences for some characters (NEC and IBM extensions);
    // encoding picks one, so the other doesn't round-trip.
    let nec = b"\xED\x40";
    let loaded = decode(nec, Some(SHIFT_JIS));
    assert_eq!(loaded.text.to_vec(), "纊".as_bytes());
    assert!(!loaded.exact);
}

/// Multi-byte characters straddle the loader's 1 MB read boundaries.
#[test]
fn large_files_detect_across_read_boundaries() {
    let text = JAPANESE.repeat(30_000); // ~3.8 MB of UTF-8
    let utf8 = text.as_bytes();
    let loaded = decode(utf8, None);
    assert_eq!(loaded.format.encoding, UTF_8);
    assert_eq!(&loaded.text.to_vec()[..], utf8);
    loaded.text.check_invariants();

    let sjis = encoded(&text, SHIFT_JIS);
    let loaded = decode(&sjis, None);
    assert_eq!(loaded.format.encoding, SHIFT_JIS);
    assert_eq!(&loaded.text.to_vec()[..], utf8);
    assert_round_trip(&sjis);

    let emoji = "😀 surrogate pairs ".repeat(100_000);
    let utf16 = [&b"\xFF\xFE"[..], &encoded(&emoji, UTF_16LE)].concat();
    let loaded = decode(&utf16, None);
    assert_eq!(loaded.text.to_vec(), emoji.as_bytes());
    assert_round_trip(&utf16);
}

#[test]
fn saving_unmappable_text_fails_with_the_offset() {
    let text = Rope::from("café 筆");
    let format = Format {
        encoding: WINDOWS_1252,
        ..Format::default()
    };
    let err = encode(&text, &format, &mut Vec::new()).unwrap_err();
    assert!(
        matches!(
            err,
            SaveError::Unmappable {
                offset: 6,
                ch: '筆',
                ..
            }
        ),
        "{err:?}"
    );

    let raw = Rope::from(&b"ok \xFF"[..]);
    let utf16 = Format {
        encoding: UTF_16LE,
        ..Format::default()
    };
    let err = encode(&raw, &utf16, &mut Vec::new()).unwrap_err();
    assert!(
        matches!(err, SaveError::InvalidUtf8 { offset: 3, .. }),
        "{err:?}"
    );
}

#[test]
fn stateful_encoders_are_flushed() {
    let text = Rope::from("abc 日本 def 日本");
    let format = Format {
        encoding: ISO_2022_JP,
        ..Format::default()
    };
    let mut out = Vec::new();
    encode(&text, &format, &mut out).unwrap();
    let (back, had_errors) = ISO_2022_JP.decode_without_bom_handling(&out);
    assert!(!had_errors);
    assert_eq!(back, "abc 日本 def 日本");
    assert!(out.ends_with(b"\x1b(B"), "returns to ASCII at the end");
}

#[test]
fn line_endings() {
    let loaded = decode(b"a\r\nb\r\nc\nd\re", None);
    let counts = loaded.text.line_ending_counts();
    assert_eq!(
        counts,
        LineEndingCounts {
            lf: 1,
            crlf: 2,
            cr: 1
        }
    );
    assert!(counts.is_mixed());
    assert_eq!(loaded.format.line_ending, LineEnding::CrLf);
    assert_eq!(
        decode(b"no breaks", None).format.line_ending,
        LineEnding::PLATFORM
    );
}

#[test]
fn converting_line_endings_across_leaves() {
    // Long enough for many leaves, with CRLF pairs landing on leaf boundaries.
    let mut text = Vec::new();
    for i in 0..20_000 {
        text.extend_from_slice(format!("line {i}").as_bytes());
        text.extend_from_slice([&b"\r\n"[..], b"\n", b"\r"][i % 3]);
    }
    let rope = Rope::from(&text[..]);
    for to in [LineEnding::Lf, LineEnding::CrLf, LineEnding::Cr] {
        let converted = convert_line_endings(&rope, to);
        converted.check_invariants();
        let expected: Vec<u8> = String::from_utf8(text.clone())
            .unwrap()
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .replace('\n', std::str::from_utf8(to.as_bytes()).unwrap())
            .into_bytes();
        assert!(
            converted.to_vec() == expected,
            "converting to {}",
            to.name()
        );
        assert_eq!(converted.line_count(), rope.line_count());
    }
    let lf_only = Rope::from("a\nb\n");
    assert!(convert_line_endings(&lf_only, LineEnding::Lf).ptr_eq(&lf_only));
}

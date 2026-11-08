//! Decoding arbitrary bytes never panics, and whenever the loader says a decode is exact,
//! encoding it again gives back the input.

#![no_main]

use encoding_rs::{
    BIG5, EUC_JP, EUC_KR, GBK, ISO_2022_JP, SHIFT_JIS, UTF_8, UTF_16BE, UTF_16LE, WINDOWS_1252,
};
use fude_buffer::{convert_line_endings, decode, encode, LineEnding};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&choice, bytes)) = data.split_first() else { return };
    let forced = [
        None,
        Some(UTF_8),
        Some(UTF_16LE),
        Some(UTF_16BE),
        Some(SHIFT_JIS),
        Some(EUC_JP),
        Some(ISO_2022_JP),
        Some(GBK),
        Some(BIG5),
        Some(EUC_KR),
        Some(WINDOWS_1252),
    ][usize::from(choice) % 11];
    let loaded = decode(bytes, forced);
    loaded.text.check_invariants();
    let mut out = Vec::new();
    let encoded = encode(&loaded.text, &loaded.format, &mut out);
    if loaded.exact {
        encoded.expect("an exact decode encodes");
        assert!(out == bytes, "exact decode didn't round-trip");
    }
    for to in [LineEnding::Lf, LineEnding::CrLf] {
        let converted = convert_line_endings(&loaded.text, to);
        converted.check_invariants();
        assert_eq!(converted.line_count(), loaded.text.line_count());
    }
});

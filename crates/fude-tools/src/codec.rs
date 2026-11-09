//! Case changes and encodings: Base64, URL (percent) and HTML entities.

/// Applies a string function to the valid UTF-8 in `text`, leaving invalid bytes alone.
pub fn map_str(text: &[u8], f: fn(&str) -> String) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for chunk in text.utf8_chunks() {
        out.extend_from_slice(f(chunk.valid()).as_bytes());
        out.extend_from_slice(chunk.invalid());
    }
    out
}

/// The first letter of each word upper case, the rest lower case.
pub fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = true;
    for c in s.chars() {
        if c.is_alphanumeric() || c == '\'' {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
    out
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard Base64 with padding, on one line.
pub fn base64_encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for k in 0..4 {
            if k <= chunk.len() {
                out.push(BASE64[(n >> (18 - 6 * k) & 63) as usize]);
            } else {
                out.push(b'=');
            }
        }
    }
    out
}

/// Standard or URL-safe Base64, padded or not; whitespace is skipped.
pub fn base64_decode(text: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    let mut padding = false;
    for (i, &c) in text.iter().enumerate() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => {
                padding = true;
                continue;
            }
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => {
                let (line, col) = crate::line_col(text, i);
                return Err(format!(
                    "Not Base64: unexpected {:?} at line {line}, column {col}",
                    c as char
                ));
            }
        };
        if padding {
            return Err("Not Base64: text after the padding".into());
        }
        acc = acc << 6 | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    if bits >= 6 {
        return Err("Not Base64: the length is off by one character".into());
    }
    Ok(out)
}

/// Percent-encodes everything but letters, digits and `-_.~`.
pub fn url_encode(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for &b in text {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b);
        } else {
            out.extend_from_slice(format!("%{b:02X}").as_bytes());
        }
    }
    out
}

/// Decodes `%XX` escapes (a `%` without two hex digits stays as it is).
pub fn url_decode(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'%'
            && let Some(byte) = text
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(byte);
            i += 3;
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    out
}

pub fn html_encode(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for &b in text {
        match b {
            b'&' => out.extend_from_slice(b"&amp;"),
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'"' => out.extend_from_slice(b"&quot;"),
            b'\'' => out.extend_from_slice(b"&#39;"),
            _ => out.push(b),
        }
    }
    out
}

const ENTITIES: &[(&str, char)] = &[
    ("amp", '&'),
    ("lt", '<'),
    ("gt", '>'),
    ("quot", '"'),
    ("apos", '\''),
    ("nbsp", '\u{A0}'),
    ("copy", '©'),
    ("reg", '®'),
    ("trade", '™'),
    ("hellip", '…'),
    ("mdash", '—'),
    ("ndash", '–'),
    ("lsquo", '‘'),
    ("rsquo", '’'),
    ("ldquo", '“'),
    ("rdquo", '”'),
    ("laquo", '«'),
    ("raquo", '»'),
    ("bull", '•'),
    ("middot", '·'),
    ("deg", '°'),
    ("plusmn", '±'),
    ("times", '×'),
    ("divide", '÷'),
    ("euro", '€'),
    ("pound", '£'),
    ("yen", '¥'),
    ("cent", '¢'),
    ("sect", '§'),
    ("para", '¶'),
];

/// Decodes named entities (the common ones) and numeric ones; others stay as they are.
pub fn html_decode(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i] == b'&'
            && let Some(len) = text[i..].iter().take(12).position(|&b| b == b';')
            && let Some(c) = entity(&text[i + 1..i + len])
        {
            let mut buf = [0; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            i += len + 1;
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    out
}

fn entity(name: &[u8]) -> Option<char> {
    let name = std::str::from_utf8(name).ok()?;
    if let Some(num) = name.strip_prefix('#') {
        let code = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        return char::from_u32(code);
    }
    ENTITIES.iter().find(|e| e.0 == name).map(|e| e.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64() {
        for (plain, coded) in [
            ("", ""),
            ("M", "TQ=="),
            ("Ma", "TWE="),
            ("Man", "TWFu"),
            ("筆", "562G"),
        ] {
            assert_eq!(base64_encode(plain.as_bytes()), coded.as_bytes());
            assert_eq!(base64_decode(coded.as_bytes()).unwrap(), plain.as_bytes());
        }
        assert_eq!(base64_decode(b"TW\nFu").unwrap(), b"Man");
        assert_eq!(base64_decode(b"_-8").unwrap(), [0xff, 0xef], "URL-safe");
        assert!(base64_decode(b"TW!u").unwrap_err().contains("column 3"));
        assert!(base64_decode(b"TWFuT").is_err());
    }

    #[test]
    fn url_and_html() {
        assert_eq!(url_encode("a b/é".as_bytes()), b"a%20b%2F%C3%A9");
        assert_eq!(url_decode(b"a%20b%2F%C3%A9 100%"), "a b/é 100%".as_bytes());
        assert_eq!(
            html_encode(b"<a href=\"x\">&'"),
            b"&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
        assert_eq!(
            html_decode(b"&lt;b&gt; &amp;amp; &#x7B;&#125; &copy; &bogus;"),
            "<b> &amp; {} © &bogus;".as_bytes()
        );
    }

    #[test]
    fn case() {
        assert_eq!(
            map_str("straße é\u{ff}".as_bytes(), str::to_uppercase),
            "STRASSE É\u{178}".as_bytes()
        );
        assert_eq!(map_str(b"ab\xffcd", str::to_uppercase), b"AB\xffCD");
        assert_eq!(
            title_case("the QUICK brown-fox's tail"),
            "The Quick Brown-Fox's Tail"
        );
    }
}

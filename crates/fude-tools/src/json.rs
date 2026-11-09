//! Formatting and minifying JSON without building a tree: tokens are copied as they are
//! (numbers and string escapes unchanged, keys in their order, duplicates kept) and only
//! the whitespace between them changes. The structure is checked as it goes, and errors
//! say where. Several values one after another (JSON Lines) are formatted one by one.

use crate::line_col;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Expect {
    /// A value (the top level, after `[`, `,` in an array, or `:`).
    Value,
    /// A key or `}` (after `{`).
    KeyOrEnd,
    /// A key (after `,` in an object).
    Key,
    Colon,
    /// `,` or the container's closer.
    CommaOrEnd,
}

/// Formats `text`: with `pretty` = (indent, line break), one entry per line; without,
/// as compact as it gets.
pub fn format(text: &[u8], pretty: Option<(&[u8], &[u8])>) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len());
    let mut stack: Vec<u8> = Vec::new();
    let mut expect = Expect::Value;
    let mut i = 0;
    let err = |at: usize, what: &str| {
        let (line, col) = line_col(text, at);
        Err(format!("JSON: {what} at line {line}, column {col}"))
    };
    let newline = |out: &mut Vec<u8>, depth: usize| {
        if let Some((indent, le)) = pretty {
            out.extend_from_slice(le);
            for _ in 0..depth {
                out.extend_from_slice(indent);
            }
        }
    };
    let mut values = 0;
    loop {
        while i < text.len() && matches!(text[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if i == text.len() {
            break;
        }
        let c = text[i];
        if stack.is_empty() && expect == Expect::CommaOrEnd {
            // Another top-level value: one per line.
            expect = Expect::Value;
            out.extend_from_slice(pretty.map_or(&b"\n"[..], |p| p.1));
        }
        match c {
            b'{' | b'[' => {
                if !matches!(expect, Expect::Value) {
                    return err(i, "unexpected opening bracket");
                }
                out.push(c);
                stack.push(c);
                // An empty container stays on one line.
                let mut j = i + 1;
                while j < text.len() && matches!(text[j], b' ' | b'\t' | b'\n' | b'\r') {
                    j += 1;
                }
                let close = if c == b'{' { b'}' } else { b']' };
                if text.get(j) == Some(&close) {
                    out.push(close);
                    stack.pop();
                    i = j + 1;
                    expect = Expect::CommaOrEnd;
                    continue;
                }
                newline(&mut out, stack.len());
                expect = if c == b'{' {
                    Expect::KeyOrEnd
                } else {
                    Expect::Value
                };
                i += 1;
            }
            b'}' | b']' => {
                let open = if c == b'}' { b'{' } else { b'[' };
                if stack.last() != Some(&open) || expect != Expect::CommaOrEnd {
                    return err(i, &format!("unexpected '{}'", c as char));
                }
                stack.pop();
                newline(&mut out, stack.len());
                out.push(c);
                expect = Expect::CommaOrEnd;
                i += 1;
            }
            b',' => {
                if expect != Expect::CommaOrEnd || stack.is_empty() {
                    return err(i, "unexpected ','");
                }
                out.push(b',');
                newline(&mut out, stack.len());
                expect = if stack.last() == Some(&b'{') {
                    Expect::Key
                } else {
                    Expect::Value
                };
                i += 1;
            }
            b':' => {
                if expect != Expect::Colon {
                    return err(i, "unexpected ':'");
                }
                out.push(b':');
                if pretty.is_some() {
                    out.push(b' ');
                }
                expect = Expect::Value;
                i += 1;
            }
            b'"' => {
                let start = i;
                i += 1;
                loop {
                    match text.get(i) {
                        None | Some(b'\n') => return err(start, "unterminated string"),
                        Some(b'\\') => i += 2,
                        Some(b'"') => break,
                        Some(_) => i += 1,
                    }
                }
                i += 1;
                out.extend_from_slice(&text[start..i]);
                expect = match expect {
                    Expect::KeyOrEnd | Expect::Key => Expect::Colon,
                    Expect::Value => Expect::CommaOrEnd,
                    _ => return err(start, "unexpected string"),
                };
            }
            b'-' | b'0'..=b'9' | b't' | b'f' | b'n' => {
                if expect != Expect::Value {
                    return err(i, "unexpected value");
                }
                let start = i;
                while i < text.len()
                    && (text[i].is_ascii_alphanumeric() || b"+-.".contains(&text[i]))
                {
                    i += 1;
                }
                let token = &text[start..i];
                let number = token[0] == b'-' || token[0].is_ascii_digit();
                let valid = if number {
                    token
                        .iter()
                        .all(|b| b.is_ascii_digit() || b"+-.eE".contains(b))
                } else {
                    matches!(token, b"true" | b"false" | b"null")
                };
                if !valid {
                    return err(
                        start,
                        &format!("unexpected {:?}", String::from_utf8_lossy(token)),
                    );
                }
                out.extend_from_slice(token);
                expect = Expect::CommaOrEnd;
            }
            b'/' => return err(i, "comments aren't JSON"),
            _ => return err(i, &format!("unexpected {:?}", c as char)),
        }
        if stack.is_empty() && expect == Expect::CommaOrEnd {
            values += 1;
        }
    }
    if !stack.is_empty() || (expect != Expect::CommaOrEnd && values > 0) {
        return err(text.len(), "unexpected end");
    }
    if pretty.is_some() && values > 0 {
        out.extend_from_slice(pretty.map_or(&b"\n"[..], |p| p.1));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pretty(s: &str) -> Result<String, String> {
        format(s.as_bytes(), Some((b"  ", b"\n"))).map(|v| String::from_utf8(v).unwrap())
    }

    fn mini(s: &str) -> Result<String, String> {
        format(s.as_bytes(), None).map(|v| String::from_utf8(v).unwrap())
    }

    #[test]
    fn formats_and_minifies() {
        let src = r#"{"a":1.50e3,"b":[true,null,{}],"c":"x, \"y\": [z]","d":[]}"#;
        assert_eq!(
            pretty(src).unwrap(),
            "{\n  \"a\": 1.50e3,\n  \"b\": [\n    true,\n    null,\n    {}\n  ],\n  \"c\": \"x, \\\"y\\\": [z]\",\n  \"d\": []\n}\n"
        );
        assert_eq!(mini(&pretty(src).unwrap()).unwrap(), src);
        assert_eq!(
            mini("{\"a\" : 1}\n{\"b\":2}\n").unwrap(),
            "{\"a\":1}\n{\"b\":2}",
            "JSON Lines"
        );
    }

    #[test]
    fn errors_say_where() {
        assert_eq!(
            pretty("{\"a\" 1}").unwrap_err(),
            "JSON: unexpected value at line 1, column 6"
        );
        assert!(pretty("[1,\n2,]").unwrap_err().contains("line 2, column 3"));
        assert!(pretty("{\"a\":1").unwrap_err().contains("unexpected end"));
        assert!(pretty("[tru]").is_err());
        assert!(pretty("// x\n{}").unwrap_err().contains("comments"));
        assert!(pretty("\"open").unwrap_err().contains("unterminated"));
    }
}

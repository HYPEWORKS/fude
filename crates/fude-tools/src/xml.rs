//! Formatting XML: each element, comment and processing instruction on its own line,
//! indented by depth; an element holding only text stays on one line. Tags and text are
//! copied as they are (attributes untouched, nothing re-escaped); whitespace between tags
//! is replaced. Mismatched and unclosed tags are errors.

use crate::line_col;

#[derive(Debug, PartialEq)]
enum Token<'a> {
    Start {
        name: &'a [u8],
        tag: &'a [u8],
        empty: bool,
    },
    End {
        name: &'a [u8],
        tag: &'a [u8],
    },
    /// Comments, processing instructions, CDATA, doctypes: copied on a line of their own.
    Other(&'a [u8]),
    Text(&'a [u8]),
}

fn tokens(text: &[u8]) -> Result<Vec<(usize, Token<'_>)>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    let err = |at: usize, what: &str| {
        let (line, col) = line_col(text, at);
        Err(format!("XML: {what} at line {line}, column {col}"))
    };
    let find = |from: usize, end: &[u8]| {
        text[from..]
            .windows(end.len())
            .position(|w| w == end)
            .map(|p| from + p + end.len())
    };
    while i < text.len() {
        if text[i] != b'<' {
            let end = text[i..]
                .iter()
                .position(|&b| b == b'<')
                .map_or(text.len(), |p| i + p);
            out.push((i, Token::Text(&text[i..end])));
            i = end;
            continue;
        }
        let rest = &text[i..];
        let end = if rest.starts_with(b"<!--") {
            find(i + 4, b"-->").ok_or("unterminated comment")
        } else if rest.starts_with(b"<![CDATA[") {
            find(i + 9, b"]]>").ok_or("unterminated CDATA section")
        } else if rest.starts_with(b"<?") {
            find(i + 2, b"?>").ok_or("unterminated processing instruction")
        } else if rest.starts_with(b"<!") {
            // A doctype, maybe with an internal subset in brackets.
            let mut depth = 0;
            rest.iter()
                .position(|&b| {
                    depth += i32::from(b == b'[') - i32::from(b == b']');
                    b == b'>' && depth == 0
                })
                .map(|p| i + p + 1)
                .ok_or("unterminated declaration")
        } else {
            // A tag: to the next '>' outside quotes.
            let mut quote = None;
            rest.iter()
                .position(|&b| match quote {
                    Some(q) if b == q => {
                        quote = None;
                        false
                    }
                    Some(_) => false,
                    None if b == b'"' || b == b'\'' => {
                        quote = Some(b);
                        false
                    }
                    None => b == b'>',
                })
                .map(|p| i + p + 1)
                .ok_or("unclosed tag")
        };
        let end = match end {
            Ok(e) => e,
            Err(what) => return err(i, what),
        };
        let tag = &text[i..end];
        let token = if tag.starts_with(b"<!") || tag.starts_with(b"<?") {
            Token::Other(tag)
        } else if let Some(inner) = tag.strip_prefix(b"</") {
            Token::End {
                name: name_of(inner),
                tag,
            }
        } else {
            Token::Start {
                name: name_of(&tag[1..]),
                tag,
                empty: tag.ends_with(b"/>"),
            }
        };
        if matches!(
            token,
            Token::Start { name: [], .. } | Token::End { name: [], .. }
        ) {
            return err(i, "a tag without a name");
        }
        out.push((i, token));
        i = end;
    }
    Ok(out)
}

fn name_of(s: &[u8]) -> &[u8] {
    let end = s
        .iter()
        .position(|&b| b.is_ascii_whitespace() || b == b'>' || b == b'/')
        .unwrap_or(s.len());
    &s[..end]
}

pub fn format(text: &[u8], indent: &[u8], le: &[u8]) -> Result<Vec<u8>, String> {
    let tokens = tokens(text)?;
    let mut out = Vec::with_capacity(text.len() + text.len() / 4);
    let mut open: Vec<&[u8]> = Vec::new();
    let line = |out: &mut Vec<u8>, depth: usize| {
        if !out.is_empty() {
            out.extend_from_slice(le);
        }
        for _ in 0..depth {
            out.extend_from_slice(indent);
        }
    };
    let mut k = 0;
    while k < tokens.len() {
        let (at, token) = &tokens[k];
        match token {
            Token::Text(t) => {
                let t = t.trim_ascii();
                if !t.is_empty() {
                    line(&mut out, open.len());
                    out.extend_from_slice(t);
                }
            }
            Token::Other(t) => {
                line(&mut out, open.len());
                out.extend_from_slice(t);
            }
            Token::Start { name, tag, empty } => {
                line(&mut out, open.len());
                out.extend_from_slice(tag);
                if !*empty {
                    // <a>text</a> and <a></a> stay on one line.
                    let closes = |j: usize| matches!(tokens.get(j), Some((_, Token::End { name: n, .. })) if n == name);
                    match tokens.get(k + 1) {
                        Some((_, Token::Text(t))) if closes(k + 2) => {
                            out.extend_from_slice(t.trim_ascii());
                            if let Some((_, Token::End { tag, .. })) = tokens.get(k + 2) {
                                out.extend_from_slice(tag);
                            }
                            k += 3;
                            continue;
                        }
                        _ if closes(k + 1) => {
                            if let Some((_, Token::End { tag, .. })) = tokens.get(k + 1) {
                                out.extend_from_slice(tag);
                            }
                            k += 2;
                            continue;
                        }
                        _ => open.push(name),
                    }
                }
            }
            Token::End { name, tag } => {
                match open.pop() {
                    Some(o) if o == *name => {}
                    other => {
                        let (l, c) = line_col(text, *at);
                        let expected = other.map_or("nothing open".to_owned(), |o| {
                            format!("<{}> open", String::from_utf8_lossy(o))
                        });
                        return Err(format!(
                            "XML: </{}> at line {l}, column {c}, with {expected}",
                            String::from_utf8_lossy(name)
                        ));
                    }
                }
                line(&mut out, open.len());
                out.extend_from_slice(tag);
            }
        }
        k += 1;
    }
    if let Some(name) = open.last() {
        return Err(format!(
            "XML: <{}> isn't closed",
            String::from_utf8_lossy(name)
        ));
    }
    out.extend_from_slice(le);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(s: &str) -> Result<String, String> {
        format(s.as_bytes(), b"  ", b"\n").map(|v| String::from_utf8(v).unwrap())
    }

    #[test]
    fn formats() {
        let src = r#"<?xml version="1.0"?><!-- c --><root a="x>y"><item id='1'>one</item><item/><empty></empty><list><![CDATA[<raw>]]></list></root>"#;
        assert_eq!(
            f(src).unwrap(),
            "<?xml version=\"1.0\"?>\n<!-- c -->\n<root a=\"x>y\">\n  <item id='1'>one</item>\n  <item/>\n  <empty></empty>\n  <list>\n    <![CDATA[<raw>]]>\n  </list>\n</root>\n"
        );
        // Formatting is stable.
        assert_eq!(f(&f(src).unwrap()).unwrap(), f(src).unwrap());
        assert_eq!(
            f("<!DOCTYPE x [<!ENTITY e \"v\">]><x>  text  <b>bold</b> more </x>").unwrap(),
            "<!DOCTYPE x [<!ENTITY e \"v\">]>\n<x>\n  text\n  <b>bold</b>\n  more\n</x>\n"
        );
    }

    #[test]
    fn errors() {
        assert_eq!(
            f("<a><b></a>").unwrap_err(),
            "XML: </a> at line 1, column 7, with <b> open"
        );
        assert_eq!(f("<a>").unwrap_err(), "XML: <a> isn't closed");
        assert!(f("<a").unwrap_err().contains("unclosed tag"));
        assert!(f("<!-- x").unwrap_err().contains("unterminated comment"));
    }
}

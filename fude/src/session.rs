//! The session: open tabs and unsaved changes, kept in the state directory so quitting
//! never has to ask (hot exit) and a crash loses at most a few seconds of typing.
//!
//! `session.toml` lists the tabs; each document with unsaved changes has its text in
//! `backups/<name>`, written with the same safe save as documents (temporary file, then
//! rename), so a backup is never half-written.

use std::io;
use std::path::{Path, PathBuf};

use fude_buffer::{Encoding, Format, LineEnding, Rope};

#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub path: Option<PathBuf>,
    /// The backup holding unsaved text, if the document had unsaved changes.
    pub backup: Option<String>,
    pub encoding: String,
    pub bom: bool,
    pub line_ending: LineEnding,
    /// Cursor byte offset.
    pub cursor: usize,
    /// First visible line (fractional).
    pub scroll: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Session {
    pub active: usize,
    pub tabs: Vec<Tab>,
}

impl Tab {
    pub fn format(&self) -> Format {
        Format {
            encoding: Encoding::for_label(self.encoding.as_bytes()).unwrap_or(encoding_utf8()),
            bom: self.bom,
            line_ending: self.line_ending,
        }
    }
}

fn encoding_utf8() -> &'static Encoding {
    Encoding::for_label(b"utf-8").expect("UTF-8")
}

pub fn backup_dir(state: &Path) -> PathBuf {
    state.join("backups")
}

/// A fresh, unused backup file name.
pub fn new_backup_name() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(std::process::id().into());
    format!("{:016x}.txt", h.finish())
}

/// Writes a document's text, byte for byte.
pub fn write_backup(state: &Path, name: &str, text: &Rope) -> io::Result<()> {
    let dir = backup_dir(state);
    std::fs::create_dir_all(&dir)?;
    let raw = Format {
        encoding: encoding_utf8(),
        bom: false,
        line_ending: LineEnding::Lf,
    };
    fude_buffer::save(&dir.join(name), text, &raw)
        .map(|_| ())
        .map_err(|e| io::Error::other(e.to_string()))
}

pub fn read_backup(state: &Path, name: &str) -> io::Result<Rope> {
    Ok(fude_buffer::load_with(&backup_dir(state).join(name), Some(encoding_utf8()))?.text)
}

/// Deletes backups no tab refers to any more.
pub fn clean_backups(state: &Path, keep: &[&str]) {
    let Ok(entries) = std::fs::read_dir(backup_dir(state)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !keep.contains(&name.as_ref()) && name.ends_with(".txt") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

impl Session {
    pub fn read(state: &Path) -> Option<Session> {
        let text = std::fs::read_to_string(state.join("session.toml")).ok()?;
        let table: toml::Table = text.parse().ok()?;
        let tabs = table
            .get("tab")?
            .as_array()?
            .iter()
            .filter_map(|t| {
                let t = t.as_table()?;
                let str_of = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_owned);
                Some(Tab {
                    path: str_of("path").map(PathBuf::from),
                    backup: str_of("backup"),
                    encoding: str_of("encoding").unwrap_or_else(|| "UTF-8".to_owned()),
                    bom: t.get("bom").and_then(|v| v.as_bool()).unwrap_or(false),
                    line_ending: match str_of("line_ending").as_deref() {
                        Some("CRLF") => LineEnding::CrLf,
                        Some("CR") => LineEnding::Cr,
                        _ => LineEnding::Lf,
                    },
                    cursor: t
                        .get("cursor")
                        .and_then(|v| v.as_integer())
                        .unwrap_or(0)
                        .max(0) as usize,
                    scroll: t
                        .get("scroll")
                        .and_then(|v| v.as_float())
                        .unwrap_or(0.0)
                        .max(0.0) as f32,
                })
            })
            .collect();
        let active = table
            .get("active")
            .and_then(|v| v.as_integer())
            .unwrap_or(0)
            .max(0) as usize;
        Some(Session { active, tabs })
    }

    /// Writes `session.toml` safely (temporary file, then rename).
    pub fn write(&self, state: &Path) -> io::Result<()> {
        std::fs::create_dir_all(state)?;
        let mut out = String::from("# fude's open tabs. Written by fude; edits are overwritten.\n");
        out.push_str(&format!("active = {}\n", self.active));
        for tab in &self.tabs {
            out.push_str("\n[[tab]]\n");
            if let Some(path) = &tab.path {
                out.push_str(&format!("path = {}\n", quote(&path.to_string_lossy())));
            }
            if let Some(backup) = &tab.backup {
                out.push_str(&format!("backup = {}\n", quote(backup)));
            }
            out.push_str(&format!("encoding = {}\n", quote(&tab.encoding)));
            out.push_str(&format!("bom = {}\n", tab.bom));
            out.push_str(&format!("line_ending = \"{}\"\n", tab.line_ending.name()));
            out.push_str(&format!("cursor = {}\n", tab.cursor));
            out.push_str(&format!("scroll = {:.3}\n", tab.scroll));
        }
        let raw = Format {
            encoding: encoding_utf8(),
            bom: false,
            line_ending: LineEnding::Lf,
        };
        fude_buffer::save(&state.join("session.toml"), &Rope::from(out.as_str()), &raw)
            .map(|_| ())
            .map_err(|e| io::Error::other(e.to_string()))
    }
}

/// A TOML basic string.
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let dir = std::env::temp_dir().join(format!("fude-session-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let session = Session {
            active: 1,
            tabs: vec![
                Tab {
                    path: Some(PathBuf::from("/tmp/a \"quoted\" \\ name.txt")),
                    backup: None,
                    encoding: "Shift_JIS".into(),
                    bom: false,
                    line_ending: LineEnding::CrLf,
                    cursor: 42,
                    scroll: 3.5,
                },
                Tab {
                    path: None,
                    backup: Some("abc.txt".into()),
                    encoding: "UTF-8".into(),
                    bom: true,
                    line_ending: LineEnding::Lf,
                    cursor: 0,
                    scroll: 0.0,
                },
            ],
        };
        session.write(&dir).unwrap();
        assert_eq!(Session::read(&dir), Some(session));

        let text = Rope::from(&b"raw \xff bytes\r\n"[..]);
        write_backup(&dir, "abc.txt", &text).unwrap();
        write_backup(&dir, "old.txt", &text).unwrap();
        assert_eq!(read_backup(&dir, "abc.txt").unwrap(), text);
        clean_backups(&dir, &["abc.txt"]);
        assert!(read_backup(&dir, "old.txt").is_err());
        assert!(read_backup(&dir, "abc.txt").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

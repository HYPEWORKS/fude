//! The session: open tabs and unsaved changes, kept in the state directory so quitting
//! never has to ask (hot exit) and a crash loses at most a few seconds of typing.
//!
//! `session.toml` lists the tabs; each document with unsaved changes has its text in
//! `backups/<name>`. Both are written to a temporary file and renamed into place, so they
//! are never half-written. Unlike saving a document, they aren't flushed to disk with
//! fsync: a crashed or killed process loses nothing the OS already has, and fsync can
//! take long enough on slow disks to miss the moment before a crash.

use std::io;
use std::path::{Path, PathBuf};

use std::time::{Duration, SystemTime};

use fude_buffer::{Encoding, FileStamp, Format, LineEnding, Rope};

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
    pub scroll: f64,
    /// The file as fude last read or wrote it, so a restored document with unsaved
    /// changes still notices that the file changed while fude wasn't running.
    pub stamp: Option<FileStamp>,
}

/// What the state directory held.
#[derive(Debug, PartialEq)]
pub enum Stored {
    /// No session file: a first launch.
    Nothing,
    Session(Session),
    /// A session file that can't be read (cut short by a power cut, edited by hand).
    /// The backups next to it are still good.
    Unreadable,
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
    write_replacing(&dir.join(name), |out| text.write_to(out))
}

/// Writes a file by writing a temporary file next to it and renaming it into place.
fn write_replacing(
    path: &Path,
    write: impl FnOnce(&mut io::BufWriter<std::fs::File>) -> io::Result<()>,
) -> io::Result<()> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let result = (|| {
        let mut out = io::BufWriter::new(std::fs::File::create(&tmp)?);
        write(&mut out)?;
        out.flush()?;
        drop(out);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Backups are read like any file, by the loader; this is for tests.
#[cfg(test)]
pub fn read_backup(state: &Path, name: &str) -> io::Result<Rope> {
    Ok(fude_buffer::load_with(&backup_dir(state).join(name), Some(encoding_utf8()))?.text)
}

/// The names of the backups on disk, oldest first.
pub fn list_backups(state: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(backup_dir(state)) else {
        return Vec::new();
    };
    let mut found: Vec<(Option<SystemTime>, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let modified = entry.metadata().ok().and_then(|m| m.modified().ok());
            name.ends_with(".txt").then_some((modified, name))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, name)| name).collect()
}

/// Moves a session file that couldn't be read out of the way, keeping it to look at.
pub fn set_aside(state: &Path) {
    let _ = std::fs::rename(
        state.join("session.toml"),
        state.join("session.unreadable.toml"),
    );
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
    pub fn read(state: &Path) -> Stored {
        match std::fs::read(state.join("session.toml")) {
            Ok(bytes) => String::from_utf8(bytes)
                .ok()
                .and_then(|text| Session::parse(&text))
                .map_or(Stored::Unreadable, Stored::Session),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Stored::Nothing,
            Err(_) => Stored::Unreadable,
        }
    }

    /// `None` unless `text` is a whole session file: every one has `active`, so an empty
    /// or cut-off file isn't taken for a session without tabs.
    fn parse(text: &str) -> Option<Session> {
        let table: toml::Table = text.parse().ok()?;
        let active = table.get("active")?.as_integer()?.max(0) as usize;
        let no_tabs = Vec::new();
        let tabs = match table.get("tab") {
            Some(tabs) => tabs.as_array()?,
            None => &no_tabs,
        };
        let tabs = tabs
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
                        .max(0.0),
                    stamp: read_stamp(t),
                })
            })
            .collect();
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
            if let Some(stamp) = &tab.stamp {
                out.push_str(&format!("file_len = {}\n", stamp.len));
                let since = stamp
                    .modified
                    .and_then(|m| m.duration_since(SystemTime::UNIX_EPOCH).ok());
                if let Some(since) = since {
                    out.push_str(&format!("file_secs = {}\n", since.as_secs()));
                    out.push_str(&format!("file_nanos = {}\n", since.subsec_nanos()));
                }
            }
        }
        write_replacing(&state.join("session.toml"), |w| {
            use std::io::Write;
            w.write_all(out.as_bytes())
        })
    }
}

fn read_stamp(tab: &toml::Table) -> Option<FileStamp> {
    let int = |k: &str| tab.get(k).and_then(|v| v.as_integer());
    let len = u64::try_from(int("file_len")?).ok()?;
    let modified = int("file_secs").and_then(|secs| {
        let nanos = u32::try_from(int("file_nanos").unwrap_or(0)).ok()?;
        SystemTime::UNIX_EPOCH.checked_add(Duration::new(u64::try_from(secs).ok()?, nanos))
    });
    Some(FileStamp { len, modified })
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
                    stamp: Some(FileStamp {
                        len: 7,
                        modified: Some(SystemTime::UNIX_EPOCH + Duration::new(1_700_000_000, 5)),
                    }),
                },
                Tab {
                    path: None,
                    backup: Some("abc.txt".into()),
                    encoding: "UTF-8".into(),
                    bom: true,
                    line_ending: LineEnding::Lf,
                    cursor: 0,
                    scroll: 0.0,
                    stamp: None,
                },
            ],
        };
        session.write(&dir).unwrap();
        assert_eq!(Session::read(&dir), Stored::Session(session));

        let text = Rope::from(&b"raw \xff bytes\r\n"[..]);
        write_backup(&dir, "abc.txt", &text).unwrap();
        write_backup(&dir, "old.txt", &text).unwrap();
        assert_eq!(read_backup(&dir, "abc.txt").unwrap(), text);
        clean_backups(&dir, &["abc.txt"]);
        assert!(read_backup(&dir, "old.txt").is_err());
        assert!(read_backup(&dir, "abc.txt").is_ok());
        assert_eq!(list_backups(&dir), ["abc.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cut_off_session_is_unreadable_not_empty() {
        let dir = std::env::temp_dir().join(format!("fude-session-cut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Session::read(&dir), Stored::Nothing);
        Session::default().write(&dir).unwrap();
        assert_eq!(Session::read(&dir), Stored::Session(Session::default()));
        for broken in ["", "active = ", "[[tab]]\npath = \"/x\"\n", "\u{0}\u{0}"] {
            std::fs::write(dir.join("session.toml"), broken).unwrap();
            assert_eq!(Session::read(&dir), Stored::Unreadable, "{broken:?}");
        }
        set_aside(&dir);
        assert_eq!(Session::read(&dir), Stored::Nothing);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

//! Encoding and saving files safely.
//!
//! [`save`] never truncates the original before the new contents are safely on disk. It
//! writes a temporary file next to the target, flushes it to disk and renames it over the
//! original (`ReplaceFileW` on Windows, which keeps the original's attributes and ACLs).
//! Symlinks are followed, so the link stays a link. When a rename would break a hard link
//! or lose the file's owner, or the directory isn't writable, the new contents are still
//! written and flushed to a temporary file first, then copied over the original in place.

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use encoding_rs::{EncoderResult, Encoding};

use crate::format::{is_utf8, is_utf16};
use crate::utf8::{InvalidUtf8, for_each_str};
use crate::{FileStamp, Format, Rope};

#[derive(Debug)]
pub enum SaveError {
    Io(io::Error),
    /// The file is read-only, or this user may not write it.
    ReadOnly,
    /// The text has a character the encoding can't represent.
    Unmappable {
        offset: usize,
        ch: char,
        encoding: &'static Encoding,
    },
    /// The text has bytes that aren't valid UTF-8, which only UTF-8 can save as they are.
    InvalidUtf8 {
        offset: usize,
        encoding: &'static Encoding,
    },
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::Io(e) => e.fmt(f),
            SaveError::ReadOnly => f.write_str("the file is read-only"),
            SaveError::Unmappable {
                offset,
                ch,
                encoding,
            } => write!(
                f,
                "{ch:?} (U+{:04X}) at byte {offset} can't be saved as {}",
                *ch as u32,
                encoding.name()
            ),
            SaveError::InvalidUtf8 { offset, encoding } => write!(
                f,
                "the invalid UTF-8 at byte {offset} can't be saved as {}",
                encoding.name()
            ),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SaveError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for SaveError {
    fn from(e: io::Error) -> SaveError {
        SaveError::Io(e)
    }
}

/// Error type for the internal encoder, which also has to report invalid UTF-8 without
/// knowing the encoding.
enum EncodeError {
    Sink(io::Error),
    Unmappable { offset: usize, ch: char },
    InvalidUtf8 { offset: usize },
}

impl From<InvalidUtf8> for EncodeError {
    fn from(e: InvalidUtf8) -> EncodeError {
        EncodeError::InvalidUtf8 { offset: e.offset }
    }
}

impl EncodeError {
    fn into_save_error(self, encoding: &'static Encoding) -> SaveError {
        match self {
            EncodeError::Sink(e) => SaveError::Io(e),
            EncodeError::Unmappable { offset, ch } => SaveError::Unmappable {
                offset,
                ch,
                encoding,
            },
            EncodeError::InvalidUtf8 { offset } => SaveError::InvalidUtf8 { offset, encoding },
        }
    }
}

/// Writes `text` encoded as `format` says (including the BOM). Line breaks are written as
/// they are; converting them is an edit.
pub fn encode(text: &Rope, format: &Format, out: &mut impl Write) -> Result<(), SaveError> {
    if format.bom {
        out.write_all(bom(format.encoding))?;
    }
    encode_body(text, format.encoding, |bytes| out.write_all(bytes))
        .map_err(|e| e.into_save_error(format.encoding))
}

fn bom(encoding: &'static Encoding) -> &'static [u8] {
    if encoding == encoding_rs::UTF_16LE {
        b"\xFF\xFE"
    } else if encoding == encoding_rs::UTF_16BE {
        b"\xFE\xFF"
    } else if is_utf8(encoding) {
        b"\xEF\xBB\xBF"
    } else {
        b""
    }
}

const OUT_CHUNK: usize = 64 * 1024;

fn encode_body(
    text: &Rope,
    encoding: &'static Encoding,
    mut sink: impl FnMut(&[u8]) -> io::Result<()>,
) -> Result<(), EncodeError> {
    let sink_err = EncodeError::Sink;
    if is_utf8(encoding) {
        // Raw, so invalid bytes round-trip.
        return text.chunks().try_for_each(&mut sink).map_err(sink_err);
    }
    if is_utf16(encoding) {
        // encoding_rs only decodes UTF-16 (its encoders output UTF-8, per the Encoding
        // Standard), so encode it here.
        let le = encoding == encoding_rs::UTF_16LE;
        let mut out = Vec::with_capacity(OUT_CHUNK + 8);
        for_each_str(text, |s, _| {
            for unit in s.encode_utf16() {
                out.extend_from_slice(&if le {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
                if out.len() >= OUT_CHUNK {
                    sink(&out).map_err(sink_err)?;
                    out.clear();
                }
            }
            Ok::<(), EncodeError>(())
        })?;
        return sink(&out).map_err(sink_err);
    }
    let mut encoder = encoding.new_encoder();
    let mut out = vec![0u8; OUT_CHUNK];
    let mut encode = |s: &str, offset: usize, last: bool| -> Result<(), EncodeError> {
        let mut src = s;
        let mut pos = offset;
        loop {
            let (result, read, written) =
                encoder.encode_from_utf8_without_replacement(src, &mut out, last);
            sink(&out[..written]).map_err(sink_err)?;
            src = &src[read..];
            pos += read;
            match result {
                EncoderResult::InputEmpty => return Ok(()),
                EncoderResult::OutputFull => {}
                EncoderResult::Unmappable(ch) => {
                    return Err(EncodeError::Unmappable {
                        offset: pos - ch.len_utf8(),
                        ch,
                    });
                }
            }
        }
    };
    for_each_str(text, |s, offset| encode(s, offset, false))?;
    // Flush the encoder's state (ISO-2022-JP ends by switching back to ASCII).
    encode("", text.len(), true)
}

/// Whether `text` encodes to exactly `original`.
pub(crate) fn encodes_to(text: &Rope, encoding: &'static Encoding, original: &Rope) -> bool {
    let mut expected = original.chunks();
    let mut pending: &[u8] = &[];
    let mismatch = || io::Error::other("mismatch");
    let compared = encode_body(text, encoding, |mut bytes| {
        while !bytes.is_empty() {
            if pending.is_empty() {
                pending = expected.next().ok_or_else(mismatch)?;
            }
            let n = pending.len().min(bytes.len());
            if pending[..n] != bytes[..n] {
                return Err(mismatch());
            }
            pending = &pending[n..];
            bytes = &bytes[n..];
        }
        Ok(())
    });
    compared.is_ok() && pending.is_empty() && expected.next().is_none()
}

/// Saves `text` to `path` (see the module docs). Returns the saved file's stamp.
///
/// On error the original file is unchanged, except in the rare in-place case if the
/// final copy fails, where the error says which temporary file holds the new contents.
pub fn save(path: &Path, text: &Rope, format: &Format) -> Result<FileStamp, SaveError> {
    let target = resolve_symlinks(path)?;
    let existing = match fs::metadata(&target) {
        Ok(meta) => Some(meta),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    if let Some(meta) = &existing {
        if !meta.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "not a regular file").into());
        }
        check_writable(&target, meta)?;
    }
    let name = target
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let dir = match target.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let private = existing.is_some();

    // 1. The new contents go to a temporary file, next to the target if possible.
    let (tmp_path, file, same_dir) = match create_temp(&dir, name, private) {
        Ok((p, f)) => (p, f, true),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied && existing.is_some() => {
            let (p, f) = create_temp(&std::env::temp_dir(), name, true)?;
            (p, f, false)
        }
        Err(e) => return Err(e.into()),
    };
    let mut tmp = TempFile(Some(tmp_path));
    let mut writer = BufWriter::with_capacity(1 << 20, file);
    encode(text, format, &mut writer)?;
    let file = writer
        .into_inner()
        .map_err(io::IntoInnerError::into_error)?;

    // 2. Then it replaces the original: atomically by rename when that keeps everything
    //    about the file, otherwise by copying over it.
    let atomic = same_dir
        && existing
            .as_ref()
            .is_none_or(|meta| platform::can_replace(&file, meta));
    if atomic {
        if let Some(meta) = &existing {
            file.set_permissions(meta.permissions())?;
        }
        file.sync_all()?;
        drop(file);
        platform::replace(tmp.path(), &target, existing.is_some())?;
        tmp.keep();
        platform::sync_dir(&dir);
    } else {
        file.sync_all()?;
        drop(file);
        if let Err(e) = overwrite(tmp.path(), &target) {
            let kept = tmp.keep();
            return Err(io::Error::new(
                e.kind(),
                format!("{e}; the new contents are in {}", kept.display()),
            )
            .into());
        }
    }
    Ok(FileStamp::of(&target)?)
}

/// The file a path names, following symlinks (to a target that may not exist yet).
fn resolve_symlinks(path: &Path) -> io::Result<PathBuf> {
    let is_link = fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if !is_link {
        return Ok(path.to_path_buf());
    }
    match fs::canonicalize(path) {
        Ok(real) => Ok(real),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // A dangling link: create the file it points to.
            let link = fs::read_link(path)?;
            Ok(match path.parent() {
                Some(dir) if link.is_relative() => dir.join(link),
                _ => link,
            })
        }
        Err(e) => Err(e),
    }
}

fn check_writable(path: &Path, meta: &Metadata) -> Result<(), SaveError> {
    if meta.permissions().readonly() {
        return Err(SaveError::ReadOnly);
    }
    // Permission bits don't tell the whole story (ownership, ACLs, another process holding
    // the file on Windows), so try opening it for writing. This doesn't modify it.
    match OpenOptions::new().write(true).open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => Err(SaveError::ReadOnly),
        Err(e) => Err(e.into()),
    }
}

/// Creates a new, uniquely named file in `dir`. `private` makes it readable only by this
/// user until the original's permissions are copied over.
fn create_temp(dir: &Path, name: &std::ffi::OsStr, private: bool) -> io::Result<(PathBuf, File)> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    for _ in 0..100 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let mut file_name = OsString::from(".");
        file_name.push(name);
        file_name.push(format!(".fude-{}-{n}.tmp", std::process::id()));
        let path = dir.join(file_name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        platform::set_create_mode(&mut options, private);
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "couldn't create a temporary file",
    ))
}

/// Copies `from` over `to` in place, keeping `to`'s identity (inode, links, owner).
fn overwrite(from: &Path, to: &Path) -> io::Result<()> {
    let mut source = File::open(from)?;
    let mut dest = OpenOptions::new().write(true).truncate(true).open(to)?;
    io::copy(&mut source, &mut dest)?;
    dest.sync_all()
}

/// A temporary file that is deleted when dropped, unless kept.
struct TempFile(Option<PathBuf>);

impl TempFile {
    fn path(&self) -> &Path {
        self.0.as_deref().expect("not yet kept")
    }

    fn keep(&mut self) -> PathBuf {
        self.0.take().expect("kept once")
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::fs::{self, File, Metadata, OpenOptions};
    use std::io;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, fchown};
    use std::path::Path;

    pub fn set_create_mode(options: &mut OpenOptions, private: bool) {
        options.mode(if private { 0o600 } else { 0o666 });
    }

    /// Renaming over the original keeps everything that matters unless the file has other
    /// hard links (they would keep the old contents) or we can't give the new file the
    /// original's owner and group.
    pub fn can_replace(new: &File, original: &Metadata) -> bool {
        if original.nlink() > 1 {
            return false;
        }
        if fchown(new, Some(original.uid()), Some(original.gid())).is_ok() {
            return true;
        }
        new.metadata()
            .is_ok_and(|m| m.uid() == original.uid() && m.gid() == original.gid())
    }

    pub fn replace(from: &Path, to: &Path, _exists: bool) -> io::Result<()> {
        fs::rename(from, to)
    }

    /// Makes the rename itself durable. Best effort: not every file system supports it.
    pub fn sync_dir(dir: &Path) {
        if let Ok(d) = File::open(dir) {
            let _ = d.sync_all();
        }
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    use std::ffi::c_void;
    use std::fs::{self, File, Metadata, OpenOptions};
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr;

    const REPLACEFILE_IGNORE_MERGE_ERRORS: u32 = 0x2;
    const REPLACEFILE_IGNORE_ACL_ERRORS: u32 = 0x4;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: *mut c_void,
            reserved: *mut c_void,
        ) -> i32;
    }

    pub fn set_create_mode(_options: &mut OpenOptions, _private: bool) {}

    pub fn can_replace(_new: &File, _original: &Metadata) -> bool {
        true
    }

    /// `ReplaceFileW` keeps the original's attributes, ACLs and creation time. It doesn't
    /// work everywhere (some network and FAT drives), so fall back to a plain rename.
    pub fn replace(from: &Path, to: &Path, exists: bool) -> io::Result<()> {
        if exists && replace_file(from, to).is_ok() {
            return Ok(());
        }
        fs::rename(from, to)
    }

    fn replace_file(from: &Path, to: &Path) -> io::Result<()> {
        let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() };
        let (replaced, replacement) = (wide(to), wide(from));
        // SAFETY: both strings are NUL-terminated and outlive the call; the optional
        // arguments are null, which the API allows.
        let ok = unsafe {
            ReplaceFileW(
                replaced.as_ptr(),
                replacement.as_ptr(),
                ptr::null(),
                REPLACEFILE_IGNORE_MERGE_ERRORS | REPLACEFILE_IGNORE_ACL_ERRORS,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        if ok != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    pub fn sync_dir(_dir: &Path) {}
}

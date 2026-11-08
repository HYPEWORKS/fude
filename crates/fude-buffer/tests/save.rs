//! Saving to a real file system.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use fude_buffer::{FileStamp, Format, Rope, SaveError, load, save};

/// A fresh, empty directory under the target directory.
fn temp_dir(name: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "save-{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Names in `dir`, sorted, to check no temporary files are left behind.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn saves_new_and_existing_files() {
    let dir = temp_dir("basic");
    let path = dir.join("a.txt");
    let stamp = save(&path, &Rope::from("first"), &Format::default()).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"first");
    assert_eq!(stamp, FileStamp::of(&path).unwrap());

    save(&path, &Rope::from("second, longer"), &Format::default()).unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"second, longer");
    assert_eq!(entries(&dir), ["a.txt"], "no temporary files left");
}

#[test]
fn unedited_files_round_trip_byte_for_byte() {
    let dir = temp_dir("roundtrip");
    let samples: [(&str, &[u8]); 4] = [
        ("bom-crlf.txt", b"\xEF\xBB\xBFline\r\nline\r\n"),
        ("utf16.txt", b"\xFF\xFEh\0i\0\n\0"),
        (
            "sjis.txt",
            b"\x93\xfa\x96\x7b\x8c\xea\x82\xcc\x83\x65\x83\x4c\x83\x58\x83\x67",
        ),
        ("invalid.txt", b"ok \xFF\xFE \xC3 mixed\r\n\n\r"),
    ];
    for (name, bytes) in samples {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        let loaded = load(&path).unwrap();
        assert!(loaded.exact, "{name}");
        save(&path, &loaded.text, &loaded.format).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes, "{name}");
    }
}

#[test]
fn failed_encoding_leaves_the_original_alone() {
    let dir = temp_dir("unmappable");
    let path = dir.join("latin1.txt");
    fs::write(&path, b"caf\xE9").unwrap();
    let loaded = load(&path).unwrap();
    let mut text = loaded.text.clone();
    text.insert(text.len(), "筆".as_bytes());
    let err = save(&path, &text, &loaded.format).unwrap_err();
    assert!(
        matches!(err, SaveError::Unmappable { ch: '筆', .. }),
        "{err}"
    );
    assert_eq!(fs::read(&path).unwrap(), b"caf\xE9");
    assert_eq!(entries(&dir), ["latin1.txt"]);
}

#[test]
fn read_only_files_are_refused() {
    let dir = temp_dir("readonly");
    let path = dir.join("ro.txt");
    fs::write(&path, b"keep").unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_readonly(true);
    fs::set_permissions(&path, perms.clone()).unwrap();
    let err = save(&path, &Rope::from("changed"), &Format::default()).unwrap_err();
    assert!(matches!(err, SaveError::ReadOnly), "{err}");
    assert_eq!(fs::read(&path).unwrap(), b"keep");
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    fs::set_permissions(&path, perms).unwrap();
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    use super::*;

    #[test]
    fn permissions_are_kept() {
        let dir = temp_dir("perms");
        let path = dir.join("script.sh");
        fs::write(&path, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
        save(
            &path,
            &Rope::from("#!/bin/sh\necho hi\n"),
            &Format::default(),
        )
        .unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o750);
    }

    #[test]
    fn symlinks_are_followed() {
        let dir = temp_dir("symlink");
        let real = dir.join("real.txt");
        let link = dir.join("link.txt");
        fs::write(&real, b"old").unwrap();
        symlink("real.txt", &link).unwrap();
        save(&link, &Rope::from("new"), &Format::default()).unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&real).unwrap(), b"new");

        // A dangling link creates its target.
        let dangling = dir.join("dangling.txt");
        symlink("created.txt", &dangling).unwrap();
        save(&dangling, &Rope::from("made"), &Format::default()).unwrap();
        assert_eq!(fs::read(dir.join("created.txt")).unwrap(), b"made");
        assert!(
            fs::symlink_metadata(&dangling)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn hard_links_keep_sharing_the_file() {
        let dir = temp_dir("hardlink");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, b"old").unwrap();
        fs::hard_link(&a, &b).unwrap();
        let inode = fs::metadata(&a).unwrap().ino();
        save(&a, &Rope::from("new contents"), &Format::default()).unwrap();
        assert_eq!(fs::read(&b).unwrap(), b"new contents");
        assert_eq!(fs::metadata(&a).unwrap().ino(), inode, "written in place");
        assert_eq!(entries(&dir), ["a.txt", "b.txt"]);
    }

    #[test]
    fn unwritable_directory_falls_back_to_writing_in_place() {
        let dir = temp_dir("lockeddir");
        let path = dir.join("f.txt");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let probe = dir.join("probe");
        if fs::write(&probe, b"").is_ok() {
            // Running as root: permissions don't apply, nothing to test.
            let _ = fs::remove_file(probe);
        } else {
            save(&path, &Rope::from("new"), &Format::default()).unwrap();
            assert_eq!(fs::read(&path).unwrap(), b"new");
        }
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

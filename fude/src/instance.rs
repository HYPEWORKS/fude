//! Single instance: launching fude while it's running hands the files to the running
//! window instead of opening another.
//!
//! The running instance listens on a loopback TCP port and writes the port and a random
//! token to a file in the user's state directory (readable only by the user). A new
//! launch reads it, connects, sends the token and its files' absolute paths, waits for an
//! acknowledgement and exits, all before creating a window. If nothing answers (no
//! instance, or a stale file), the new launch becomes the instance. The same code works on
//! every platform, using only the standard library.
//!
//! Becoming the instance means holding a lock on `instance.lock` in the state directory
//! for as long as the process lives (the OS lets go of it when the process ends, however
//! it ends, so it can't go stale). Two launches at the same moment therefore can't both
//! become the instance: the one that doesn't get the lock waits for the other to start
//! listening and hands its files over. The instance is also the one process that keeps
//! the session (see `hot_exit`).

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fude_platform::Waker;

const MAGIC: &str = "fude-open 1";
const TIMEOUT: Duration = Duration::from_secs(2);
/// How long a launch waits for whoever holds the lock to start listening, before it
/// opens a window of its own.
const CLAIM_FOR: Duration = Duration::from_secs(3);
const CLAIM_RETRY: Duration = Duration::from_millis(25);

/// What another launch asked for.
#[derive(Debug, PartialEq, Eq)]
pub struct OpenRequest {
    pub files: Vec<PathBuf>,
}

/// The running instance's side: requests from other launches.
pub struct Server {
    rx: Receiver<OpenRequest>,
    waker: Arc<Mutex<Option<Waker>>>,
    lock: PathBuf,
    token: String,
    /// The lock that makes this process the instance, held until it ends.
    _held: Option<std::fs::File>,
}

/// How a launch that wants to be (or reach) the single instance came out.
pub enum Launch {
    /// A running instance took the files: this launch is done.
    HandedOff,
    /// This process is the instance now.
    Instance(Server),
    /// Another process holds the lock but didn't answer in time: this launch opens a
    /// window of its own, without the session, like `--new-instance`.
    Extra,
}

/// Hands `files` to the running instance, or becomes it.
pub fn claim(state_dir: &Path, files: &[PathBuf]) -> Launch {
    claim_within(state_dir, files, CLAIM_FOR)
}

fn claim_within(state_dir: &Path, files: &[PathBuf], patience: Duration) -> Launch {
    let deadline = Instant::now() + patience;
    loop {
        if hand_off(state_dir, files) {
            return Launch::HandedOff;
        }
        match lock(state_dir) {
            Lock::Held(file) => return Launch::Instance(serve(state_dir, Some(file))),
            // No locks here (some network file systems): as good as it gets without.
            Lock::Unavailable => return Launch::Instance(serve(state_dir, None)),
            // Someone else is the instance and isn't listening yet (or any more).
            Lock::Taken if Instant::now() >= deadline => return Launch::Extra,
            Lock::Taken => std::thread::sleep(CLAIM_RETRY),
        }
    }
}

enum Lock {
    Held(std::fs::File),
    Taken,
    Unavailable,
}

/// Tries to take the instance lock, without waiting for it.
fn lock(state_dir: &Path) -> Lock {
    if std::fs::create_dir_all(state_dir).is_err() {
        return Lock::Unavailable;
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let Ok(file) = options.open(state_dir.join("instance.lock")) else {
        return Lock::Unavailable;
    };
    match file.try_lock() {
        Ok(()) => Lock::Held(file),
        Err(std::fs::TryLockError::WouldBlock) => Lock::Taken,
        Err(std::fs::TryLockError::Error(_)) => Lock::Unavailable,
    }
}

impl Server {
    /// Lets the listener wake the event loop when a request arrives.
    pub fn set_waker(&self, waker: Waker) {
        if let Ok(mut w) = self.waker.lock() {
            *w = Some(waker);
        }
    }

    pub fn try_recv(&self) -> Option<OpenRequest> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Server {
    /// Removes the lock file, if it's still ours.
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.lock)
            .is_ok_and(|s| s.split_whitespace().nth(1) == Some(&self.token))
        {
            let _ = std::fs::remove_file(&self.lock);
        }
    }
}

/// Hands `files` to a running instance. Returns true if one took them.
pub fn hand_off(state_dir: &Path, files: &[PathBuf]) -> bool {
    let Ok(lock) = std::fs::read_to_string(state_dir.join("instance")) else {
        return false;
    };
    let mut parts = lock.split_whitespace();
    let (Some(port), Some(token)) = (
        parts.next().and_then(|p| p.parse::<u16>().ok()),
        parts.next(),
    ) else {
        return false;
    };
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, TIMEOUT) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let mut message = format!("{MAGIC}\n{token}\n");
    for f in files {
        let absolute = std::path::absolute(f).unwrap_or_else(|_| f.clone());
        message.push_str(&encode_path(&absolute));
        message.push('\n');
    }
    message.push('\n');
    if stream.write_all(message.as_bytes()).is_err() {
        return false;
    }
    let mut reply = String::new();
    let _ = stream.take(16).read_to_string(&mut reply);
    reply.trim() == "ok"
}

/// Becomes the instance, holding the lock `held`: listens for other launches. If
/// listening fails, fude still works, just without files being handed over.
fn serve(state_dir: &Path, held: Option<std::fs::File>) -> Server {
    let token = random_token();
    let lock = state_dir.join("instance");
    let (tx, rx) = mpsc::channel();
    let waker: Arc<Mutex<Option<Waker>>> = Arc::new(Mutex::new(None));
    let _ = listen(&lock, &token, tx, waker.clone());
    Server {
        rx,
        waker,
        lock,
        token,
        _held: held,
    }
}

/// Starts the thread that takes requests, and writes where it listens to `lock`.
fn listen(
    lock: &Path,
    token: &str,
    tx: mpsc::Sender<OpenRequest>,
    waker: Arc<Mutex<Option<Waker>>>,
) -> Option<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let port = listener.local_addr().ok()?.port();
    write_private(lock, &format!("{port} {token}\n")).ok()?;
    let wake = waker;
    let expected = token.to_owned();
    std::thread::Builder::new()
        .name("fude-instance".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let Some(request) = read_request(stream, &expected) else {
                    continue;
                };
                if tx.send(request).is_err() {
                    return;
                }
                if let Ok(w) = wake.lock()
                    && let Some(w) = &*w
                {
                    w.wake();
                }
            }
        })
        .ok()?;
    Some(())
}

fn read_request(mut stream: TcpStream, token: &str) -> Option<OpenRequest> {
    let _ = stream.set_read_timeout(Some(TIMEOUT));
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    let mut next = |line: &mut String| -> Option<String> {
        line.clear();
        reader.read_line(line).ok()?;
        Some(line.trim_end_matches(['\r', '\n']).to_owned())
    };
    if next(&mut line)? != MAGIC || next(&mut line)? != token {
        return None;
    }
    let mut files = Vec::new();
    loop {
        let entry = next(&mut line)?;
        if entry.is_empty() {
            break;
        }
        files.push(decode_path(&entry)?);
    }
    let _ = stream.write_all(b"ok\n");
    Some(OpenRequest { files })
}

fn random_token() -> String {
    // RandomState is seeded from the OS's random source.
    let mut token = String::new();
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(i);
        token.push_str(&format!("{:016x}", h.finish()));
    }
    token
}

/// Writes a file only this user can read.
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(contents.as_bytes())
}

/// Paths travel as hex of their native bytes, so any path survives (on Unix, paths need
/// not be UTF-8; on Windows, UTF-16).
fn encode_path(path: &Path) -> String {
    native_bytes(path)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn decode_path(hex: &str) -> Option<PathBuf> {
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    from_native_bytes(bytes)
}

#[cfg(unix)]
fn native_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(unix)]
fn from_native_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(windows)]
fn native_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(windows)]
fn from_native_bytes(bytes: Vec<u8>) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let wide: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .collect();
    Some(PathBuf::from(std::ffi::OsString::from_wide(&wide)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hands_files_to_the_running_instance() {
        let dir = std::env::temp_dir().join(format!("fude-instance-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!hand_off(&dir, &[]), "nobody running yet");
        let Launch::Instance(server) = claim(&dir, &[]) else {
            panic!("the first launch is the instance");
        };
        let files = vec![
            PathBuf::from("/tmp/a b.txt"),
            PathBuf::from("/tmp/日本.txt"),
        ];
        assert!(hand_off(&dir, &files));
        let mut got = None;
        for _ in 0..100 {
            got = server.try_recv();
            if got.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let expected: Vec<PathBuf> = files
            .iter()
            .map(|f| std::path::absolute(f).unwrap())
            .collect();
        assert_eq!(got, Some(OpenRequest { files: expected }));
        drop(server);
        assert!(
            !dir.join("instance").exists(),
            "the lock goes with the instance"
        );
        assert!(
            !hand_off(&dir, &[]),
            "a stale or missing lock means nobody's running"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_token_is_ignored() {
        let dir = std::env::temp_dir().join(format!("fude-instance-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let Launch::Instance(server) = claim(&dir, &[]) else {
            panic!("the first launch is the instance");
        };
        let lock = std::fs::read_to_string(dir.join("instance")).unwrap();
        let port = lock.split_whitespace().next().unwrap();
        std::fs::write(dir.join("instance"), format!("{port} not-the-token\n")).unwrap();
        assert!(!hand_off(&dir, &[PathBuf::from("/x")]));
        std::thread::sleep(Duration::from_millis(50));
        assert!(server.try_recv().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn state(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fude-instance-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn only_one_launch_becomes_the_instance() {
        let dir = state("one");
        let Launch::Instance(server) = claim(&dir, &[]) else {
            panic!("the first launch is the instance");
        };
        let file = PathBuf::from("/tmp/later.txt");
        assert!(matches!(claim(&dir, &[file]), Launch::HandedOff));
        drop(server);
        // The lock goes with the instance: the next launch takes over.
        assert!(matches!(claim(&dir, &[]), Launch::Instance(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn launches_at_the_same_moment_make_one_instance() {
        let dir = state("race");
        let launches: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || claim(&dir, &[]))
            })
            .collect();
        let launches: Vec<Launch> = launches.into_iter().map(|t| t.join().unwrap()).collect();
        let instances = launches
            .iter()
            .filter(|l| matches!(l, Launch::Instance(_)))
            .count();
        let handed = launches
            .iter()
            .filter(|l| matches!(l, Launch::HandedOff))
            .count();
        assert_eq!((instances, handed), (1, 7));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_launch_waits_for_the_lock_holder_to_listen() {
        let dir = state("wait");
        // Another launch has the lock but isn't listening yet.
        let Lock::Held(held) = lock(&dir) else {
            panic!("free at first");
        };
        // Out of patience: a window of its own, without the session.
        assert!(matches!(
            claim_within(&dir, &[], Duration::from_millis(60)),
            Launch::Extra
        ));
        let starting = {
            let dir = dir.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                serve(&dir, Some(held))
            })
        };
        assert!(matches!(
            claim_within(&dir, &[], Duration::from_secs(10)),
            Launch::HandedOff
        ));
        drop(starting.join().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

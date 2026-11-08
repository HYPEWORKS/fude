//! `cargo xtask ui-test`: drives the real app through X11 and checks what it saves.
//!
//! Run it on an X server without a window manager, normally Xvfb:
//!
//!     env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 1920x1080x24" cargo xtask ui-test
//!
//! It types, clicks and uses shortcuts with XTest (synthetic input the server can't tell
//! from a real keyboard and mouse), saves with Ctrl+S after each step, and compares the
//! file on disk with what the step should have produced. Pixel positions assume scale 1
//! and the default 14 px font. With `FUDE_UI_SHOTS=<dir>`, it also saves screenshots of
//! a selection and the unsaved-changes prompt there (needs ImageMagick's `import`).

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, Window};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const MOTION: u8 = 6;

const RETURN: u32 = 0xff0d;
const ESCAPE: u32 = 0xff1b;
const HOME: u32 = 0xff50;
const END: u32 = 0xff57;
const SHIFT: u32 = 0xffe1;
const CONTROL: u32 = 0xffe3;

/// Layout at scale 1 with a 14 px font: where line `n` (0-based) and column `c` are.
const TAB_STRIP: i16 = 36;
const PAD_TOP: i16 = 6;
const LINE_H: i16 = 21;
const TEXT_X: f32 = 62.0;
const COLUMN: f32 = 8.4;

struct Driver {
    conn: RustConnection,
    root: Window,
    window: Window,
    origin: (i16, i16),
    /// keysym → (keycode, needs shift)
    keys: Vec<(u32, u8, bool)>,
}

impl Driver {
    fn sync(&self) -> Result<(), String> {
        self.conn.sync().map_err(|e| e.to_string())
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) -> Result<(), String> {
        self.conn
            .xtest_fake_input(kind, detail, 0, self.root, x, y, 0)
            .map_err(|e| e.to_string())?;
        self.sync()?;
        sleep(Duration::from_millis(15));
        Ok(())
    }

    fn keycode(&self, keysym: u32) -> Result<(u8, bool), String> {
        self.keys
            .iter()
            .find(|k| k.0 == keysym)
            .map(|k| (k.1, k.2))
            .ok_or_else(|| format!("no key for keysym {keysym:#x}"))
    }

    /// Presses `keysym` with modifiers held.
    fn key(&self, mods: &[u32], keysym: u32) -> Result<(), String> {
        let (code, shift) = self.keycode(keysym)?;
        let mut held: Vec<u8> = mods
            .iter()
            .map(|&m| self.keycode(m).map(|k| k.0))
            .collect::<Result<_, _>>()?;
        if shift && !mods.contains(&SHIFT) {
            held.push(self.keycode(SHIFT)?.0);
        }
        for &m in &held {
            self.fake(KEY_PRESS, m, 0, 0)?;
        }
        self.fake(KEY_PRESS, code, 0, 0)?;
        self.fake(KEY_RELEASE, code, 0, 0)?;
        for &m in held.iter().rev() {
            self.fake(KEY_RELEASE, m, 0, 0)?;
        }
        Ok(())
    }

    fn ctrl(&self, c: char) -> Result<(), String> {
        self.key(&[CONTROL], c as u32)
    }

    fn type_text(&self, s: &str) -> Result<(), String> {
        for c in s.chars() {
            self.key(&[], c as u32)?;
        }
        Ok(())
    }

    /// Clicks at a line and column of the text, `count` times.
    fn click(&self, line: usize, column: f32, count: u8) -> Result<(), String> {
        let x = self.origin.0 + (TEXT_X + column * COLUMN) as i16;
        let y = self.origin.1 + TAB_STRIP + PAD_TOP + LINE_H * line as i16 + LINE_H / 2;
        self.fake(MOTION, 0, x, y)?;
        for _ in 0..count {
            self.fake(BUTTON_PRESS, 1, x, y)?;
            self.fake(BUTTON_RELEASE, 1, x, y)?;
        }
        Ok(())
    }
}

/// Finds the top-level window whose title mentions `name`.
fn find_window(conn: &RustConnection, root: Window, name: &str) -> Option<Window> {
    let tree = conn.query_tree(root).ok()?.reply().ok()?;
    tree.children.into_iter().find(|&w| {
        conn.get_property(false, w, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 1024)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some_and(|p| String::from_utf8_lossy(&p.value).contains(name))
    })
}

/// Saves a screenshot of `window` if `FUDE_UI_SHOTS` names a directory.
fn shot(window: Window, name: &str) {
    let Some(dir) = std::env::var_os("FUDE_UI_SHOTS") else {
        return;
    };
    sleep(Duration::from_millis(150));
    let path = Path::new(&dir).join(format!("{name}.png"));
    let ok = Command::new("import")
        .args(["-window", &window.to_string()])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("  (couldn't save screenshot {})", path.display());
    }
}

struct App(Child);

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn run(exe: &Path) -> Result<(), String> {
    let dir = std::env::temp_dir().join(format!("fude-ui-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file: PathBuf = dir.join("ui-test.txt");
    std::fs::write(&file, "first line\nsecond line\n").map_err(|e| e.to_string())?;

    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("no X server: {e}"))?;
    let root = conn.setup().roots[screen].root;
    let mut app = App(Command::new(exe)
        .arg(&file)
        .spawn()
        .map_err(|e| format!("launching {}: {e}", exe.display()))?);

    let deadline = Instant::now() + Duration::from_secs(10);
    let window = loop {
        if let Some(w) = find_window(&conn, root, "ui-test.txt") {
            break w;
        }
        if Instant::now() > deadline {
            return Err("the window didn't appear".into());
        }
        sleep(Duration::from_millis(50));
    };
    sleep(Duration::from_millis(300));
    conn.set_input_focus(x11rb::protocol::xproto::InputFocus::PARENT, window, 0u32)
        .map_err(|e| e.to_string())?;
    let origin = conn
        .translate_coordinates(window, root, 0, 0)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;

    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let map = conn
        .get_keyboard_mapping(min, max - min + 1)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let per = map.keysyms_per_keycode as usize;
    let mut keys = Vec::new();
    for (i, syms) in map.keysyms.chunks(per).enumerate() {
        for (level, &sym) in syms.iter().take(2).enumerate() {
            if sym != 0 && !keys.iter().any(|k: &(u32, u8, bool)| k.0 == sym) {
                keys.push((sym, min + i as u8, level == 1));
            }
        }
    }
    let d = Driver {
        conn,
        root,
        window,
        origin: (origin.dst_x, origin.dst_y),
        keys,
    };
    let _ = d.window;

    let mut failures = 0;
    // Returns whether the file on disk matches.
    let check = |name: &str, expected: &str| -> bool {
        // The save runs on a worker thread; give it a moment.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut actual = String::new();
        while Instant::now() < deadline {
            actual = std::fs::read_to_string(&file).unwrap_or_default();
            if actual == expected {
                break;
            }
            sleep(Duration::from_millis(50));
        }
        if actual == expected {
            println!("  ✓ {name}");
            true
        } else {
            println!("  ✗ {name}\n      expected {expected:?}\n      got      {actual:?}");
            false
        }
    };

    println!("fude UI test ({})", file.display());
    d.key(&[CONTROL], END)?;
    d.type_text("added")?;
    d.ctrl('s')?;
    failures += usize::from(!check(
        "type at the end and save",
        "first line\nsecond line\nadded",
    ));

    d.key(&[CONTROL], HOME)?;
    d.key(&[SHIFT], END)?;
    d.type_text("replaced")?;
    d.ctrl('s')?;
    failures += usize::from(!check(
        "select a line with Shift+End and type over it",
        "replaced\nsecond line\nadded",
    ));

    d.ctrl('z')?;
    d.ctrl('s')?;
    failures += usize::from(!check("undo", "first line\nsecond line\nadded"));

    d.click(1, 2.5, 2)?;
    shot(window, "selection");
    d.type_text("X")?;
    d.ctrl('s')?;
    failures += usize::from(!check(
        "double-click a word and type over it",
        "first line\nX line\nadded",
    ));

    d.click(0, 0.0, 1)?;
    d.click(0, 5.0, 1)?;
    d.key(&[SHIFT], END)?;
    d.ctrl('x')?;
    d.key(&[CONTROL], END)?;
    d.ctrl('v')?;
    d.ctrl('s')?;
    failures += usize::from(!check(
        "click, select, cut and paste",
        "first\nX line\nadded line",
    ));

    d.key(&[], RETURN)?;
    d.type_text("unsaved")?;
    d.ctrl('q')?;
    sleep(Duration::from_millis(200));
    shot(window, "prompt");
    d.key(&[], ESCAPE)?;
    sleep(Duration::from_millis(200));
    let still_open = app.0.try_wait().map_err(|e| e.to_string())?.is_none();
    if still_open {
        println!("  ✓ quitting with unsaved changes asks first (and Escape cancels)");
    } else {
        println!("  ✗ the app quit without asking about unsaved changes");
        failures += 1;
    }
    d.ctrl('q')?;
    sleep(Duration::from_millis(200));
    d.type_text("d")?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut exited = false;
    while Instant::now() < deadline {
        if app.0.try_wait().map_err(|e| e.to_string())?.is_some() {
            exited = true;
            break;
        }
        sleep(Duration::from_millis(50));
    }
    if exited {
        println!("  ✓ \"Don't Save\" quits");
    } else {
        println!("  ✗ the app didn't quit after \"Don't Save\"");
        failures += 1;
    }
    failures += usize::from(!check(
        "the unsaved edit was not written",
        "first\nX line\nadded line",
    ));

    let _ = std::fs::remove_dir_all(&dir);
    if failures > 0 {
        return Err(format!("{failures} UI checks failed"));
    }
    println!("all UI checks passed");
    Ok(())
}

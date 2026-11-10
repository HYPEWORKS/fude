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
const ALT: u32 = 0xffe9;
const TAB: u32 = 0xff09;
const DOWN: u32 = 0xff54;

/// Layout at scale 1 with a 14 px font: where line `n` (0-based) and column `c` are,
/// below the menu bar and the tab strip.
const MENU_BAR: i16 = 28;
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

    /// Clicks at a point of the window.
    fn click_at(&self, x: i16, y: i16) -> Result<(), String> {
        let (x, y) = (self.origin.0 + x, self.origin.1 + y);
        self.fake(MOTION, 0, x, y)?;
        self.fake(BUTTON_PRESS, 1, x, y)?;
        self.fake(BUTTON_RELEASE, 1, x, y)
    }

    /// Clicks at a line and column of the text, `count` times.
    fn click(&self, line: usize, column: f32, count: u8) -> Result<(), String> {
        let x = self.origin.0 + (TEXT_X + column * COLUMN) as i16;
        let y = self.origin.1 + MENU_BAR + TAB_STRIP + PAD_TOP + LINE_H * line as i16 + LINE_H / 2;
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

/// Waits up to 10 s for a window whose title mentions `name`.
fn wait_window(conn: &RustConnection, root: Window, name: &str) -> Option<Window> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(w) = find_window(conn, root, name) {
            return Some(w);
        }
        if Instant::now() > deadline {
            return None;
        }
        sleep(Duration::from_millis(50));
    }
}

/// Waits up to 3 s for a process to exit; returns whether it did.
fn exits(child: &mut Child) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return true;
        }
        sleep(Duration::from_millis(50));
    }
    false
}

/// Phase 4 of the UI test. Returns how many checks failed.
fn power_features(
    exe: &Path,
    dir: &Path,
    d: &mut Driver,
    focus: &dyn Fn(&mut Driver, &str) -> Result<(), String>,
) -> Result<usize, String> {
    let file = dir.join("power.txt");
    std::fs::write(&file, "cat dog cat\nalpha\nbeta\n").map_err(|e| e.to_string())?;
    let config = dir.join("config4").join("fude");
    std::fs::create_dir_all(&config).map_err(|e| e.to_string())?;
    std::fs::write(
        config.join("fude.toml"),
        "[[tool]]\nname = \"Shout\"\ncommand = [\"tr\", \"a-z\", \"A-Z\"]\nkey = \"ctrl+alt+u\"\n",
    )
    .map_err(|e| e.to_string())?;
    let mut app = App(Command::new(exe)
        .arg("--new-instance")
        .arg(&file)
        .env("XDG_CONFIG_HOME", dir.join("config4"))
        .env("XDG_STATE_HOME", dir.join("state4"))
        .spawn()
        .map_err(|e| format!("launching {}: {e}", exe.display()))?);
    focus(d, "power.txt")?;
    let mut failures = 0;
    let mut check = |d: &Driver, name: &str, expected: &str| -> Result<(), String> {
        d.ctrl('s')?;
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
        } else {
            println!("  ✗ {name}\n      expected {expected:?}\n      got      {actual:?}");
            failures += 1;
        }
        Ok(())
    };

    d.click(0, 1.0, 1)?;
    d.ctrl('d')?;
    d.ctrl('d')?;
    d.type_text("cow")?;
    check(
        d,
        "Ctrl+D twice selects both \"cat\"s; typing replaces them",
        "cow dog cow\nalpha\nbeta\n",
    )?;

    d.key(&[], ESCAPE)?;
    d.click(1, 2.0, 1)?;
    d.key(&[ALT], DOWN)?;
    check(
        d,
        "Alt+Down moves a line down",
        "cow dog cow\nbeta\nalpha\n",
    )?;

    d.key(&[CONTROL, SHIFT], 'd' as u32)?;
    check(
        d,
        "Ctrl+Shift+D duplicates it",
        "cow dog cow\nbeta\nalpha\nalpha\n",
    )?;

    d.key(&[CONTROL], '/' as u32)?;
    check(
        d,
        "Ctrl+/ comments the copy out",
        "cow dog cow\nbeta\nalpha\n# alpha\n",
    )?;

    d.key(&[CONTROL], HOME)?;
    d.key(&[SHIFT, ALT], DOWN)?;
    d.key(&[SHIFT, ALT], DOWN)?;
    d.type_text(">")?;
    check(
        d,
        "Shift+Alt+Down makes a column of cursors",
        ">cow dog cow\n>beta\n>alpha\n# alpha\n",
    )?;

    d.key(&[], ESCAPE)?;
    d.ctrl('a')?;
    d.key(&[CONTROL, SHIFT], 'p' as u32)?;
    d.type_text("sort lines (a to")?;
    d.key(&[], RETURN)?;
    check(
        d,
        "the command palette sorts the lines",
        "# alpha\n>alpha\n>beta\n>cow dog cow\n",
    )?;

    d.ctrl('a')?;
    d.key(&[CONTROL, ALT], 'u' as u32)?;
    // The external tool runs on a worker; give it a moment.
    sleep(Duration::from_millis(800));
    check(
        d,
        "an external tool (tr a-z A-Z) from the settings, on its own shortcut",
        "# ALPHA\n>ALPHA\n>BETA\n>COW DOG COW\n",
    )?;

    d.key(&[CONTROL], END)?;
    d.type_text("if x {")?;
    d.key(&[], RETURN)?;
    d.type_text("y")?;
    d.key(&[], RETURN)?;
    d.type_text("}")?;
    check(
        d,
        "Enter indents after {, and } outdents",
        "# ALPHA\n>ALPHA\n>BETA\n>COW DOG COW\nif x {\n\ty\n}",
    )?;

    // Alt+T opens the Tools menu on its first item; lowercase is ten items down.
    d.key(&[ALT], 't' as u32)?;
    for _ in 0..10 {
        d.key(&[], DOWN)?;
    }
    d.key(&[], RETURN)?;
    check(
        d,
        "Alt+T, then Down to lowercase and Enter, runs it from the Tools menu",
        "# alpha\n>alpha\n>beta\n>cow dog cow\nif x {\n\ty\n}",
    )?;

    // Edit is the second title (13 px font: "File" ends near x = 56); Undo is its first
    // item, just below the bar.
    d.click_at(82, MENU_BAR / 2)?;
    d.click_at(90, MENU_BAR + 21)?;
    check(
        d,
        "clicking Edit, then Undo, undoes it",
        "# ALPHA\n>ALPHA\n>BETA\n>COW DOG COW\nif x {\n\ty\n}",
    )?;

    d.ctrl('q')?;
    if !exits(&mut app.0) {
        println!("  ✗ the app didn't quit");
        failures += 1;
    }
    Ok(failures)
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
    // One long line, for word wrap.
    let wrap_file: PathBuf = dir.join("wrap-test.txt");
    let long_line = "word ".repeat(60);
    std::fs::write(&wrap_file, &long_line).map_err(|e| e.to_string())?;

    // Phase 1 tests the unsaved-changes prompt, so hot exit is off.
    let config_dir = dir.join("config").join("fude");
    std::fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    std::fs::write(config_dir.join("fude.toml"), "hot_exit = false\n")
        .map_err(|e| e.to_string())?;

    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("no X server: {e}"))?;
    let root = conn.setup().roots[screen].root;
    let mut app = App(Command::new(exe)
        .arg(&wrap_file)
        .arg(&file)
        // Its own settings and session, so the user's don't change what's tested.
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
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

    d.ctrl('h')?;
    d.type_text("line")?;
    d.key(&[], TAB)?;
    d.type_text("row")?;
    shot(window, "find");
    d.key(&[CONTROL, ALT], RETURN)?;
    sleep(Duration::from_millis(300));
    d.key(&[], ESCAPE)?;
    d.ctrl('s')?;
    failures += usize::from(!check("replace all (Ctrl+H)", "first\nX row\nadded row"));

    d.ctrl('n')?;
    d.type_text("scratch")?;
    d.ctrl('w')?;
    sleep(Duration::from_millis(200));
    d.type_text("d")?;
    d.key(&[CONTROL], END)?;
    d.type_text("!")?;
    d.ctrl('s')?;
    failures += usize::from(!check(
        "a new tab closes without saving, back to the file's tab",
        "first\nX row\nadded row!",
    ));

    d.ctrl('g')?;
    d.type_text("2")?;
    d.key(&[], RETURN)?;
    d.type_text("Z")?;
    d.ctrl('s')?;
    failures += usize::from(!check("go to line (Ctrl+G)", "first\nZX row\nadded row!"));

    d.key(&[CONTROL, SHIFT], 'p' as u32)?;
    d.type_text("sel all")?;
    shot(window, "palette");
    d.key(&[], RETURN)?;
    d.type_text("fin")?;
    d.ctrl('s')?;
    failures += usize::from(!check("command palette: Select All, then type", "fin"));

    // Word wrap: Down from the start of a long line stays in it, on its second row.
    d.ctrl('1')?;
    d.key(&[ALT], 'z' as u32)?;
    d.key(&[CONTROL], HOME)?;
    d.key(&[], DOWN)?;
    d.type_text("X")?;
    d.ctrl('s')?;
    let wrapped = std::fs::read_to_string(&wrap_file).unwrap_or_default();
    let x = wrapped.find('X');
    let ok = x.is_some_and(|i| i > 0 && i + 1 < wrapped.len() && wrapped[..i].ends_with(' '))
        && wrapped.replace('X', "") == long_line;
    if ok {
        println!("  ✓ word wrap (Alt+Z): Down moves to the next row of a long line");
    } else {
        println!("  ✗ word wrap: X landed at {x:?} in {wrapped:?}");
        failures += 1;
    }
    d.ctrl('2')?;

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
    failures += usize::from(!check("the unsaved edit was not written", "fin"));

    // Phase 2: hot exit and single instance, with a fresh session.
    std::fs::write(config_dir.join("fude.toml"), "hot_exit = true\n").map_err(|e| e.to_string())?;
    let launch = |args: &[&Path]| -> Result<Child, String> {
        Command::new(exe)
            .args(args)
            .env("XDG_CONFIG_HOME", dir.join("config"))
            .env("XDG_STATE_HOME", dir.join("state2"))
            .spawn()
            .map_err(|e| format!("launching {}: {e}", exe.display()))
    };
    let mut d = d;
    let focus = |d: &mut Driver, name: &str| -> Result<(), String> {
        let w = wait_window(&d.conn, d.root, name).ok_or(format!("no window for {name}"))?;
        sleep(Duration::from_millis(300));
        d.window = w;
        d.conn
            .set_input_focus(x11rb::protocol::xproto::InputFocus::PARENT, w, 0u32)
            .map_err(|e| e.to_string())?;
        d.sync()
    };
    let mut second = App(launch(&[&file])?);
    focus(&mut d, "ui-test.txt")?;
    d.key(&[CONTROL], END)?;
    d.type_text("#")?;
    d.ctrl('q')?;
    if exits(&mut second.0) {
        println!("  ✓ with hot exit, quitting doesn't ask");
    } else {
        println!("  ✗ with hot exit, the app didn't quit");
        failures += 1;
    }

    let mut third = App(launch(&[])?);
    focus(&mut d, "ui-test.txt")?;
    let other = dir.join("other.txt");
    std::fs::write(&other, "other\n").map_err(|e| e.to_string())?;
    let mut handed = launch(&[&other])?;
    let handed_ok = exits(&mut handed) && handed.wait().is_ok_and(|s| s.success());
    let switched = wait_window(&d.conn, d.root, "other.txt").is_some();
    if handed_ok && switched {
        println!("  ✓ a second launch hands its file to the running window and exits");
    } else {
        println!(
            "  ✗ single instance: second launch exited {handed_ok}, window switched {switched}"
        );
        failures += 1;
    }
    // Back to the restored file, which should still have its unsaved edit; save it.
    focus(&mut d, "other.txt")?;
    d.key(&[CONTROL, SHIFT], TAB)?;
    d.ctrl('s')?;
    failures += usize::from(!check("unsaved changes come back after hot exit", "fin#"));

    // A crash: the session written in the background a moment after typing survives it.
    d.ctrl('2')?;
    d.type_text("@")?;
    // The session is written 2 s after a change; leave room for slow CI machines.
    sleep(Duration::from_millis(4500));
    let _ = third.0.kill();
    let _ = third.0.wait();
    let mut fourth = App(launch(&[])?);
    focus(&mut d, "other.txt")?;
    d.ctrl('s')?;
    let recovered = std::fs::read_to_string(&other).unwrap_or_default();
    if recovered == "@other\n" {
        println!("  ✓ unsaved typing survives a crash (killed 4.5 s after typing)");
    } else {
        println!("  ✗ after a crash, other.txt saved as {recovered:?}");
        failures += 1;
    }
    d.ctrl('q')?;
    if !exits(&mut fourth.0) {
        println!("  ✗ the recovered app didn't quit");
        failures += 1;
    }

    // Phase 3: a large file, loaded in the background, ending in a 2 MB line.
    let big = dir.join("big.log");
    let mut text = String::with_capacity(42 << 20);
    for i in 0..500_000 {
        use std::fmt::Write as _;
        let _ = writeln!(
            text,
            "{i:08} INFO worker request took {} ms: the quick brown fox",
            i % 977
        );
    }
    text.push_str(&"{\"k\":1},".repeat(2 << 20 >> 3));
    std::fs::write(&big, &text).map_err(|e| e.to_string())?;
    let mut fifth = App(Command::new(exe)
        .arg("--new-instance")
        .arg(&big)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state3"))
        .spawn()
        .map_err(|e| format!("launching {}: {e}", exe.display()))?);
    focus(&mut d, "big.log")?;
    // Loading takes well under a second; until then, typing is refused.
    sleep(Duration::from_millis(1500));
    d.key(&[CONTROL], END)?;
    d.type_text("!")?;
    d.ctrl('s')?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let expected = text.len() + 1;
    let mut saved = Vec::new();
    while Instant::now() < deadline {
        saved = std::fs::read(&big).unwrap_or_default();
        if saved.len() == expected {
            break;
        }
        sleep(Duration::from_millis(100));
    }
    if saved.len() == expected && saved.ends_with(b",!") && saved.starts_with(b"00000000 INFO") {
        println!("  ✓ a 40 MB file loads, and typing at the end of its 2 MB last line saves");
    } else {
        println!(
            "  ✗ large file: saved {} bytes (expected {expected}), ends {:?}",
            saved.len(),
            String::from_utf8_lossy(&saved[saved.len().saturating_sub(8)..])
        );
        failures += 1;
    }
    d.ctrl('q')?;
    if !exits(&mut fifth.0) {
        println!("  ✗ the app with the large file didn't quit");
        failures += 1;
    }

    // Phase 4: multiple cursors, line commands, comments, column selection, tools.
    failures += power_features(exe, &dir, &mut d, &focus)?;

    let _ = std::fs::remove_dir_all(&dir);
    if failures > 0 {
        return Err(format!("{failures} UI checks failed"));
    }
    println!("all UI checks passed");
    Ok(())
}

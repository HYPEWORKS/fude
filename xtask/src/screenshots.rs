//! `cargo xtask screenshots`: the README's screenshots, taken from the real window.
//!
//! Run it on an X server without a window manager, normally Xvfb, with a screen larger
//! than the window (1100×720 at scale 2):
//!
//!     env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 2600x1700x24 -noreset" cargo xtask screenshots
//!
//! Each scene opens the files in `docs/screenshots/demo` in a fresh fude with its own
//! settings (theme and hot exit off) and session, drives it with the UI test's XTest
//! driver, and saves the window with ImageMagick's `import`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread::sleep;
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, InputFocus};

use crate::ui_test::{ALT, CONTROL, Driver, END, ESCAPE, RETURN, SHIFT, TAB, wait_window};

/// What a scene does once the window is up.
type Steps = fn(&Driver) -> Result<(), String>;

/// File name, theme, steps.
const SCENES: &[(&str, &str, Steps)] = &[
    ("editor-dark", "dark", |d| {
        go_to_line(d, 47)?;
        d.key(&[], END)
    }),
    ("command-palette", "dark", |d| {
        d.key(&[CONTROL, SHIFT], 'p' as u32)?;
        d.type_text("sort")
    }),
    ("find-replace-regex", "light", |d| {
        // Scroll to the end and back, so the matches sit in the middle of the view.
        go_to_line(d, 62)?;
        go_to_line(d, 36)?;
        d.ctrl('h')?;
        d.type_text(r"item\.(\w+)")?;
        d.key(&[], TAB)?;
        d.type_text("self.$1")?;
        d.key(&[ALT], 'r' as u32)
    }),
    ("multiple-cursors", "light", |d| {
        d.ctrl('f')?;
        d.type_text("price_cents")?;
        d.key(&[], RETURN)?;
        d.key(&[], ESCAPE)?;
        d.key(&[CONTROL, SHIFT], 'l' as u32)?;
        d.type_text("unit_price")
    }),
];

/// The demo files, in tab order; the last is the one shown.
const FILES: &[&str] = &["notes.md", "fude.toml", "inventory.rs"];

fn go_to_line(d: &Driver, line: usize) -> Result<(), String> {
    d.ctrl('g')?;
    d.type_text(&line.to_string())?;
    d.key(&[], RETURN)
}

struct App(Child);

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn run(exe: &Path, root_dir: &Path, out: &Path, only: &[String]) -> Result<(), String> {
    let demo = root_dir.join("docs/screenshots/demo");
    let tmp = std::env::temp_dir().join(format!("fude-screenshots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;

    for &(name, theme, steps) in SCENES {
        if !only.is_empty() && !only.iter().any(|o| o == name) {
            continue;
        }
        let path = out.join(format!("{name}.png"));
        scene(exe, &demo, &tmp.join(name), theme, steps, &path)
            .map_err(|e| format!("{name}: {e}"))?;
        println!("  {}", path.display());
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

fn scene(
    exe: &Path,
    demo: &Path,
    dir: &Path,
    theme: &str,
    steps: Steps,
    path: &Path,
) -> Result<(), String> {
    let config = dir.join("config").join("fude");
    std::fs::create_dir_all(&config).map_err(|e| e.to_string())?;
    std::fs::write(
        config.join("fude.toml"),
        format!("theme = \"{theme}\"\nhot_exit = false\n"),
    )
    .map_err(|e| e.to_string())?;

    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("no X server: {e}"))?;
    let root = conn.setup().roots[screen].root;
    let files: Vec<PathBuf> = FILES.iter().map(|f| demo.join(f)).collect();
    let _app = App(Command::new(exe)
        .args(["--new-instance", "--wait"])
        .args(&files)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_STATE_HOME", dir.join("state"))
        // Sharp text for high-density screens.
        .env("WINIT_X11_SCALE_FACTOR", "2")
        .spawn()
        .map_err(|e| format!("launching {}: {e}", exe.display()))?);

    let window =
        wait_window(&conn, root, FILES[FILES.len() - 1]).ok_or("the window didn't appear")?;
    sleep(Duration::from_millis(500));
    conn.set_input_focus(InputFocus::PARENT, window, 0u32)
        .map_err(|e| e.to_string())?;
    let origin = conn
        .translate_coordinates(window, root, 0, 0)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let d = Driver::new(conn, root, window, (origin.dst_x, origin.dst_y))?;
    steps(&d)?;
    // Carets blink for 10 s after the last key, then stay on: wait that out, so every
    // shot shows them.
    sleep(Duration::from_millis(10_500));

    let ok = Command::new("import")
        .args(["-window", &window.to_string()])
        .arg(path)
        .status()
        .map_err(|e| format!("running ImageMagick's import: {e}"))?
        .success();
    if ok {
        Ok(())
    } else {
        Err("import failed".into())
    }
}

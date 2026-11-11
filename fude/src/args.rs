use std::ffi::OsString;
use std::path::PathBuf;

pub const USAGE: &str = "\
fude: Fast, Uncomplicated Document Editor

Usage: fude [OPTIONS] [FILE]...

Options:
  -w, --wait             Stay in the terminal until the window closes, in a window of its
                         own (for git: core.editor = 'fude --wait')
  -n, --new-instance     Open a new window instead of handing the files to a running fude
                         (like --wait, it keeps no session and asks before closing with
                         unsaved changes)
  -h, --help             Print help
  -V, --version          Print version

Development:
  --bench-startup        Exit after the first frame is presented and print timings
  --screenshot <PATH>    Render one frame without a window, save it as PNG, and exit
  --bench-render         Render frames without a window and print timings
  --bench-typing         Type into the document without a window: time per keystroke,
                         from edit to finished frame
  --bench-file <PATH>    Open a (large) file without a window: time the first screen,
                         loading, scrolling and typing, and measure memory
  --check                With --bench-file, fail when a budget is missed
  --size <WxH>           Canvas size in physical pixels for the modes without a
                         window [default: 1280x800]
  --scale <FACTOR>       Scale factor for the modes without a window [default: 1]";

#[derive(Debug, PartialEq)]
pub enum Mode {
    Window,
    Help,
    Version,
    Screenshot(PathBuf),
    BenchRender,
    BenchTyping,
    BenchFile(PathBuf),
}

#[derive(Debug, PartialEq)]
pub struct Args {
    pub mode: Mode,
    pub files: Vec<PathBuf>,
    pub bench_startup: bool,
    pub new_instance: bool,
    /// Stay in the foreground (don't detach from the terminal), in a new instance.
    pub wait: bool,
    /// Internal: this process was started detached by a launch from a terminal.
    pub detached: bool,
    pub size: (u32, u32),
    pub scale: f32,
    /// Benchmarks fail when over budget.
    pub check: bool,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let mut out = Args {
        mode: Mode::Window,
        files: Vec::new(),
        bench_startup: false,
        new_instance: false,
        wait: false,
        detached: false,
        size: (1280, 800),
        scale: 1.0,
        check: false,
    };
    let mut args = args.into_iter();
    let mut only_files = false;
    while let Some(arg) = args.next() {
        let flag = if only_files { None } else { arg.to_str() };
        let mut value = |name: &str| args.next().ok_or_else(|| format!("{name} needs a value"));
        match flag {
            Some("-h" | "--help") => out.mode = Mode::Help,
            Some("-V" | "--version") => out.mode = Mode::Version,
            Some("--bench-startup") => out.bench_startup = true,
            Some("-n" | "--new-instance") => out.new_instance = true,
            Some("-w" | "--wait") => out.wait = true,
            Some("--detached") => out.detached = true,
            Some("--bench-render") => out.mode = Mode::BenchRender,
            Some("--bench-typing") => out.mode = Mode::BenchTyping,
            Some("--bench-file") => out.mode = Mode::BenchFile(value("--bench-file")?.into()),
            Some("--check") => out.check = true,
            Some("--screenshot") => out.mode = Mode::Screenshot(value("--screenshot")?.into()),
            Some("--size") => {
                let v = value("--size")?;
                out.size = parse_size(v.to_str().unwrap_or(""))
                    .ok_or_else(|| format!("invalid --size {v:?}, expected WxH like 1920x1080"))?;
            }
            Some("--scale") => {
                let v = value("--scale")?;
                out.scale = v
                    .to_str()
                    .and_then(|s| s.parse::<f32>().ok())
                    .filter(|s| (0.5..=8.0).contains(s))
                    .ok_or_else(|| {
                        format!("invalid --scale {v:?}, expected a number from 0.5 to 8")
                    })?;
            }
            Some("--") => only_files = true,
            Some(f) if f.starts_with('-') && f.len() > 1 => {
                return Err(format!("unknown option {f}"));
            }
            _ => out.files.push(arg.into()),
        }
    }
    Ok(out)
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    ((1..=16384).contains(&w) && (1..=16384).contains(&h)).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn defaults() {
        let a = p(&[]).unwrap();
        assert_eq!(a.mode, Mode::Window);
        assert_eq!(a.size, (1280, 800));
        assert!(a.files.is_empty());
    }

    #[test]
    fn file_and_flags() {
        let a = p(&[
            "--size",
            "3840x2160",
            "--scale",
            "2",
            "notes.txt",
            "--bench-startup",
        ])
        .unwrap();
        assert_eq!(a.size, (3840, 2160));
        assert_eq!(a.scale, 2.0);
        assert!(a.bench_startup);
        assert_eq!(a.files, [PathBuf::from("notes.txt")]);
    }

    #[test]
    fn double_dash_allows_dash_filenames() {
        assert_eq!(
            p(&["--", "-weird.txt", "b.txt"]).unwrap().files,
            [PathBuf::from("-weird.txt"), PathBuf::from("b.txt")]
        );
    }

    #[test]
    fn errors() {
        assert!(p(&["--size", "big"]).is_err());
        assert!(p(&["--scale"]).is_err());
        assert!(p(&["--nope"]).is_err());
    }
}

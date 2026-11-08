use std::ffi::OsString;
use std::path::PathBuf;

pub const USAGE: &str = "\
fude: Fast, Uncomplicated Document Editor

Usage: fude [OPTIONS] [FILE]

Options:
  -h, --help             Print help
  -V, --version          Print version

Development:
  --bench-startup        Exit after the first frame is presented and print timings
  --screenshot <PATH>    Render one frame without a window, save it as PNG, and exit
  --bench-render         Render frames without a window and print timings
  --size <WxH>           Canvas size in physical pixels for --screenshot and
                         --bench-render [default: 1280x800]
  --scale <FACTOR>       Scale factor for --screenshot and --bench-render [default: 1]";

#[derive(Debug, PartialEq)]
pub enum Mode {
    Window,
    Help,
    Version,
    Screenshot(PathBuf),
    BenchRender,
}

#[derive(Debug, PartialEq)]
pub struct Args {
    pub mode: Mode,
    pub file: Option<PathBuf>,
    pub bench_startup: bool,
    pub size: (u32, u32),
    pub scale: f32,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Args, String> {
    let mut out = Args {
        mode: Mode::Window,
        file: None,
        bench_startup: false,
        size: (1280, 800),
        scale: 1.0,
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
            Some("--bench-render") => out.mode = Mode::BenchRender,
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
            _ => {
                if out.file.is_some() {
                    return Err("only one file can be opened for now".into());
                }
                out.file = Some(arg.into());
            }
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
        assert_eq!(a.file, None);
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
        assert_eq!(a.file, Some(PathBuf::from("notes.txt")));
    }

    #[test]
    fn double_dash_allows_dash_filenames() {
        assert_eq!(
            p(&["--", "-weird.txt"]).unwrap().file,
            Some(PathBuf::from("-weird.txt"))
        );
    }

    #[test]
    fn errors() {
        assert!(p(&["--size", "big"]).is_err());
        assert!(p(&["--scale"]).is_err());
        assert!(p(&["--nope"]).is_err());
        assert!(p(&["a.txt", "b.txt"]).is_err());
    }
}

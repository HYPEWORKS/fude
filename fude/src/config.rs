//! Settings, from a TOML file that opens in fude like any other document and applies
//! when saved.

use std::path::PathBuf;

use fude_platform::OsTheme;
use fude_ui::{SUMI, Theme, WASHI};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    /// Follow the OS's light or dark appearance.
    Auto,
    Dark,
    Light,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub font_size: f32,
    pub tab_width: usize,
    /// The Tab key inserts spaces up to the next tab stop.
    pub insert_spaces: bool,
    pub word_wrap: bool,
    pub theme: ThemeChoice,
    /// Quit without asking about unsaved changes; they come back on the next launch.
    pub hot_exit: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            font_size: 14.0,
            tab_width: 4,
            insert_spaces: false,
            word_wrap: false,
            theme: ThemeChoice::Auto,
            hot_exit: true,
        }
    }
}

/// What a new settings file contains: every setting, with its default.
pub const TEMPLATE: &str = r#"# fude settings. Save this file to apply changes.

# Editor font size in points.
font_size = 14

# Columns between tab stops.
tab_width = 4

# Whether the Tab key inserts spaces (up to the next tab stop) instead of a tab.
insert_spaces = false

# Wrap long lines at the window's edge.
word_wrap = false

# "auto" follows the system's light or dark appearance; or "dark" or "light".
theme = "auto"

# Quit without asking about unsaved changes. They are kept, and come back the next
# time fude starts.
hot_exit = true
"#;

impl Config {
    /// Reads the settings file. A missing file means defaults; a broken one also gives
    /// defaults (as far as it couldn't be read) and messages saying what's wrong.
    pub fn load() -> (Config, Vec<String>) {
        let Some(path) = config_path() else {
            return (Config::default(), Vec::new());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Config::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Config::default(), Vec::new()),
            Err(e) => (
                Config::default(),
                vec![format!("couldn't read settings: {e}")],
            ),
        }
    }

    pub fn parse(text: &str) -> (Config, Vec<String>) {
        let mut config = Config::default();
        let mut problems = Vec::new();
        let table: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(e) => {
                let first = e.to_string().lines().next().unwrap_or("").to_owned();
                return (config, vec![format!("settings: {first}")]);
            }
        };
        for (key, value) in &table {
            let ok = match key.as_str() {
                "font_size" => number(value).map(|n| config.font_size = n.clamp(6.0, 72.0)),
                "tab_width" => number(value).map(|n| config.tab_width = (n as usize).clamp(1, 16)),
                "insert_spaces" => value.as_bool().map(|b| config.insert_spaces = b),
                "word_wrap" => value.as_bool().map(|b| config.word_wrap = b),
                "hot_exit" => value.as_bool().map(|b| config.hot_exit = b),
                "theme" => match value.as_str() {
                    Some("auto") => Some(ThemeChoice::Auto),
                    Some("dark") => Some(ThemeChoice::Dark),
                    Some("light") => Some(ThemeChoice::Light),
                    _ => None,
                }
                .map(|theme| config.theme = theme),
                _ => {
                    problems.push(format!("settings: unknown setting `{key}`"));
                    continue;
                }
            };
            if ok.is_none() {
                problems.push(format!("settings: `{key}` has the wrong kind of value"));
            }
        }
        (config, problems)
    }

    pub fn theme(&self, os: Option<OsTheme>) -> &'static Theme {
        match (self.theme, os) {
            (ThemeChoice::Light, _) | (ThemeChoice::Auto, Some(OsTheme::Light)) => &WASHI,
            _ => &SUMI,
        }
    }
}

fn number(value: &toml::Value) -> Option<f32> {
    value
        .as_float()
        .map(|f| f as f32)
        .or_else(|| value.as_integer().map(|i| i as f32))
}

/// Where settings live: `$XDG_CONFIG_HOME/fude` (usually `~/.config/fude`) on Linux and
/// the BSDs, `~/Library/Application Support/fude` on macOS, `%APPDATA%\fude` on Windows.
pub fn config_dir() -> Option<PathBuf> {
    platform_dir(Kind::Config)
}

/// Where session data (open files, unsaved changes) lives: `$XDG_STATE_HOME/fude`
/// (usually `~/.local/state/fude`), `~/Library/Application Support/fude`, or
/// `%LOCALAPPDATA%\fude`.
pub fn state_dir() -> Option<PathBuf> {
    platform_dir(Kind::State)
}

pub fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("fude.toml"))
}

enum Kind {
    Config,
    State,
}

fn platform_dir(kind: Kind) -> Option<PathBuf> {
    let env = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        match kind {
            Kind::Config => env("APPDATA")?,
            Kind::State => env("LOCALAPPDATA")?,
        }
    } else if cfg!(target_os = "macos") {
        env("HOME")?.join("Library/Application Support")
    } else {
        match kind {
            Kind::Config => {
                env("XDG_CONFIG_HOME").or_else(|| Some(env("HOME")?.join(".config")))?
            }
            Kind::State => {
                env("XDG_STATE_HOME").or_else(|| Some(env("HOME")?.join(".local/state")))?
            }
        }
    };
    Some(base.join("fude"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_is_the_defaults() {
        let (config, problems) = Config::parse(TEMPLATE);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn values_and_problems() {
        let (config, problems) = Config::parse(
            "font_size = 16.5\ntab_width = 2\ntheme = \"light\"\nword_wrap = \"yes\"\ncolour = 1\n",
        );
        assert_eq!(config.font_size, 16.5);
        assert_eq!(config.tab_width, 2);
        assert_eq!(config.theme, ThemeChoice::Light);
        assert!(!config.word_wrap, "the bad value is ignored");
        assert_eq!(problems.len(), 2);

        let (config, problems) = Config::parse("font_size = = 3");
        assert_eq!(config, Config::default());
        assert!(problems[0].contains("line 1"), "{problems:?}");
    }
}

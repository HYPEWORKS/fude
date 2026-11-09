//! Settings, from a TOML file that opens in fude like any other document and applies
//! when saved.

use std::path::PathBuf;
use std::time::Duration;

use fude_platform::OsTheme;
use fude_ui::{SUMI, Shortcut, Theme, WASHI};

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
    /// Ask before opening files larger than this (MB).
    pub large_file_mb: u64,
    /// External tools (`[[tool]]` tables).
    pub tools: Vec<ExternalTool>,
}

/// A program run as a command: it gets the selection or document on standard input, and
/// what it prints is used as `output` says.
#[derive(Clone, Debug, PartialEq)]
pub struct ExternalTool {
    pub name: String,
    /// The program and its arguments; `{file}`, `{dir}`, `{name}` and `{line}` are
    /// replaced with the document's path, folder, file name and the cursor's line.
    pub command: Vec<String>,
    pub input: ToolInput,
    pub output: ToolOutput,
    pub key: Option<Shortcut>,
    /// The program is stopped if it runs longer than this.
    pub timeout: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolInput {
    /// The selections (each run on its own), or the whole document if nothing is selected.
    SelectionOrDocument,
    Selection,
    Document,
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolOutput {
    /// Replaces the input.
    Replace,
    /// Goes in at the cursor.
    Insert,
    NewTab,
    /// Shown in the status bar.
    Message,
    Nothing,
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
            large_file_mb: 1024,
            tools: Vec::new(),
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

# Ask before opening a file larger than this many MB. Open files take about as much
# memory as their size.
large_file_mb = 1024

# External tools run a program on the selection (or the whole document) and use what it
# prints; they're in the command palette as "Tool: <name>". In the command, {file},
# {dir} and {name} are the document's path, folder and file name, {line} the cursor's
# line. For example:
#
# [[tool]]
# name = "Format JSON (jq)"
# command = ["jq", "."]
# input = "selection-or-document"   # or "selection", "document", "nothing"
# output = "replace"                # or "insert", "new-tab", "message", "nothing"
# key = "ctrl+alt+j"                # optional
# timeout = 30                      # seconds
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
                "large_file_mb" => number(value).map(|n| config.large_file_mb = (n as u64).max(1)),
                "tool" => value.as_array().map(|tools| {
                    for (i, t) in tools.iter().enumerate() {
                        match parse_tool(t) {
                            Ok(tool) => config.tools.push(tool),
                            Err(e) => problems.push(format!("settings: tool {}: {e}", i + 1)),
                        }
                    }
                }),
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

fn parse_tool(value: &toml::Value) -> Result<ExternalTool, String> {
    let t = value.as_table().ok_or("expected a [[tool]] table")?;
    let string = |k: &str| t.get(k).and_then(|v| v.as_str());
    let name = string("name").ok_or("needs a name")?.to_owned();
    let command: Vec<String> = match t.get("command") {
        Some(toml::Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect::<Option<_>>()
            .ok_or("command must be a list of strings")?,
        Some(toml::Value::String(s)) => s.split_whitespace().map(str::to_owned).collect(),
        _ => return Err(format!("{name:?} needs a command")),
    };
    if command.is_empty() {
        return Err(format!("{name:?} has an empty command"));
    }
    let input = match string("input").unwrap_or("selection-or-document") {
        "selection-or-document" => ToolInput::SelectionOrDocument,
        "selection" => ToolInput::Selection,
        "document" => ToolInput::Document,
        "nothing" | "none" => ToolInput::Nothing,
        other => return Err(format!("unknown input {other:?}")),
    };
    let output = match string("output").unwrap_or("replace") {
        "replace" => ToolOutput::Replace,
        "insert" => ToolOutput::Insert,
        "new-tab" => ToolOutput::NewTab,
        "message" => ToolOutput::Message,
        "nothing" | "none" => ToolOutput::Nothing,
        other => return Err(format!("unknown output {other:?}")),
    };
    let key = string("key").map(Shortcut::parse).transpose()?;
    let timeout = t
        .get("timeout")
        .and_then(number)
        .map_or(Duration::from_secs(30), |s| {
            Duration::from_secs_f32(s.clamp(0.1, 3600.0))
        });
    Ok(ExternalTool {
        name,
        command,
        input,
        output,
        key,
        timeout,
    })
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

    #[test]
    fn external_tools() {
        let (config, problems) = Config::parse(
            r#"
[[tool]]
name = "Sort"
command = ["sort", "-u"]

[[tool]]
name = "Word count"
command = "wc -w"
input = "document"
output = "message"
key = "ctrl+alt+w"
timeout = 5

[[tool]]
name = "Broken"
output = "sideways"
command = ["x"]
"#,
        );
        assert_eq!(problems, ["settings: tool 3: unknown output \"sideways\""]);
        assert_eq!(config.tools.len(), 2);
        assert_eq!(config.tools[0].command, ["sort", "-u"]);
        assert_eq!(config.tools[0].output, ToolOutput::Replace);
        assert_eq!(config.tools[1].command, ["wc", "-w"]);
        assert_eq!(config.tools[1].input, ToolInput::Document);
        assert_eq!(config.tools[1].timeout, Duration::from_secs(5));
        assert!(config.tools[1].key.is_some());
    }
}

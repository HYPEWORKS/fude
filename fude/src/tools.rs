//! Running text tools on the document: the built-in ones (`fude-tools`) and external
//! programs from the settings. A built-in tool on a small selection runs right away;
//! bigger inputs and every external program run on a worker thread over a snapshot, and
//! the result applies as one undo step if the document hasn't changed meanwhile.

use std::io::{Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fude_buffer::{Format, LineEnding, Rope, Version};
use fude_editor::Selection;
use fude_platform::Ctx;
use fude_tools::{Options, Output, Tool};

use crate::app::Fude;
use crate::config::{ExternalTool, ToolInput, ToolOutput};
use crate::doc::{Doc, DocId};

/// Built-in tools on less text than this run right away, without a worker.
const SYNC_LIMIT: usize = 256 * 1024;

/// What a tool produced.
#[derive(Debug, PartialEq)]
pub enum ToolResult {
    /// New text for ranges (sorted) of the text the tool started from.
    Replace(Vec<(Range<usize>, Vec<u8>)>),
    /// Text to put in at the cursor.
    Insert(Vec<u8>),
    /// The whole text, in a new format (line endings converted).
    Content(Rope, Format),
    NewTab(Vec<u8>),
    /// A value to show and copy (a hash).
    Copy(String),
    Message(String),
    Error(String),
}

/// A tool running on a worker thread.
pub struct ToolJob {
    doc: DocId,
    version: Version,
    name: String,
    result: mpsc::Receiver<ToolResult>,
}

impl Fude {
    /// Says so if a tool is already running.
    fn tool_busy(&mut self) -> bool {
        let Some(job) = &self.tool_job else {
            return false;
        };
        let name = job.name.clone();
        self.say(format!("{name} is still running"), false);
        true
    }

    /// The non-empty selections, or (with nothing selected) the whole text.
    fn tool_ranges(&self) -> Vec<Range<usize>> {
        let editor = &self.doc().view.editor;
        let ranges: Vec<Range<usize>> = editor
            .selections()
            .iter()
            .filter(|s| !s.is_empty())
            .map(Selection::range)
            .collect();
        if ranges.is_empty() {
            std::iter::once(0..editor.text().len()).collect()
        } else {
            ranges
        }
    }

    /// Runs a built-in tool on the selections, or the whole document.
    pub fn run_tool(&mut self, ctx: &mut Ctx<'_>, tool: Tool) {
        let active = self.active;
        if self.tool_busy() || self.still_loading(active) {
            return;
        }
        let mut ranges = self.tool_ranges();
        if !tool.changes_text() {
            // A hash is of one thing: the primary selection, or the document.
            let primary = *self.doc().view.editor.selections().primary();
            ranges = vec![if primary.is_empty() {
                ranges[0].clone()
            } else {
                primary.range()
            }];
        }
        let editor = &self.doc().view.editor;
        let options = Options {
            tab_width: editor.settings.tab_width,
            indent: editor.indent_unit(),
            line_ending: editor.format().line_ending.as_bytes().to_vec(),
        };
        let text = editor.text().clone();
        let (doc, version) = (self.doc().id, editor.buffer().version());
        let size: usize = ranges.iter().map(|r| r.len()).sum();
        let job = move || builtin(tool, &text, &ranges, &options);
        if size <= SYNC_LIMIT {
            let result = job();
            self.finish_tool(ctx, doc, version, tool.title(), result);
        } else {
            self.spawn_tool(doc, version, tool.title().to_owned(), job);
        }
    }

    /// Runs external tool `index` from the settings.
    pub fn run_external(&mut self, index: usize) {
        let active = self.active;
        let Some(tool) = self.config.tools.get(index).cloned() else {
            return;
        };
        if self.tool_busy() || self.still_loading(active) {
            return;
        }
        let editor = &self.doc().view.editor;
        let text = editor.text().clone();
        let primary = *editor.selections().primary();
        let inputs: Vec<Option<Range<usize>>> = match tool.input {
            ToolInput::Nothing => vec![None],
            ToolInput::Document => vec![Some(0..text.len())],
            ToolInput::SelectionOrDocument => self.tool_ranges().into_iter().map(Some).collect(),
            ToolInput::Selection => {
                let ranges: Vec<_> = editor
                    .selections()
                    .iter()
                    .filter(|s| !s.is_empty())
                    .map(|s| Some(s.range()))
                    .collect();
                if ranges.is_empty() {
                    self.say(
                        format!("{} works on a selection: select some text first", tool.name),
                        false,
                    );
                    return;
                }
                ranges
            }
        };
        let path = self.doc().path.clone();
        let line = text.byte_to_line(primary.head) + 1;
        let argv = expand(&tool.command, path.as_deref(), line);
        let cwd = path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        let (doc, version) = (self.doc().id, editor.buffer().version());
        let name = tool.name.clone();
        self.spawn_tool(doc, version, name, move || {
            external(&tool, &argv, &cwd, &text, &inputs, primary.range())
        });
    }

    /// Converts every line break to `le` (one undo step), on a worker for a big document.
    pub fn convert_line_endings(&mut self, ctx: &mut Ctx<'_>, le: LineEnding) {
        let active = self.active;
        if self.tool_busy() || self.still_loading(active) {
            return;
        }
        let editor = &self.doc().view.editor;
        let text = editor.text().clone();
        let format = Format {
            line_ending: le,
            ..*editor.format()
        };
        let (doc, version) = (self.doc().id, editor.buffer().version());
        let job = move || ToolResult::Content(fude_buffer::convert_line_endings(&text, le), format);
        if self.doc().view.editor.text().len() <= SYNC_LIMIT {
            let result = job();
            self.finish_tool(ctx, doc, version, "Converting line endings", result);
        } else {
            self.spawn_tool(doc, version, "Converting line endings".into(), job);
        }
    }

    fn spawn_tool(
        &mut self,
        doc: DocId,
        version: Version,
        name: String,
        job: impl FnOnce() -> ToolResult + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let waker = self.waker.clone();
        let spawned = std::thread::Builder::new()
            .name("fude-tool".into())
            .spawn(move || {
                let _ = tx.send(job());
                if let Some(w) = waker {
                    w.wake();
                }
            });
        if let Err(e) = spawned {
            self.say(format!("Couldn't start {name}: {e}"), true);
            return;
        }
        self.say(format!("Running {name}…"), false);
        self.tool_job = Some(ToolJob {
            doc,
            version,
            name,
            result: rx,
        });
    }

    /// Takes in a finished tool's result. Returns whether to redraw.
    pub fn poll_tools(&mut self, ctx: &mut Ctx<'_>) -> bool {
        let Some(job) = &self.tool_job else {
            return false;
        };
        let result = match job.result.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                ToolResult::Error(format!("{} stopped unexpectedly", job.name))
            }
        };
        let job = self.tool_job.take().expect("a job");
        self.finish_tool(ctx, job.doc, job.version, &job.name, result);
        true
    }

    fn finish_tool(
        &mut self,
        ctx: &mut Ctx<'_>,
        doc: DocId,
        version: Version,
        name: &str,
        result: ToolResult,
    ) {
        let Some(i) = self.index_of(doc) else {
            return;
        };
        let now = Instant::now();
        let changes = matches!(
            result,
            ToolResult::Replace(_) | ToolResult::Insert(_) | ToolResult::Content(..)
        );
        if changes && self.docs[i].view.editor.buffer().version() != version {
            self.say(
                format!("The text changed while {name} ran; nothing was replaced"),
                true,
            );
            return;
        }
        match result {
            ToolResult::Replace(edits) if edits.is_empty() => {
                self.say(format!("{name}: nothing to change"), false);
            }
            ToolResult::Replace(edits) => {
                let view = &mut self.docs[i].view;
                view.editor.replace_ranges(edits);
                view.changed(now);
                self.message = None;
            }
            ToolResult::Insert(text) => {
                let view = &mut self.docs[i].view;
                let range = view.editor.selections().primary().range();
                view.editor.replace_ranges(vec![(range, text)]);
                view.changed(now);
                self.message = None;
            }
            ToolResult::Content(text, format) => {
                let view = &mut self.docs[i].view;
                view.editor.replace_content(text, format);
                view.changed(now);
                let le = format.line_ending.name();
                self.say(format!("Line endings are now {le}"), false);
            }
            ToolResult::NewTab(text) => {
                let id = self.new_id();
                let format = Format {
                    line_ending: LineEnding::PLATFORM,
                    ..Format::default()
                };
                self.add_doc(Doc::new(id, Rope::from(&text[..]), format, None));
                self.message = None;
            }
            ToolResult::Copy(value) => {
                ctx.set_clipboard_text(value.clone());
                self.say(format!("{name}: {value} (copied)"), false);
            }
            ToolResult::Message(text) => self.say(text, false),
            ToolResult::Error(text) => self.say(text, true),
        }
    }
}

/// A built-in tool over each range.
fn builtin(tool: Tool, text: &Rope, ranges: &[Range<usize>], options: &Options) -> ToolResult {
    let mut edits = Vec::new();
    for r in ranges {
        let mut bytes = Vec::new();
        text.copy_range(r.clone(), &mut bytes);
        match tool.run(&bytes, options) {
            Ok(Output::Text(out)) if out != bytes => edits.push((r.clone(), out)),
            Ok(Output::Text(_)) => {}
            Ok(Output::Value(v)) => return ToolResult::Copy(v),
            Err(e) => return ToolResult::Error(e),
        }
    }
    ToolResult::Replace(edits)
}

/// The command with `{file}`, `{dir}`, `{name}` and `{line}` filled in.
fn expand(command: &[String], path: Option<&Path>, line: usize) -> Vec<String> {
    let file = path.map(|p| p.display().to_string()).unwrap_or_default();
    let dir = path
        .and_then(Path::parent)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let name = path
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    command
        .iter()
        .map(|arg| {
            arg.replace("{file}", &file)
                .replace("{dir}", &dir)
                .replace("{name}", &name)
                .replace("{line}", &line.to_string())
        })
        .collect()
}

/// Runs an external tool once per input and turns what it printed into a result.
fn external(
    tool: &ExternalTool,
    argv: &[String],
    cwd: &Path,
    text: &Rope,
    inputs: &[Option<Range<usize>>],
    primary: Range<usize>,
) -> ToolResult {
    let mut outputs = Vec::new();
    for input in inputs {
        let bytes = input.as_ref().map(|r| {
            let mut b = Vec::new();
            text.copy_range(r.clone(), &mut b);
            b
        });
        match run_process(argv, cwd, bytes.as_deref(), tool.timeout) {
            Ok(mut out) => {
                // Programs end their output with a line break; one the input didn't
                // have would be added at every use.
                let had = bytes.as_deref().is_none_or(|b| b.ends_with(b"\n"));
                if !had {
                    if out.ends_with(b"\r\n") {
                        out.truncate(out.len() - 2);
                    } else if out.ends_with(b"\n") {
                        out.truncate(out.len() - 1);
                    }
                }
                outputs.push((input.clone().unwrap_or_else(|| primary.clone()), out));
            }
            Err(e) => return ToolResult::Error(e),
        }
    }
    let first = || outputs.first().map(|o| o.1.clone()).unwrap_or_default();
    match tool.output {
        ToolOutput::Replace => ToolResult::Replace(
            outputs
                .into_iter()
                .filter(|(r, out)| {
                    let mut b = Vec::new();
                    text.copy_range(r.clone(), &mut b);
                    b != *out
                })
                .collect(),
        ),
        ToolOutput::Insert => ToolResult::Insert(first()),
        ToolOutput::NewTab => ToolResult::NewTab(outputs.into_iter().flat_map(|o| o.1).collect()),
        ToolOutput::Message => {
            let out = first();
            let line = String::from_utf8_lossy(&out)
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map_or_else(
                    || format!("{}: done", tool.name),
                    |l| format!("{}: {l}", tool.name),
                );
            ToolResult::Message(line)
        }
        ToolOutput::Nothing => ToolResult::Message(format!("{}: done", tool.name)),
    }
}

/// Runs a program with `input` on its standard input (none if `None`) and returns what it
/// printed, or why it failed: it couldn't start, exited with an error (the first line it
/// printed to standard error says why), or ran past `timeout` and was stopped.
pub fn run_process(
    argv: &[String],
    cwd: &Path,
    input: Option<&[u8]>,
    timeout: Duration,
) -> Result<Vec<u8>, String> {
    let program = &argv[0];
    let mut command = Command::new(program);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        // No console window flashing up for each run.
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("Couldn't run {program}: not found"),
        _ => format!("Couldn't run {program}: {e}"),
    })?;
    // Standard input goes in on a thread of its own, so a program that prints before it
    // has read everything can't block on us while we block on it.
    let writer = match (input, child.stdin.take()) {
        (Some(bytes), Some(mut stdin)) => {
            let bytes = bytes.to_vec();
            Some(std::thread::spawn(move || {
                let _ = stdin.write_all(&bytes);
            }))
        }
        _ => None,
    };
    let reader = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut out);
            }
            out
        })
    };
    let stdout = reader(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = reader(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{program} ran longer than {} s and was stopped",
                    timeout.as_secs_f32()
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => return Err(format!("{program}: {e}")),
        }
    };
    if let Some(w) = writer {
        let _ = w.join();
    }
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    if !status.success() {
        let err = String::from_utf8_lossy(&err);
        let why = err.lines().map(str::trim).find(|l| !l.is_empty());
        return Err(match why {
            Some(line) => format!("{program}: {line}"),
            None => format!("{program} failed ({status})"),
        });
    }
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    #[test]
    fn builtin_tools_on_ranges() {
        let text = Rope::from("b\na\nc\n");
        let options = Options::default();
        assert_eq!(
            builtin(Tool::SortLines, &text, &[0..6], &options),
            ToolResult::Replace(vec![(0..6, b"a\nb\nc\n".to_vec())])
        );
        assert_eq!(
            builtin(Tool::SortLines, &Rope::from("a\nb"), &[0..3], &options),
            ToolResult::Replace(vec![])
        );
        assert!(
            matches!(builtin(Tool::Sha256, &text, &[0..6], &options), ToolResult::Copy(h) if h.len() == 64)
        );
        assert!(matches!(
            builtin(Tool::FormatJson, &text, &[0..6], &options),
            ToolResult::Error(_)
        ));
    }

    #[test]
    fn placeholders() {
        let argv = expand(
            &[
                "fmt".into(),
                "{file}".into(),
                "--line={line}".into(),
                "{name}@{dir}".into(),
            ],
            Some(Path::new("/tmp/x/a.rs")),
            7,
        );
        assert_eq!(argv, ["fmt", "/tmp/x/a.rs", "--line=7", "a.rs@/tmp/x"]);
    }

    #[cfg(unix)]
    #[test]
    fn external_programs() {
        let dir = std::env::temp_dir();
        let run = |argv: &[&str], input: Option<&[u8]>| {
            let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
            run_process(&argv, &dir, input, Duration::from_secs(5))
        };
        assert_eq!(run(&["sort"], Some(b"b\na\n")).unwrap(), b"a\nb\n");
        // A big input and output can't deadlock.
        let big = "line\n".repeat(400_000);
        assert_eq!(
            run(&["cat"], Some(big.as_bytes())).unwrap().len(),
            big.len()
        );
        assert!(
            run(&["sh", "-c", "echo oops >&2; exit 3"], None)
                .unwrap_err()
                .contains("oops")
        );
        assert!(
            run(&["no-such-program-fude"], None)
                .unwrap_err()
                .contains("not found")
        );
        let slow: Vec<String> = ["sleep", "5"].iter().map(|s| s.to_string()).collect();
        let err = run_process(&slow, &dir, None, Duration::from_millis(200)).unwrap_err();
        assert!(err.contains("stopped"), "{err}");

        // Replacing a selection without a line break doesn't add the program's.
        let tool = ExternalTool {
            name: "Upper".into(),
            command: vec!["tr".into(), "a-z".into(), "A-Z".into()],
            input: ToolInput::SelectionOrDocument,
            output: ToolOutput::Replace,
            key: None,
            timeout: Duration::from_secs(5),
        };
        let text = Rope::from("abc def");
        let argv = tool.command.clone();
        assert_eq!(
            external(&tool, &argv, &dir, &text, &[Some(4..7)], 0..0),
            ToolResult::Replace(vec![(4..7, b"DEF".to_vec())])
        );
        let echo = ExternalTool {
            command: vec![
                "sh".into(),
                "-c".into(),
                "printf 'hello\\n\\nworld\\n'".into(),
            ],
            input: ToolInput::Nothing,
            output: ToolOutput::Message,
            ..tool
        };
        let argv = echo.command.clone();
        assert_eq!(
            external(&echo, &argv, &dir, &text, &[None], 0..0),
            ToolResult::Message("Upper: hello".into())
        );
    }
}

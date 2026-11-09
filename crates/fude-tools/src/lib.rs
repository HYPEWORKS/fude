//! Built-in text tools (ARCHITECTURE.md §14): pure functions from text to text, so they
//! can run on a worker over a snapshot and be tested without an editor.
//!
//! Text is bytes: the document's raw UTF-8, which may hold invalid bytes. Tools that need
//! characters (case changes, formatters) leave invalid bytes as they are or refuse with
//! a message saying why.

mod codec;
mod hash;
mod json;
mod lines;
mod xml;

/// A built-in tool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    SortLines,
    SortLinesDescending,
    SortLinesIgnoringCase,
    SortLinesNaturally,
    RemoveDuplicateLines,
    ReverseLines,
    TrimTrailingWhitespace,
    IndentWithSpaces,
    IndentWithTabs,
    UpperCase,
    LowerCase,
    TitleCase,
    Base64Encode,
    Base64Decode,
    UrlEncode,
    UrlDecode,
    HtmlEncode,
    HtmlDecode,
    FormatJson,
    MinifyJson,
    FormatXml,
    Sha256,
    Sha1,
    Md5,
}

/// What the document's settings say about the text a tool works on.
#[derive(Clone, Debug)]
pub struct Options {
    pub tab_width: usize,
    /// One level of indentation, for formatters.
    pub indent: Vec<u8>,
    /// The document's line break, for formatters.
    pub line_ending: Vec<u8>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            tab_width: 4,
            indent: b"    ".to_vec(),
            line_ending: b"\n".to_vec(),
        }
    }
}

/// What a tool made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// New text for what it was given.
    Text(Vec<u8>),
    /// A value about the text (a hash), to show or copy; the text stays as it is.
    Value(String),
}

impl Tool {
    pub const ALL: [Tool; 24] = [
        Tool::SortLines,
        Tool::SortLinesDescending,
        Tool::SortLinesIgnoringCase,
        Tool::SortLinesNaturally,
        Tool::RemoveDuplicateLines,
        Tool::ReverseLines,
        Tool::TrimTrailingWhitespace,
        Tool::IndentWithSpaces,
        Tool::IndentWithTabs,
        Tool::UpperCase,
        Tool::LowerCase,
        Tool::TitleCase,
        Tool::Base64Encode,
        Tool::Base64Decode,
        Tool::UrlEncode,
        Tool::UrlDecode,
        Tool::HtmlEncode,
        Tool::HtmlDecode,
        Tool::FormatJson,
        Tool::MinifyJson,
        Tool::FormatXml,
        Tool::Sha256,
        Tool::Sha1,
        Tool::Md5,
    ];

    /// The title in the command palette.
    pub fn title(self) -> &'static str {
        match self {
            Tool::SortLines => "Sort Lines (A to Z)",
            Tool::SortLinesDescending => "Sort Lines (Z to A)",
            Tool::SortLinesIgnoringCase => "Sort Lines, Ignoring Case",
            Tool::SortLinesNaturally => "Sort Lines by Number (2 before 10)",
            Tool::RemoveDuplicateLines => "Remove Duplicate Lines",
            Tool::ReverseLines => "Reverse Lines",
            Tool::TrimTrailingWhitespace => "Trim Trailing Whitespace",
            Tool::IndentWithSpaces => "Indentation: Tabs to Spaces",
            Tool::IndentWithTabs => "Indentation: Spaces to Tabs",
            Tool::UpperCase => "UPPERCASE",
            Tool::LowerCase => "lowercase",
            Tool::TitleCase => "Title Case",
            Tool::Base64Encode => "Base64 Encode",
            Tool::Base64Decode => "Base64 Decode",
            Tool::UrlEncode => "URL Encode",
            Tool::UrlDecode => "URL Decode",
            Tool::HtmlEncode => "HTML Encode (Escape)",
            Tool::HtmlDecode => "HTML Decode (Unescape)",
            Tool::FormatJson => "Format JSON",
            Tool::MinifyJson => "Minify JSON",
            Tool::FormatXml => "Format XML",
            Tool::Sha256 => "SHA-256 (Copy)",
            Tool::Sha1 => "SHA-1 (Copy)",
            Tool::Md5 => "MD5 (Copy)",
        }
    }

    /// A stable name, for keymaps.
    pub fn name(self) -> &'static str {
        match self {
            Tool::SortLines => "tools.sort_lines",
            Tool::SortLinesDescending => "tools.sort_lines_descending",
            Tool::SortLinesIgnoringCase => "tools.sort_lines_ignoring_case",
            Tool::SortLinesNaturally => "tools.sort_lines_naturally",
            Tool::RemoveDuplicateLines => "tools.remove_duplicate_lines",
            Tool::ReverseLines => "tools.reverse_lines",
            Tool::TrimTrailingWhitespace => "tools.trim_trailing_whitespace",
            Tool::IndentWithSpaces => "tools.indent_with_spaces",
            Tool::IndentWithTabs => "tools.indent_with_tabs",
            Tool::UpperCase => "tools.upper_case",
            Tool::LowerCase => "tools.lower_case",
            Tool::TitleCase => "tools.title_case",
            Tool::Base64Encode => "tools.base64_encode",
            Tool::Base64Decode => "tools.base64_decode",
            Tool::UrlEncode => "tools.url_encode",
            Tool::UrlDecode => "tools.url_decode",
            Tool::HtmlEncode => "tools.html_encode",
            Tool::HtmlDecode => "tools.html_decode",
            Tool::FormatJson => "tools.format_json",
            Tool::MinifyJson => "tools.minify_json",
            Tool::FormatXml => "tools.format_xml",
            Tool::Sha256 => "tools.sha256",
            Tool::Sha1 => "tools.sha1",
            Tool::Md5 => "tools.md5",
        }
    }

    /// Runs the tool on `text`.
    pub fn run(self, text: &[u8], options: &Options) -> Result<Output, String> {
        let t = |v: Vec<u8>| Ok(Output::Text(v));
        match self {
            Tool::SortLines => t(lines::sort(text, lines::Order::Bytes, false)),
            Tool::SortLinesDescending => t(lines::sort(text, lines::Order::Bytes, true)),
            Tool::SortLinesIgnoringCase => t(lines::sort(text, lines::Order::IgnoringCase, false)),
            Tool::SortLinesNaturally => t(lines::sort(text, lines::Order::Natural, false)),
            Tool::RemoveDuplicateLines => t(lines::unique(text)),
            Tool::ReverseLines => t(lines::reverse(text)),
            Tool::TrimTrailingWhitespace => t(lines::trim_trailing(text)),
            Tool::IndentWithSpaces => t(lines::indent_with_spaces(text, options.tab_width)),
            Tool::IndentWithTabs => t(lines::indent_with_tabs(text, options.tab_width)),
            Tool::UpperCase => t(codec::map_str(text, str::to_uppercase)),
            Tool::LowerCase => t(codec::map_str(text, str::to_lowercase)),
            Tool::TitleCase => t(codec::map_str(text, codec::title_case)),
            Tool::Base64Encode => t(codec::base64_encode(text)),
            Tool::Base64Decode => codec::base64_decode(text).map(Output::Text),
            Tool::UrlEncode => t(codec::url_encode(text)),
            Tool::UrlDecode => t(codec::url_decode(text)),
            Tool::HtmlEncode => t(codec::html_encode(text)),
            Tool::HtmlDecode => t(codec::html_decode(text)),
            Tool::FormatJson => {
                json::format(text, Some((&options.indent, &options.line_ending))).map(Output::Text)
            }
            Tool::MinifyJson => json::format(text, None).map(Output::Text),
            Tool::FormatXml => {
                xml::format(text, &options.indent, &options.line_ending).map(Output::Text)
            }
            Tool::Sha256 => Ok(Output::Value(hex(&hash::sha256(text)))),
            Tool::Sha1 => Ok(Output::Value(hex(&hash::sha1(text)))),
            Tool::Md5 => Ok(Output::Value(hex(&hash::md5(text)))),
        }
    }

    /// Hashes describe the text rather than change it.
    pub fn changes_text(self) -> bool {
        !matches!(self, Tool::Sha256 | Tool::Sha1 | Tool::Md5)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Line and column (1-based) of a byte offset, for error messages.
pub(crate) fn line_col(text: &[u8], at: usize) -> (usize, usize) {
    let before = &text[..at.min(text.len())];
    let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
    let col = before.len()
        - before
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1)
        + 1;
    (line, col)
}

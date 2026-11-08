//! Demo text for the modes without a window when no file is given.

pub const WELCOME: &str = "\
fude 筆: Fast, Uncomplicated Document Editor
===========================================

Type anywhere. This build edits one document at a time.

  Move             arrows, Home / End, PageUp / PageDown, Ctrl+arrows by word
  Select           Shift + any move, mouse drag, double-click word, triple-click line
  Edit             Ctrl+Z / Ctrl+Y undo and redo, Ctrl+X / C / V, Ctrl+A
  Files            Ctrl+N new, Ctrl+O open, Ctrl+S save, Ctrl+Shift+S save as
  Zoom             Ctrl+= / Ctrl+- / Ctrl+0   (Cmd instead of Ctrl on macOS)

Unicode:      café naïve Ærøskøbing  “quotes” • bullets → arrows ≠ ≤ ≥ ∞
CJK:          筆 墨 朱  日本語のテキスト  中文  한국어
Emoji:        😀 👍🏽 🇯🇵
Control:      a\u{1b}b   (escape character, shown as a chip)

fn main() {
\tlet greeting = \"Hello, fude!\";
\tfor (i, word) in greeting.split(' ').enumerate() {
\t\tprintln!(\"{i}: {word}\");   // tabs: 4 columns
\t}
}
";

/// The welcome text plus numbered filler lines, so there is something to scroll.
pub fn text() -> String {
    let mut s = WELCOME.to_owned();
    for i in 1..=500 {
        s.push_str(&format!(
            "{i:>4}  The quick brown fox jumps over the lazy dog. 0123456789 ()[]{{}}<>!=->\n"
        ));
    }
    s
}

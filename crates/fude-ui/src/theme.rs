//! Colors by role. Every piece of UI draws with these, never with literal colors.
//!
//! Text meets WCAG AA contrast (4.5:1) on every background it's drawn on, selections
//! aside (tested below).

use fude_render::Color;
use fude_syntax::Style;

/// Code colors by style.
#[derive(Debug)]
pub struct SyntaxColors {
    pub comment: Color,
    pub string: Color,
    pub escape: Color,
    pub number: Color,
    pub constant: Color,
    pub keyword: Color,
    pub storage: Color,
    pub operator: Color,
    pub function: Color,
    pub type_name: Color,
    pub tag: Color,
    pub attribute: Color,
    pub punctuation: Color,
    pub heading: Color,
    pub link: Color,
}

#[derive(Debug)]
pub struct Theme {
    pub editor_bg: Color,
    pub text: Color,
    pub line_highlight: Color,
    pub selection: Color,
    /// Selection while the window isn't focused.
    pub selection_inactive: Color,
    /// Find matches other than the current one.
    /// Outline around a bracket next to the cursor and its partner.
    pub bracket_match: Color,
    pub find_match: Color,
    pub caret: Color,
    pub gutter_text: Color,
    pub gutter_text_active: Color,
    /// Tab strip and status bar.
    pub chrome_bg: Color,
    pub chrome_text: Color,
    pub chrome_text_muted: Color,
    pub border: Color,
    /// The one strong color: active tab marker, primary buttons, focus.
    pub accent: Color,
    pub accent_text: Color,
    /// Control character chips.
    pub chip_bg: Color,
    pub chip_text: Color,
    /// Chips for bytes that aren't valid UTF-8.
    pub invalid_bg: Color,
    pub invalid_text: Color,
    /// IME composition underline.
    pub preedit: Color,
    pub scrollbar: Color,
    pub scrollbar_hover: Color,
    /// Dims the window behind a dialog.
    pub scrim: Color,
    pub dialog_bg: Color,
    pub button_bg: Color,
    pub button_text: Color,
    pub error: Color,
    pub syntax: SyntaxColors,
}

impl Theme {
    /// The color for highlighted code.
    pub fn style(&self, style: Style) -> Color {
        let s = &self.syntax;
        match style {
            Style::Plain | Style::Emphasis => self.text,
            Style::Comment => s.comment,
            Style::String => s.string,
            Style::Escape => s.escape,
            Style::Number => s.number,
            Style::Constant => s.constant,
            Style::Keyword => s.keyword,
            Style::Storage => s.storage,
            Style::Operator => s.operator,
            Style::Function => s.function,
            Style::Type => s.type_name,
            Style::Tag => s.tag,
            Style::Attribute => s.attribute,
            Style::Punctuation => s.punctuation,
            Style::Heading => s.heading,
            Style::Link => s.link,
            Style::Invalid => self.error,
        }
    }
}

/// Sumi (墨): warm ink black with a vermilion (朱) accent.
pub static SUMI: Theme = Theme {
    editor_bg: Color::rgb(0x1E1D1B),
    text: Color::rgb(0xE6E1D6),
    line_highlight: Color::rgb(0x262522),
    selection: Color::rgb(0x3D3530),
    selection_inactive: Color::rgb(0x312D29),
    find_match: Color::rgb(0x4D4128),
    bracket_match: Color::rgba(0xE6E1D6, 0x60),
    caret: Color::rgb(0xE2553F),
    gutter_text: Color::rgb(0x89857C),
    gutter_text_active: Color::rgb(0xA8A296),
    chrome_bg: Color::rgb(0x171614),
    chrome_text: Color::rgb(0xD6D1C6),
    chrome_text_muted: Color::rgb(0xA49F94),
    border: Color::rgb(0x2C2A27),
    accent: Color::rgb(0xE2553F),
    accent_text: Color::rgb(0x1A1918),
    chip_bg: Color::rgb(0x36332F),
    chip_text: Color::rgb(0xB5AFA3),
    invalid_bg: Color::rgb(0x5A2A22),
    invalid_text: Color::rgb(0xF2B8AC),
    preedit: Color::rgb(0xE6E1D6),
    scrollbar: Color::rgba(0xE6E1D6, 0x24),
    scrollbar_hover: Color::rgba(0xE6E1D6, 0x44),
    scrim: Color::rgba(0x000000, 0x70),
    dialog_bg: Color::rgb(0x252421),
    button_bg: Color::rgb(0x34322E),
    button_text: Color::rgb(0xE6E1D6),
    error: Color::rgb(0xF07A66),
    syntax: SyntaxColors {
        comment: Color::rgb(0x938C80),
        string: Color::rgb(0xA9BC8F),
        escape: Color::rgb(0xE6A26A),
        number: Color::rgb(0xDDA35E),
        constant: Color::rgb(0xDDA35E),
        keyword: Color::rgb(0xE07A62),
        storage: Color::rgb(0xE07A62),
        operator: Color::rgb(0xC9C2B5),
        function: Color::rgb(0xE5C67F),
        type_name: Color::rgb(0x94B9C4),
        tag: Color::rgb(0xE07A62),
        attribute: Color::rgb(0xE5C67F),
        punctuation: Color::rgb(0xA8A296),
        heading: Color::rgb(0xE26651),
        link: Color::rgb(0x94B9C4),
    },
};

/// Washi (和紙): warm paper white with the same vermilion accent.
pub static WASHI: Theme = Theme {
    editor_bg: Color::rgb(0xFAF7F0),
    text: Color::rgb(0x2B2A27),
    line_highlight: Color::rgb(0xF2EDE3),
    selection: Color::rgb(0xEED8CD),
    selection_inactive: Color::rgb(0xE8E2D6),
    find_match: Color::rgb(0xF4E2B2),
    bracket_match: Color::rgba(0x2B2A27, 0x60),
    caret: Color::rgb(0xC8402A),
    gutter_text: Color::rgb(0x757067),
    gutter_text_active: Color::rgb(0x5E584E),
    chrome_bg: Color::rgb(0xF0EBE1),
    chrome_text: Color::rgb(0x3A3833),
    chrome_text_muted: Color::rgb(0x656056),
    border: Color::rgb(0xDCD5C8),
    accent: Color::rgb(0xC8402A),
    accent_text: Color::rgb(0xFFFFFF),
    chip_bg: Color::rgb(0xE6E0D4),
    chip_text: Color::rgb(0x5E584E),
    invalid_bg: Color::rgb(0xF5D5CC),
    invalid_text: Color::rgb(0x9A2E1E),
    preedit: Color::rgb(0x2B2A27),
    scrollbar: Color::rgba(0x2B2A27, 0x22),
    scrollbar_hover: Color::rgba(0x2B2A27, 0x44),
    scrim: Color::rgba(0x000000, 0x40),
    dialog_bg: Color::rgb(0xFBF9F4),
    button_bg: Color::rgb(0xE8E2D6),
    button_text: Color::rgb(0x2B2A27),
    error: Color::rgb(0xB0301F),
    syntax: SyntaxColors {
        comment: Color::rgb(0x6F6A60),
        string: Color::rgb(0x4E6E34),
        escape: Color::rgb(0x9A5410),
        number: Color::rgb(0x9A5410),
        constant: Color::rgb(0x9A5410),
        keyword: Color::rgb(0xB23A25),
        storage: Color::rgb(0xB23A25),
        operator: Color::rgb(0x4A463F),
        function: Color::rgb(0x775610),
        type_name: Color::rgb(0x2C6273),
        tag: Color::rgb(0xB23A25),
        attribute: Color::rgb(0x775610),
        punctuation: Color::rgb(0x6F695E),
        heading: Color::rgb(0xBE3F2A),
        link: Color::rgb(0x2C6273),
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG 2's contrast ratio of two opaque colors.
    fn contrast(a: Color, b: Color) -> f64 {
        let luminance = |c: Color| {
            let channel = |shift: u32| {
                let v = f64::from((c.0 >> shift) & 0xFF) / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Every text color against every background it's drawn on. Code on a selection or
    /// a find match is left out, as is disabled text (WCAG exempts it).
    #[test]
    fn text_meets_wcag_aa_contrast() {
        for (name, t) in [("Sumi", &SUMI), ("Washi", &WASHI)] {
            let s = &t.syntax;
            let editor = [t.editor_bg, t.line_highlight];
            let chrome = [
                t.chrome_bg,
                t.dialog_bg,
                t.editor_bg,
                t.line_highlight,
                t.selection,
            ];
            let code = [
                s.comment,
                s.string,
                s.escape,
                s.number,
                s.constant,
                s.keyword,
                s.storage,
                s.operator,
                s.function,
                s.type_name,
                s.tag,
                s.attribute,
                s.punctuation,
                s.heading,
                s.link,
                t.error,
            ];
            let mut checks: Vec<(&str, Color, Vec<Color>)> = vec![
                (
                    "text",
                    t.text,
                    vec![t.editor_bg, t.line_highlight, t.selection, t.find_match],
                ),
                ("line numbers", t.gutter_text, vec![t.editor_bg]),
                ("current line number", t.gutter_text_active, editor.to_vec()),
                ("chrome text", t.chrome_text, chrome.to_vec()),
                ("muted chrome text", t.chrome_text_muted, chrome.to_vec()),
                ("error", t.error, vec![t.chrome_bg, t.dialog_bg]),
                ("button text", t.button_text, vec![t.button_bg]),
                ("primary button text", t.accent_text, vec![t.accent]),
                ("chip", t.chip_text, vec![t.chip_bg]),
                ("invalid byte chip", t.invalid_text, vec![t.invalid_bg]),
            ];
            checks.extend(code.iter().map(|&c| ("code", c, editor.to_vec())));
            for (what, fg, backgrounds) in checks {
                for bg in backgrounds {
                    let ratio = contrast(fg, bg);
                    assert!(
                        ratio >= 4.5,
                        "{name}: {what} {:06X} on {:06X} is {ratio:.2}:1",
                        fg.0 & 0xFF_FFFF,
                        bg.0 & 0xFF_FFFF
                    );
                }
            }
        }
    }
}

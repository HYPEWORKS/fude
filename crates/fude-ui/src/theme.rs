//! Colors by role. Every piece of UI draws with these, never with literal colors.

use fude_render::Color;

#[derive(Debug)]
pub struct Theme {
    pub editor_bg: Color,
    pub text: Color,
    pub line_highlight: Color,
    pub selection: Color,
    /// Selection while the window isn't focused.
    pub selection_inactive: Color,
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
}

/// Sumi (墨): warm ink black with a vermilion (朱) accent.
pub static SUMI: Theme = Theme {
    editor_bg: Color::rgb(0x1E1D1B),
    text: Color::rgb(0xE6E1D6),
    line_highlight: Color::rgb(0x262522),
    selection: Color::rgb(0x3D3530),
    selection_inactive: Color::rgb(0x312D29),
    caret: Color::rgb(0xE2553F),
    gutter_text: Color::rgb(0x5E5A53),
    gutter_text_active: Color::rgb(0xA8A296),
    chrome_bg: Color::rgb(0x171614),
    chrome_text: Color::rgb(0xD6D1C6),
    chrome_text_muted: Color::rgb(0x7D786F),
    border: Color::rgb(0x2C2A27),
    accent: Color::rgb(0xE2553F),
    accent_text: Color::rgb(0x1E1D1B),
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
};

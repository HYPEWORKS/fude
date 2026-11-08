use fude_render::Color;

/// Color roles for the UI. Every widget draws from these; nothing hard-codes colors.
pub struct Theme {
    pub editor_bg: Color,
    pub text: Color,
    pub line_highlight: Color,
    pub gutter_text: Color,
    pub gutter_text_active: Color,
    pub chrome_bg: Color,
    pub chrome_text: Color,
    pub chrome_text_muted: Color,
    pub border: Color,
    pub accent: Color,
}

/// 墨 Sumi: warm ink-black with washi-paper text and a 朱 (vermilion) accent, the red
/// ink used for corrections in Japanese calligraphy.
pub const SUMI: Theme = Theme {
    editor_bg: Color::rgb(0x1E1D1B),
    text: Color::rgb(0xE6E1D6),
    line_highlight: Color::rgb(0x262522),
    gutter_text: Color::rgb(0x5E5A53),
    gutter_text_active: Color::rgb(0xA8A296),
    chrome_bg: Color::rgb(0x171614),
    chrome_text: Color::rgb(0xD6D1C6),
    chrome_text_muted: Color::rgb(0x7D786F),
    border: Color::rgb(0x2C2A27),
    accent: Color::rgb(0xE2553F),
};

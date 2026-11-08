//! Chips: text drawn as a small labeled box instead of glyphs.
//!
//! Control characters and bytes that aren't valid UTF-8 have no glyphs, so the view shows
//! them as chips (`NUL`, `ESC`, `FF`). It encodes them into the text it lays out as code
//! points from the end of Unicode's last private use plane, which real text practically
//! never uses; layout gives each one a box-sized advance, and the view draws the box.

/// A chip, by what it stands for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chip {
    /// A C0 or C1 control character (other than tab and line breaks), or DEL.
    Control(u8),
    /// A byte that isn't part of valid UTF-8.
    Byte(u8),
}

const CONTROL_BASE: u32 = 0x10_FE00;
const BYTE_BASE: u32 = 0x10_FF00;

impl Chip {
    /// The placeholder character to put in laid-out text.
    pub fn encode(self) -> char {
        let code = match self {
            Chip::Control(c) => CONTROL_BASE + u32::from(c),
            Chip::Byte(b) => BYTE_BASE + u32::from(b),
        };
        char::from_u32(code).expect("private use plane 16")
    }

    pub fn decode(c: char) -> Option<Chip> {
        let code = c as u32;
        let byte = (code & 0xFF) as u8;
        match code & !0xFF {
            CONTROL_BASE => Some(Chip::Control(byte)),
            BYTE_BASE => Some(Chip::Byte(byte)),
            _ => None,
        }
    }

    /// The chip for a character that should be shown as one, if any.
    pub fn for_char(c: char) -> Option<Chip> {
        match c {
            '\t' | '\n' | '\r' => None,
            '\0'..='\u{1F}' | '\u{7F}'..='\u{9F}' => Some(Chip::Control(c as u8)),
            _ => None,
        }
    }

    /// The label: an abbreviation for C0 controls and DEL, hex otherwise.
    pub fn label(self) -> String {
        const C0: [&str; 32] = [
            "NUL", "SOH", "STX", "ETX", "EOT", "ENQ", "ACK", "BEL", "BS", "HT", "LF", "VT", "FF",
            "CR", "SO", "SI", "DLE", "DC1", "DC2", "DC3", "DC4", "NAK", "SYN", "ETB", "CAN", "EM",
            "SUB", "ESC", "FS", "GS", "RS", "US",
        ];
        match self {
            Chip::Control(c) if usize::from(c) < C0.len() => C0[usize::from(c)].to_owned(),
            Chip::Control(0x7F) => "DEL".to_owned(),
            Chip::Control(c) | Chip::Byte(c) => format!("{c:02X}"),
        }
    }
}

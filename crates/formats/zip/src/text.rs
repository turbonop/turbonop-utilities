//! Names and times as ZIP records them: code page 437 (the names of
//! entries whose general-purpose flag 11 is clear) and MS-DOS dates and
//! times.

use std::borrow::Cow;

/// Code page 437's characters 0x80 to 0xFF (0x00 to 0x7F are ASCII's, as
/// every ZIP reader takes them).
const CP437_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', //
    'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥', '₧', 'ƒ', //
    'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', //
    '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗', '╝', '╜', '╛', '┐', //
    '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', //
    '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘', '┌', '█', '▄', '▌', '▐', '▀', //
    'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', //
    '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈', '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{A0}',
];

/// `bytes` decoded as code page 437 (borrowed when they're all ASCII).
pub fn decode_cp437(bytes: &[u8]) -> Cow<'_, str> {
    if bytes.is_ascii() {
        // (ASCII is UTF-8.)
        return String::from_utf8_lossy(bytes);
    }
    Cow::Owned(
        bytes
            .iter()
            .map(|&b| {
                if b < 0x80 {
                    char::from(b)
                } else {
                    CP437_HIGH[usize::from(b - 0x80)]
                }
            })
            .collect(),
    )
}

/// An MS-DOS date and time, as a ZIP header records an entry's
/// modification (local time, to two seconds, from 1980).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DosDateTime {
    /// The date: the year less 1980 (bits 9-15), month (5-8), day (0-4).
    pub date: u16,
    /// The time: hours (bits 11-15), minutes (5-10), seconds / 2 (0-4).
    pub time: u16,
}

impl DosDateTime {
    /// The year (1980 to 2107).
    pub fn year(self) -> u16 {
        1980 + (self.date >> 9)
    }

    /// The month (1 to 12, if it's valid).
    pub fn month(self) -> u8 {
        ((self.date >> 5) & 0xF) as u8
    }

    /// The day of the month (1 to 31, if it's valid).
    pub fn day(self) -> u8 {
        (self.date & 0x1F) as u8
    }

    /// The hour (0 to 23, if it's valid).
    pub fn hour(self) -> u8 {
        (self.time >> 11) as u8
    }

    /// The minute (0 to 59, if it's valid).
    pub fn minute(self) -> u8 {
        ((self.time >> 5) & 0x3F) as u8
    }

    /// The second (even: 0 to 58, if it's valid).
    pub fn second(self) -> u8 {
        (self.time & 0x1F) as u8 * 2
    }

    /// Whether every field is in its range (MS-DOS's zero date, 1980-00-00,
    /// isn't).
    pub fn is_valid(self) -> bool {
        (1..=12).contains(&self.month())
            && (1..=31).contains(&self.day())
            && self.hour() < 24
            && self.minute() < 60
            && self.second() < 60
    }
}

impl std::fmt::Display for DosDateTime {
    /// `YYYY-MM-DD hh:mm:ss`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year(),
            self.month(),
            self.day(),
            self.hour(),
            self.minute(),
            self.second()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cp437() {
        assert!(matches!(
            decode_cp437(b"abc/d.txt"),
            Cow::Borrowed("abc/d.txt")
        ));
        assert_eq!(decode_cp437(b"caf\x82"), "café");
        assert_eq!(decode_cp437(b"\x80\xFF\xE1"), "Ç\u{A0}ß");
        // Every byte decodes to one character.
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode_cp437(&all).chars().count(), 256);
    }

    #[test]
    fn dos_date_time() {
        // 2024-02-29 13:37:58.
        let t = DosDateTime {
            date: (44 << 9) | (2 << 5) | 29,
            time: (13 << 11) | (37 << 5) | 29,
        };
        assert_eq!(t.to_string(), "2024-02-29 13:37:58");
        assert!(t.is_valid());
        assert!(!DosDateTime { date: 0, time: 0 }.is_valid());
        assert_eq!(DosDateTime { date: 0, time: 0 }.year(), 1980);
        let max = DosDateTime {
            date: 0xFFFF,
            time: 0xFFFF,
        };
        assert_eq!(max.to_string(), "2107-15-31 31:63:62");
        assert!(!max.is_valid());
    }
}

//! Modified UTF-8 (MUTF-8), DEX's string encoding: UTF-16 code units, each
//! as UTF-8 encodes a code point of its value (so a supplementary
//! character is a surrogate pair of three bytes each), with NUL as the
//! two bytes `C0 80` so that strings can end with a zero byte.

/// Why bytes aren't MUTF-8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mutf8Error {
    /// Where the bad sequence starts.
    pub offset: usize,
}

/// The UTF-16 code units `bytes` encode. (Four-byte UTF-8 sequences,
/// which no DEX writer should emit but ART reads, give their surrogate
/// pairs.)
pub fn decode_utf16(bytes: &[u8]) -> Result<Vec<u16>, Mutf8Error> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let cont = |i: usize| -> Result<u16, Mutf8Error> {
        match bytes.get(i) {
            Some(&b) if b & 0xC0 == 0x80 => Ok(u16::from(b & 0x3F)),
            _ => Err(Mutf8Error { offset: i }),
        }
    };
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0 {
            // (A NUL ends a DEX string: none can be in one.)
            return Err(Mutf8Error { offset: i });
        }
        if b < 0x80 {
            out.push(u16::from(b));
            i += 1;
        } else if b & 0xE0 == 0xC0 {
            out.push((u16::from(b & 0x1F) << 6) | cont(i + 1).map_err(|_| Mutf8Error { offset: i })?);
            i += 2;
        } else if b & 0xF0 == 0xE0 {
            let (c1, c2) = (cont(i + 1), cont(i + 2));
            let (Ok(c1), Ok(c2)) = (c1, c2) else {
                return Err(Mutf8Error { offset: i });
            };
            out.push((u16::from(b & 0x0F) << 12) | (c1 << 6) | c2);
            i += 3;
        } else if b & 0xF8 == 0xF0 {
            let (Ok(c1), Ok(c2), Ok(c3)) = (cont(i + 1), cont(i + 2), cont(i + 3)) else {
                return Err(Mutf8Error { offset: i });
            };
            let cp = (u32::from(b & 0x07) << 18)
                | (u32::from(c1) << 12)
                | (u32::from(c2) << 6)
                | u32::from(c3);
            if !(0x1_0000..=0x10_FFFF).contains(&cp) {
                return Err(Mutf8Error { offset: i });
            }
            let v = cp - 0x1_0000;
            #[allow(clippy::cast_possible_truncation)]
            {
                out.push(0xD800 | (v >> 10) as u16);
                out.push(0xDC00 | (v & 0x3FF) as u16);
            }
            i += 4;
        } else {
            return Err(Mutf8Error { offset: i });
        }
    }
    Ok(out)
}

/// The string `bytes` encode, unpaired surrogates as U+FFFD; `None` if
/// they aren't MUTF-8.
pub fn decode(bytes: &[u8]) -> Result<String, Mutf8Error> {
    // ASCII (most strings) is itself.
    if bytes.is_ascii() && !bytes.contains(&0) {
        return Ok(String::from_utf8_lossy(bytes).into_owned());
    }
    Ok(char::decode_utf16(decode_utf16(bytes)?)
        .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect())
}

/// The string `bytes` encode, as far as they're MUTF-8: bad sequences and
/// unpaired surrogates as U+FFFD.
pub fn decode_lossy(bytes: &[u8]) -> String {
    match decode(bytes) {
        Ok(s) => s,
        Err(_) => {
            let mut out = String::new();
            let mut rest = bytes;
            while !rest.is_empty() {
                match decode(rest) {
                    Ok(s) => {
                        out.push_str(&s);
                        break;
                    }
                    Err(e) => {
                        out.push_str(&decode(&rest[..e.offset]).unwrap_or_default());
                        out.push(char::REPLACEMENT_CHARACTER);
                        rest = &rest[e.offset + 1..];
                    }
                }
            }
            out
        }
    }
}

/// The MUTF-8 encoding of `s`.
pub fn encode(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for u in s.encode_utf16() {
        encode_unit(u, &mut out);
    }
    out
}

/// The MUTF-8 encoding of a UTF-16 code unit.
#[allow(clippy::cast_possible_truncation)]
pub fn encode_unit(u: u16, out: &mut Vec<u8>) {
    if u != 0 && u < 0x80 {
        out.push(u as u8);
    } else if u < 0x800 {
        out.extend_from_slice(&[0xC0 | (u >> 6) as u8, 0x80 | (u & 0x3F) as u8]);
    } else {
        out.extend_from_slice(&[
            0xE0 | (u >> 12) as u8,
            0x80 | ((u >> 6) & 0x3F) as u8,
            0x80 | (u & 0x3F) as u8,
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for s in ["", "abc", "\u{e9}t\u{e9}", "\u{4e2d}", "nul\0nul", "\u{1F600}", "\u{ffff}"] {
            let e = encode(s);
            assert!(!e.contains(&0), "{s:?}");
            assert_eq!(decode(&e).unwrap(), s);
            assert_eq!(decode_utf16(&e).unwrap(), s.encode_utf16().collect::<Vec<_>>());
        }
        assert_eq!(encode("\0"), [0xC0, 0x80]);
        // A supplementary character: a surrogate pair, three bytes each.
        assert_eq!(encode("\u{1F600}"), [0xED, 0xA0, 0xBD, 0xED, 0xB8, 0x80]);
        // ... which four-byte UTF-8 also reads as.
        assert_eq!(decode(&[0xF0, 0x9F, 0x98, 0x80]).unwrap(), "\u{1F600}");
    }

    #[test]
    fn unpaired_surrogates_and_bad_bytes() {
        let mut lone = Vec::new();
        encode_unit(0xD800, &mut lone);
        lone.push(b'x');
        assert_eq!(decode_utf16(&lone).unwrap(), [0xD800, u16::from(b'x')]);
        assert_eq!(decode(&lone).unwrap(), "\u{FFFD}x");
        for bad in [&[0x80][..], &[0xC0], &[0xE0, 0x80], &[0xF8], &[0], &[0xF4, 0x90, 0x80, 0x80]] {
            assert!(decode(bad).is_err(), "{bad:?}");
        }
        assert_eq!(decode(&[b'a', 0xFF, b'b']).unwrap_err().offset, 1);
        assert_eq!(decode_lossy(&[b'a', 0xFF, b'b', 0xC3]), "a\u{FFFD}b\u{FFFD}");
    }
}

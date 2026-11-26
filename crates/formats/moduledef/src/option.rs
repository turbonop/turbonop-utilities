//! `/EXPORT:` specifications, on `lib.exe`'s command line and in objects'
//! `.drectve` sections, as `lib.exe` 14.52 reads them (measured case by
//! case):
//!
//! `name[=internal][,@ordinal[,NONAME]][,DATA][,PRIVATE][,EXPORTAS,name]`
//!
//! - The name runs to the first `=` (it may hold spaces, and be empty); the
//!   internal name is the rest, `=` included (`A=B=C`'s is `B=C`). An
//!   empty internal name (`A=`) is kept, with a warning (LNK4051).
//! - The fields are separated by commas, in that order, keywords in any
//!   case (`CONSTANT` is not one): anything else, or out of order, is
//!   fatal (LNK1161, "invalid export specification").
//! - `@ordinal`: a decimal number as `strtoul` reads it (spaces before it,
//!   what follows its digits ignored: `@5NONAME` is 5), from 1 to 65535
//!   (else LNK1119, naming what follows the `@`).
//! - An empty field ends the specification, and what follows is ignored
//!   (`A,,DATA` is code); a specification that ends with a comma is
//!   warned of (LNK4051, "extra delimiter in /EXPORT specification").

use crate::{Export, ExportKind};

/// Why an `/EXPORT` specification can't be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OptionError<'a> {
    /// LNK1161: "invalid export specification".
    Invalid,
    /// LNK1119: "invalid ordinal number '...'", with what follows the `@`.
    InvalidOrdinal(&'a [u8]),
}

/// An export specification read: the export, and whether it gets LNK4051.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExportOption<'a> {
    /// The export (its `line` is 0).
    pub export: Export<'a>,
    /// Whether `lib.exe` warns of an extra delimiter (LNK4051).
    pub extra_delimiter: bool,
}

/// A decimal number as `strtoul` reads it: spaces, an optional sign, and
/// digits, with what follows ignored.
fn strtoul(s: &[u8]) -> Option<u64> {
    let s = &s[s
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(s.len())..];
    let (neg, s) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits = s.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return None;
    }
    let v = s[..digits].iter().fold(0u64, |v, &d| {
        v.saturating_mul(10).saturating_add(u64::from(d - b'0'))
    });
    Some(if neg { v.wrapping_neg() } else { v })
}

/// Reads an `/EXPORT:` specification (what follows the `:`).
pub fn parse_export_option(spec: &[u8]) -> Result<ExportOption<'_>, OptionError<'_>> {
    let mut fields = spec.split(|&b| b == b',');
    let head = fields.next().unwrap_or_default();
    let (name, internal) = match head.iter().position(|&b| b == b'=') {
        Some(at) => (&head[..at], Some(&head[at + 1..])),
        None => (head, None),
    };
    let mut e = Export {
        name,
        internal,
        ordinal: None,
        noname: false,
        kind: ExportKind::Code,
        private: false,
        export_as: None,
        line: 0,
    };
    let mut extra_delimiter = spec.last() == Some(&b',') || internal == Some(&[][..]);
    // The fields' order: an ordinal (and NONAME), DATA, PRIVATE, EXPORTAS.
    let mut stage = 0;
    while let Some(f) = fields.next() {
        if f.is_empty() {
            break;
        }
        let is = |k: &[u8]| f.eq_ignore_ascii_case(k);
        if let Some(text) = f.strip_prefix(b"@") {
            if stage > 0 {
                return Err(OptionError::Invalid);
            }
            let o = strtoul(text)
                .and_then(|v| u16::try_from(v).ok())
                .filter(|&v| v != 0)
                .ok_or(OptionError::InvalidOrdinal(text))?;
            e.ordinal = Some(o);
            stage = 1;
        } else if is(b"NONAME") {
            if stage != 1 {
                return Err(OptionError::Invalid);
            }
            e.noname = true;
            stage = 2;
        } else if is(b"DATA") {
            if stage > 2 {
                return Err(OptionError::Invalid);
            }
            e.kind = ExportKind::Data;
            stage = 3;
        } else if is(b"PRIVATE") {
            if stage > 3 {
                return Err(OptionError::Invalid);
            }
            e.private = true;
            stage = 4;
        } else if is(b"EXPORTAS") {
            let Some(to) = fields.next().filter(|t| !t.is_empty()) else {
                return Err(OptionError::Invalid);
            };
            e.export_as = Some(to);
            if fields.next().is_some_and(|f| !f.is_empty()) {
                return Err(OptionError::Invalid);
            }
            extra_delimiter = spec.last() == Some(&b',');
            break;
        } else {
            return Err(OptionError::Invalid);
        }
    }
    Ok(ExportOption {
        export: e,
        extra_delimiter,
    })
}

/// The `/EXPORT:` (or `-EXPORT:`, in any case) specifications of a
/// `.drectve` section, in order: its words are separated by spaces, and a
/// quoted part may hold them (the quotes are dropped).
pub fn drectve_exports(drectve: &[u8]) -> alloc::vec::Vec<alloc::vec::Vec<u8>> {
    let mut out = alloc::vec::Vec::new();
    let mut word = alloc::vec::Vec::new();
    let mut quoted = false;
    let mut words = alloc::vec::Vec::new();
    for &b in drectve {
        match b {
            b'"' => quoted = !quoted,
            b' ' | b'\t' | b'\r' | b'\n' | 0 if !quoted => {
                if !word.is_empty() {
                    words.push(core::mem::take(&mut word));
                }
            }
            _ => word.push(b),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    for w in words {
        if w.len() > 8 && matches!(w[0], b'/' | b'-') && w[1..8].eq_ignore_ascii_case(b"EXPORT:") {
            out.push(w[8..].to_vec());
        }
    }
    out
}

#[cfg(test)]
mod tests;

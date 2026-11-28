//! Method bodies (II.25.4): a tiny header (code under 64 bytes, a stack of
//! 8, no locals, no exception handling) or a fat one, the IL code, and the
//! extra data sections after it (exception handling clauses, small or
//! fat).

use turbonop_core::{ErrorKind, Result};

use crate::reader::Cursor;
use crate::tables::Token;

/// Header flags.
pub mod flags {
    /// A tiny header.
    pub const TINY_FORMAT: u16 = 0x2;
    /// A fat header.
    pub const FAT_FORMAT: u16 = 0x3;
    /// Data sections follow the code.
    pub const MORE_SECTS: u16 = 0x8;
    /// Locals are zeroed.
    pub const INIT_LOCALS: u16 = 0x10;
}

/// Data section kinds.
pub mod section {
    /// Exception handling clauses.
    pub const EH_TABLE: u8 = 0x01;
    /// (Reserved.) An optimized IL table.
    pub const OPT_IL_TABLE: u8 = 0x02;
    /// Fat clauses (24 bytes, not 12).
    pub const FAT_FORMAT: u8 = 0x40;
    /// Another section follows.
    pub const MORE_SECTS: u8 = 0x80;
}

/// What kind of handler a clause has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ClauseKind {
    /// `catch` of a type.
    Catch(Token),
    /// A filter, at this offset, then its handler.
    Filter(u32),
    /// `finally`.
    Finally,
    /// `fault`.
    Fault,
    /// Flags no kind has.
    Unknown(u32),
}

/// An exception handling clause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExceptionClause {
    /// Its flags (0 catch, 1 filter, 2 finally, 4 fault).
    pub flags: u32,
    /// The protected block's offset.
    pub try_offset: u32,
    /// Its length.
    pub try_length: u32,
    /// The handler's offset.
    pub handler_offset: u32,
    /// Its length.
    pub handler_length: u32,
    /// What it handles.
    pub kind: ClauseKind,
    /// Whether it was a fat clause.
    pub fat: bool,
}

/// A data section after the code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DataSection<'d> {
    /// Its kind byte.
    pub kind: u8,
    /// Its offset from the body's start.
    pub offset: u32,
    /// Its size, header included.
    pub size: u32,
    /// Its bytes, header included.
    pub data: &'d [u8],
}

/// A method body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodBody<'d> {
    /// Whether the header was fat.
    pub fat: bool,
    /// The header's flags (12 bits; with a tiny header, its format bits).
    pub flags: u16,
    /// The header's size.
    pub header_size: u32,
    /// The deepest the stack gets.
    pub max_stack: u16,
    /// The locals' StandAloneSig token (null if none).
    pub local_var_sig: Token,
    /// The code.
    pub code: &'d [u8],
    /// Exception handling clauses, from every EH section.
    pub clauses: Vec<ExceptionClause>,
    /// The data sections.
    pub sections: Vec<DataSection<'d>>,
    /// The body's whole size: header, code, padding, and sections.
    pub size: u32,
}

impl MethodBody<'_> {
    /// Whether locals are zeroed.
    pub fn init_locals(&self) -> bool {
        self.fat && self.flags & flags::INIT_LOCALS != 0
    }
}

fn clause(c: &mut Cursor<'_>, fat: bool) -> Result<ExceptionClause> {
    let w = |c: &mut Cursor<'_>, n: usize| c.uint(n, "exception clause");
    let (f, to, tl, ho, hl) = if fat {
        (w(c, 4)?, w(c, 4)?, w(c, 4)?, w(c, 4)?, w(c, 4)?)
    } else {
        (w(c, 2)?, w(c, 2)?, w(c, 1)?, w(c, 2)?, w(c, 1)?)
    };
    let extra = c.u32("exception clause")?;
    let kind = match f {
        0 => ClauseKind::Catch(Token(extra)),
        1 => ClauseKind::Filter(extra),
        2 => ClauseKind::Finally,
        4 => ClauseKind::Fault,
        f => ClauseKind::Unknown(f),
    };
    Ok(ExceptionClause {
        flags: f,
        try_offset: to,
        try_length: tl,
        handler_offset: ho,
        handler_length: hl,
        kind,
        fat,
    })
}

/// Parses the method body at the start of `data` (which may run past it:
/// to the end of its section). `base`, the body's file offset, is for
/// errors.
pub fn parse_method_body(data: &[u8], base: u64) -> Result<MethodBody<'_>> {
    let mut c = Cursor::with_base(data, base);
    let first = c.u8("method body header")?;
    match u16::from(first) & 3 {
        flags::TINY_FORMAT => {
            let code = c.bytes(usize::from(first >> 2), "method body code")?;
            Ok(MethodBody {
                fat: false,
                flags: u16::from(first & 3),
                header_size: 1,
                max_stack: 8,
                local_var_sig: Token(0),
                code,
                clauses: Vec::new(),
                sections: Vec::new(),
                size: 1 + u32::from(first >> 2),
            })
        }
        flags::FAT_FORMAT => {
            let word = u16::from(first) | (u16::from(c.u8("method body header")?) << 8);
            let header_flags = word & 0x0FFF;
            let header_size = usize::from(word >> 12) * 4;
            if header_size < 12 {
                return Err(c.error(ErrorKind::Malformed, "method body header size"));
            }
            let max_stack = c.u16("method body header")?;
            let code_size = c.u32("method body header")?;
            let local_var_sig = Token(c.u32("method body header")?);
            c.seek(header_size, "method body header")?;
            let code = c.bytes(code_size as usize, "method body code")?;
            let mut clauses = Vec::new();
            let mut sections = Vec::new();
            let mut more = header_flags & flags::MORE_SECTS != 0;
            while more {
                c.align(4, "method data section")?;
                let start = c.pos();
                let kind = c.u8("method data section")?;
                let fat = kind & section::FAT_FORMAT != 0;
                let size = if fat {
                    c.uint(3, "method data section")? as usize
                } else {
                    let s = usize::from(c.u8("method data section")?);
                    c.u16("method data section")?;
                    s
                };
                if size < 4 {
                    return Err(c.error(ErrorKind::Malformed, "method data section size"));
                }
                let data = c
                    .data()
                    .get(start..start + size)
                    .ok_or_else(|| c.error(ErrorKind::Truncated, "method data section"))?;
                if kind & section::EH_TABLE != 0 {
                    let each = if fat { 24 } else { 12 };
                    let mut s = Cursor::with_base(&data[4..], base + start as u64 + 4);
                    for _ in 0..(size - 4) / each {
                        clauses.push(clause(&mut s, fat)?);
                    }
                }
                sections.push(DataSection {
                    kind,
                    offset: u32::try_from(start).unwrap_or(u32::MAX),
                    size: u32::try_from(size).unwrap_or(u32::MAX),
                    data,
                });
                c.seek(start + size, "method data section")?;
                more = kind & section::MORE_SECTS != 0;
            }
            Ok(MethodBody {
                fat: true,
                flags: header_flags,
                header_size: u32::try_from(header_size).unwrap_or(u32::MAX),
                max_stack,
                local_var_sig,
                code,
                clauses,
                sections,
                size: u32::try_from(c.pos()).unwrap_or(u32::MAX),
            })
        }
        _ => Err(c.error(ErrorKind::Malformed, "method body header")),
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn tiny() {
        let b = parse_method_body(&[(3 << 2) | 2, 0x02, 0x03, 0x2A, 0xFF], 0).unwrap();
        assert!(!b.fat && !b.init_locals());
        assert_eq!((b.code, b.max_stack, b.size), (&[2, 3, 0x2A][..], 8, 4));
        assert!(parse_method_body(&[(3 << 2) | 2, 0x02], 0).is_err());
        assert!(parse_method_body(&[0x01], 0).is_err());
        assert!(parse_method_body(&[], 0).is_err());
    }

    fn fat(flags: u16, code: &[u8], sections: &[u8]) -> Vec<u8> {
        let mut v = ((3u16 << 12) | flags | 3).to_le_bytes().to_vec();
        v.extend_from_slice(&5u16.to_le_bytes());
        v.extend_from_slice(&(code.len() as u32).to_le_bytes());
        v.extend_from_slice(&0x1100_0002u32.to_le_bytes());
        v.extend_from_slice(code);
        while !v.len().is_multiple_of(4) {
            v.push(0xEE);
        }
        v.extend_from_slice(sections);
        v
    }

    #[test]
    fn fat_with_clauses() {
        // A small EH section (two clauses), then a fat one (one).
        let mut s = vec![section::EH_TABLE | section::MORE_SECTS, 28, 0, 0];
        s.extend_from_slice(&[0, 0, 1, 0, 4, 5, 0, 2, 0x01, 0, 0, 0x01]); // catch 0x01000001
        s.extend_from_slice(&[2, 0, 1, 0, 4, 7, 0, 1, 0, 0, 0, 0]); // finally
        s.extend_from_slice(&[section::EH_TABLE | section::FAT_FORMAT, 28, 0, 0]);
        for v in [1u32, 0, 10, 10, 5, 3] {
            s.extend_from_slice(&v.to_le_bytes());
        }
        let data = fat(flags::MORE_SECTS | flags::INIT_LOCALS, &[0; 15], &s);
        let b = parse_method_body(&data, 0).unwrap();
        assert!(b.fat && b.init_locals());
        assert_eq!(
            (b.max_stack, b.local_var_sig, b.code.len()),
            (5, Token(0x1100_0002), 15)
        );
        assert_eq!(b.clauses.len(), 3);
        assert_eq!(b.clauses[0].kind, ClauseKind::Catch(Token(0x0100_0001)));
        assert_eq!((b.clauses[0].try_offset, b.clauses[0].try_length), (1, 4));
        assert_eq!(
            (b.clauses[0].handler_offset, b.clauses[0].handler_length),
            (5, 2)
        );
        assert_eq!(b.clauses[1].kind, ClauseKind::Finally);
        assert_eq!(b.clauses[2].kind, ClauseKind::Filter(3));
        assert_eq!(b.sections.len(), 2);
        assert_eq!(b.size as usize, data.len());
        for n in 0..data.len() {
            assert!(parse_method_body(&data[..n], 0).is_err(), "{n}");
        }
        // A section whose size is too small to move on.
        let data = fat(
            flags::MORE_SECTS,
            &[0x2A],
            &[section::EH_TABLE | section::MORE_SECTS, 0, 0, 0],
        );
        assert!(parse_method_body(&data, 0).is_err());
        // A header smaller than 12 bytes.
        let mut data = fat(0, &[0x2A], &[]);
        data[1] = 0x20;
        assert!(parse_method_body(&data, 0).is_err());
        // A huge code size.
        let mut data = fat(0, &[0x2A], &[]);
        data[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_method_body(&data, 0).is_err());
    }
}

//! Where a type or id record refers to other records: the offsets of its
//! index fields.
//!
//! Merging type streams, hashing records structurally, and renumbering
//! them all need every field that holds a type or id index. A field missed
//! silently corrupts whatever is built from the records, so this is
//! exhaustive for the records compilers write: a kind it doesn't know is
//! an error ([`RefError::Unknown`]), never "no references".
//!
//! In a COFF object, types and ids share one index space (a PDB splits
//! them into its TPI and IPI streams), so each field is also marked with
//! which of the two it names ([`Ref::id`]).

use crate::types::{leaf, read_numeric};
use turbonop_core::Reader;

/// One index field of a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ref {
    /// Its offset in the record's body (after the kind).
    pub offset: u32,
    /// Whether it names an id record (`LF_FUNC_ID`, `LF_STRING_ID`, ...)
    /// rather than a type record.
    pub id: bool,
}

/// Why a record's index fields could not be found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefError {
    /// A record kind (or, in a field list, a member kind) this doesn't
    /// know.
    Unknown(u16),
    /// The record is shorter than its kind's fields, or a field list's
    /// member is.
    Malformed(u16),
}

impl core::fmt::Display for RefError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RefError::Unknown(k) => write!(f, "unknown type record kind {k:#06x}"),
            RefError::Malformed(k) => write!(f, "malformed type record (kind {k:#06x})"),
        }
    }
}

/// The method properties that carry a virtual-base offset
/// (`CV_MTintro`, `CV_MTpureintro`).
fn introduces_virtual(attributes: u16) -> bool {
    use crate::types::field_attr::{CV_MTINTRO, CV_MTPUREINTRO, MPROP_MASK, MPROP_SHIFT};
    let mprop = (attributes >> MPROP_SHIFT) & MPROP_MASK;
    mprop == CV_MTINTRO || mprop == CV_MTPUREINTRO
}

struct Out<'v> {
    refs: &'v mut Vec<Ref>,
    kind: u16,
    len: usize,
}

impl Out<'_> {
    /// `count` consecutive index fields at `offset`.
    fn at(&mut self, offset: usize, count: usize, id: bool) -> Result<(), RefError> {
        let end = count
            .checked_mul(4)
            .and_then(|n| n.checked_add(offset))
            .ok_or(RefError::Malformed(self.kind))?;
        if end > self.len {
            return Err(RefError::Malformed(self.kind));
        }
        for i in 0..count {
            self.refs.push(Ref {
                offset: (offset + 4 * i) as u32,
                id,
            });
        }
        Ok(())
    }
}

fn u16_at(body: &[u8], offset: usize, kind: u16) -> Result<u16, RefError> {
    body.get(offset..offset + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or(RefError::Malformed(kind))
}

fn u32_at(body: &[u8], offset: usize, kind: u16) -> Result<u32, RefError> {
    body.get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(RefError::Malformed(kind))
}

/// The index fields of the record of kind `kind` whose body (the bytes
/// after its kind) is `body`, appended to `refs` in the order they appear.
pub fn refs(kind: u16, body: &[u8], refs: &mut Vec<Ref>) -> Result<(), RefError> {
    let mut out = Out {
        refs,
        kind,
        len: body.len(),
    };
    match kind {
        // Records without references.
        leaf::LF_VTSHAPE
        | leaf::LF_LABEL
        | leaf::LF_ENDPRECOMP
        | leaf::LF_PRECOMP
        | leaf::LF_TYPESERVER2
        | LF_TYPESERVER
        | LF_SKIP => Ok(()),

        // type(4) attributes(2)
        leaf::LF_MODIFIER => out.at(0, 1, false),
        // referent(4) attributes(4) [containing class(4) for members]
        leaf::LF_POINTER => {
            out.at(0, 1, false)?;
            let attributes = u32_at(body, 4, kind)?;
            let mode = (attributes >> crate::types::pointer::PTRMODE_SHIFT)
                & crate::types::pointer::PTRMODE_MASK;
            if mode == crate::types::pointer::CV_PTR_MODE_PMEM
                || mode == crate::types::pointer::CV_PTR_MODE_PMFUNC
            {
                out.at(8, 1, false)?;
            }
            Ok(())
        }
        // return(4) callconv(1) attrs(1) count(2) arglist(4)
        leaf::LF_PROCEDURE => {
            out.at(0, 1, false)?;
            out.at(8, 1, false)
        }
        // return(4) class(4) this(4) callconv(1) attrs(1) count(2) arglist(4) adjust(4)
        leaf::LF_MFUNCTION => {
            out.at(0, 3, false)?;
            out.at(16, 1, false)
        }
        // count(4) types(4 × count)
        leaf::LF_ARGLIST => {
            let n = u32_at(body, 0, kind)? as usize;
            out.at(4, n, false)
        }
        // type(4) length(1) position(1)
        leaf::LF_BITFIELD => out.at(0, 1, false),
        // (attributes(2) pad(2) type(4) [vbaseoff(4)])*
        leaf::LF_METHODLIST => {
            let mut at = 0;
            while at < body.len() {
                let attributes = u16_at(body, at, kind)?;
                out.at(at + 4, 1, false)?;
                at += 8;
                if introduces_virtual(attributes) {
                    u32_at(body, at, kind)?;
                    at += 4;
                }
            }
            Ok(())
        }
        // element(4) index(4) size name
        leaf::LF_ARRAY | LF_STRIDED_ARRAY => out.at(0, 2, false),
        // count(2) properties(2) fields(4) derived(4) vshape(4) size name [unique]
        leaf::LF_CLASS | leaf::LF_STRUCTURE | leaf::LF_INTERFACE => out.at(4, 3, false),
        // properties(4) fields(4) derived(4) vshape(4) count(2) size name [unique]
        leaf::LF_CLASS2 | leaf::LF_STRUCTURE2 | leaf::LF_INTERFACE2 => out.at(4, 3, false),
        // count(2) properties(2) fields(4) size name [unique]
        leaf::LF_UNION => out.at(4, 1, false),
        // properties(4) fields(4) count(2) size name [unique]
        leaf::LF_UNION2 => out.at(4, 1, false),
        // count(2) properties(2) underlying(4) fields(4) name [unique]
        leaf::LF_ENUM => out.at(4, 2, false),
        // underlying(4) name
        LF_ALIAS => out.at(0, 1, false),
        // complete class(4) overridden vftable(4) offset(4) names length(4) names
        leaf::LF_VFTABLE => out.at(0, 2, false),
        // OEM id(2) record id(2) count(4) types(4 × count)
        LF_OEM => {
            let n = u32_at(body, 4, kind)? as usize;
            out.at(8, n, false)
        }
        // type(4) count(2) modifiers(2 × count)
        LF_MODIFIER_EX => out.at(0, 1, false),
        // subtype(4) kind(2) count(2) ...
        LF_HLSL => out.at(0, 1, false),
        // element(4) count(4) size name
        LF_VECTOR => out.at(0, 1, false),
        // element(4) rows(4) columns(4) stride(4) attributes(1) size name
        LF_MATRIX => out.at(0, 1, false),
        leaf::LF_FIELDLIST => field_list(body, &mut out),

        // Ids.
        // scope id(4) type(4) name
        leaf::LF_FUNC_ID => {
            out.at(0, 1, true)?;
            out.at(4, 1, false)
        }
        // parent type(4) type(4) name
        leaf::LF_MFUNC_ID => out.at(0, 2, false),
        // count(2) ids(4 × count)
        leaf::LF_BUILDINFO => {
            let n = usize::from(u16_at(body, 0, kind)?);
            out.at(2, n, true)
        }
        // count(4) ids(4 × count)
        leaf::LF_SUBSTR_LIST => {
            let n = u32_at(body, 0, kind)? as usize;
            out.at(4, n, true)
        }
        // substring list id(4) name
        leaf::LF_STRING_ID => out.at(0, 1, true),
        // type(4) source file id(4) line(4)
        leaf::LF_UDT_SRC_LINE => {
            out.at(0, 1, false)?;
            out.at(4, 1, true)
        }
        // type(4) source file (a /names offset, not an index)(4) line(4) module(2)
        leaf::LF_UDT_MOD_SRC_LINE => out.at(0, 1, false),

        _ => Err(RefError::Unknown(kind)),
    }
}

// Kinds the `leaf` module doesn't name (cvinfo.h's values).
const LF_SKIP: u16 = 0x1200;
const LF_TYPESERVER: u16 = 0x1501;
const LF_ALIAS: u16 = 0x150A;
const LF_OEM: u16 = 0x100F;
const LF_STRIDED_ARRAY: u16 = 0x1516;
const LF_HLSL: u16 = 0x1517;
const LF_MODIFIER_EX: u16 = 0x1518;
const LF_VECTOR: u16 = 0x151B;
const LF_MATRIX: u16 = 0x151C;

// Field-list members.
const LF_FRIENDCLS: u16 = 0x140B;
const LF_VFUNCOFF: u16 = 0x140C;
const LF_FRIENDFCN: u16 = 0x150C;
const LF_NESTTYPEEX: u16 = 0x1512;
const LF_MEMBERMODIFY: u16 = 0x1513;
const LF_MANAGED: u16 = 0x1514;

/// A field list: members, each padded to 4 bytes with `LF_PAD*` bytes.
fn field_list(body: &[u8], out: &mut Out<'_>) -> Result<(), RefError> {
    let mut r = Reader::new(body);
    let malformed = |kind| RefError::Malformed(kind);
    while !r.is_empty() {
        // Padding: a byte of LF_PAD1..LF_PAD15 says how many bytes to skip
        // (itself included).
        if let Some(&b) = r.remaining().first()
            && b > leaf::LF_PAD0 as u8
        {
            r.skip(usize::from(b & 0x0F).max(1))
                .map_err(|_| malformed(leaf::LF_FIELDLIST))?;
            continue;
        }
        let start = r.position();
        let member = r.u16_le().map_err(|_| malformed(leaf::LF_FIELDLIST))?;
        let m = |_| malformed(member);
        let at = |r: &Reader<'_>| r.position();
        match member {
            // attributes(2) type(4) offset(numeric)
            leaf::LF_BCLASS | leaf::LF_BINTERFACE => {
                r.skip(2).map_err(m)?;
                out.at(at(&r), 1, false)?;
                r.skip(4).map_err(m)?;
                read_numeric(&mut r).map_err(m)?;
            }
            // attributes(2) base(4) vbptr type(4) vbpoff(numeric) vboff(numeric)
            leaf::LF_VBCLASS | leaf::LF_IVBCLASS => {
                r.skip(2).map_err(m)?;
                out.at(at(&r), 2, false)?;
                r.skip(8).map_err(m)?;
                read_numeric(&mut r).map_err(m)?;
                read_numeric(&mut r).map_err(m)?;
            }
            // pad(2) continuation(4)
            leaf::LF_INDEX | leaf::LF_VFUNCTAB | LF_FRIENDCLS => {
                r.skip(2).map_err(m)?;
                out.at(at(&r), 1, false)?;
                r.skip(4).map_err(m)?;
            }
            // pad(2) type(4) offset(4)
            LF_VFUNCOFF => {
                r.skip(2).map_err(m)?;
                out.at(at(&r), 1, false)?;
                r.skip(8).map_err(m)?;
            }
            // attributes(2) value(numeric) name
            leaf::LF_ENUMERATE => {
                r.skip(2).map_err(m)?;
                read_numeric(&mut r).map_err(m)?;
                r.cstr().map_err(m)?;
            }
            // attributes(2) type(4) offset(numeric) name
            leaf::LF_MEMBER => {
                r.skip(2).map_err(m)?;
                out.at(at(&r), 1, false)?;
                r.skip(4).map_err(m)?;
                read_numeric(&mut r).map_err(m)?;
                r.cstr().map_err(m)?;
            }
            // attributes(2) type(4) name (pad(2) for friends and nested types)
            leaf::LF_STMEMBER
            | leaf::LF_NESTTYPE
            | LF_NESTTYPEEX
            | LF_FRIENDFCN
            | LF_MEMBERMODIFY => {
                r.skip(2).map_err(m)?;
                out.at(at(&r), 1, false)?;
                r.skip(4).map_err(m)?;
                r.cstr().map_err(m)?;
            }
            // count(2) method list(4) name
            leaf::LF_METHOD => {
                r.skip(2).map_err(m)?;
                out.at(at(&r), 1, false)?;
                r.skip(4).map_err(m)?;
                r.cstr().map_err(m)?;
            }
            // attributes(2) type(4) [vbaseoff(4)] name
            leaf::LF_ONEMETHOD => {
                let attributes = r.u16_le().map_err(m)?;
                out.at(at(&r), 1, false)?;
                r.skip(4).map_err(m)?;
                if introduces_virtual(attributes) {
                    r.skip(4).map_err(m)?;
                }
                r.cstr().map_err(m)?;
            }
            // name
            LF_MANAGED => {
                r.cstr().map_err(m)?;
            }
            _ => return Err(RefError::Unknown(member)),
        }
        debug_assert!(r.position() > start);
    }
    Ok(())
}

#[cfg(test)]
mod tests;

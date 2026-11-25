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
    /// A kind (or member kind) from before 32-bit type indices (`*_16t`),
    /// whose indices are 16 bits wide. Known, but not supported: no
    /// compiler since Visual C++ 6 writes them.
    Legacy(u16),
}

impl core::fmt::Display for RefError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RefError::Unknown(k) => write!(f, "unknown type record kind {k:#06x}"),
            RefError::Malformed(k) => write!(f, "malformed type record (kind {k:#06x})"),
            RefError::Legacy(k) => write!(f, "16-bit type record kind {k:#06x}"),
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
        // (LF_SKIP's index is the next record's: a number, not a
        // reference.)
        leaf::LF_VTSHAPE
        | leaf::LF_LABEL
        | leaf::LF_ENDPRECOMP
        | leaf::LF_PRECOMP
        | LF_PRECOMP_ST
        | leaf::LF_TYPESERVER2
        | LF_TYPESERVER
        | LF_TYPESERVER_ST
        | LF_SKIP
        | LF_NULL
        | LF_NOTTRAN
        | LF_COBOL1 => Ok(()),

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
        leaf::LF_ARRAY | LF_ARRAY_ST | LF_STRIDED_ARRAY => out.at(0, 2, false),
        // count(2) properties(2) fields(4) derived(4) vshape(4) size name [unique]
        leaf::LF_CLASS
        | leaf::LF_STRUCTURE
        | leaf::LF_INTERFACE
        | LF_CLASS_ST
        | LF_STRUCTURE_ST => out.at(4, 3, false),
        // properties(4) fields(4) derived(4) vshape(4) count(2) size name [unique]
        leaf::LF_CLASS2 | leaf::LF_STRUCTURE2 | leaf::LF_INTERFACE2 => out.at(4, 3, false),
        // count(2) properties(2) fields(4) size name [unique]
        leaf::LF_UNION | LF_UNION_ST => out.at(4, 1, false),
        // properties(4) fields(4) count(2) size name [unique]
        leaf::LF_UNION2 => out.at(4, 1, false),
        // count(2) properties(2) underlying(4) fields(4) name [unique]
        leaf::LF_ENUM | LF_ENUM_ST => out.at(4, 2, false),
        // underlying(4) name
        LF_ALIAS | LF_ALIAS_ST => out.at(0, 1, false),
        // underlying(4) dimensions(4) name
        LF_DIMARRAY | LF_DIMARRAY_ST => out.at(0, 2, false),
        // count(4) bases(4 × count), from the root
        // count(4) derived classes(4 × count)
        LF_VFTPATH | LF_DERIVED => {
            let n = u32_at(body, 0, kind)? as usize;
            out.at(4, n, false)
        }
        // type(4) expression
        LF_DEFARG | LF_DEFARG_ST => out.at(0, 1, false),
        // index type(4) rank(2) bounds
        LF_DIMCONU | LF_DIMCONLU => out.at(0, 1, false),
        // rank(4) index type(4) bounds(4 × rank, or 4 × 2 × rank with lower
        // bounds): each an LF_REFSYM or T_VOID
        LF_DIMVARU | LF_DIMVARLU => {
            let rank = u32_at(body, 0, kind)? as usize;
            let n = if kind == LF_DIMVARLU {
                rank.checked_mul(2).ok_or(RefError::Malformed(kind))?
            } else {
                rank
            };
            out.at(4, 1, false)?;
            out.at(8, n, false)
        }
        // underlying(4)
        LF_BARRAY => out.at(0, 1, false),
        // parent(4) data
        LF_COBOL0 => out.at(0, 1, false),
        // GUID(16) count(4) types(4 × count) data
        LF_OEM2 => {
            let n = u32_at(body, 16, kind)? as usize;
            out.at(20, n, false)
        }
        // A symbol record, whole (its length first): its indices.
        LF_REFSYM => ref_sym(body, &mut out),
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

        // Before 32-bit type indices.
        0x0001..=0x0009 | 0x000B | 0x000D | 0x0011..=0x0013 | 0x0015 | 0x0200..=0x020B => {
            Err(RefError::Legacy(kind))
        }

        _ => Err(RefError::Unknown(kind)),
    }
}

/// `LF_REFSYM`'s symbol: its type and id indices, at their offsets in the
/// type record's body.
fn ref_sym(body: &[u8], out: &mut Out<'_>) -> Result<(), RefError> {
    let kind = out.kind;
    let len = usize::from(u16_at(body, 0, kind)?);
    let record = body
        .get(2..2 + len)
        .filter(|r| r.len() >= 2)
        .ok_or(RefError::Malformed(kind))?;
    let symbol = u16_at(record, 0, kind)?;
    let mut inner = Vec::new();
    crate::symbol_refs::refs(symbol, &record[2..], &mut inner)
        .map_err(|_| RefError::Malformed(kind))?;
    for r in inner {
        out.at(4 + r.offset as usize, 1, r.id)?;
    }
    Ok(())
}

// Kinds the `leaf` module doesn't name (cvinfo.h's values).
const LF_SKIP: u16 = 0x1200;
const LF_TYPESERVER: u16 = 0x1501;
const LF_TYPESERVER_ST: u16 = 0x0016;
const LF_NULL: u16 = 0x000F;
const LF_NOTTRAN: u16 = 0x0010;
const LF_COBOL1: u16 = 0x000C;
const LF_REFSYM: u16 = 0x020C;
const LF_ARRAY_ST: u16 = 0x1003;
const LF_CLASS_ST: u16 = 0x1004;
const LF_STRUCTURE_ST: u16 = 0x1005;
const LF_UNION_ST: u16 = 0x1006;
const LF_ENUM_ST: u16 = 0x1007;
const LF_COBOL0: u16 = 0x100A;
const LF_BARRAY: u16 = 0x100B;
const LF_DIMARRAY_ST: u16 = 0x100C;
const LF_VFTPATH: u16 = 0x100D;
const LF_PRECOMP_ST: u16 = 0x100E;
const LF_ALIAS_ST: u16 = 0x1010;
const LF_OEM2: u16 = 0x1011;
const LF_DEFARG_ST: u16 = 0x1202;
const LF_DERIVED: u16 = 0x1204;
const LF_DIMCONU: u16 = 0x1207;
const LF_DIMCONLU: u16 = 0x1208;
const LF_DIMVARU: u16 = 0x1209;
const LF_DIMVARLU: u16 = 0x120A;
const LF_DIMARRAY: u16 = 0x1508;
const LF_DEFARG: u16 = 0x150B;
const LF_ALIAS: u16 = 0x150A;
const LF_OEM: u16 = 0x100F;
const LF_STRIDED_ARRAY: u16 = 0x1516;
const LF_HLSL: u16 = 0x1517;
const LF_MODIFIER_EX: u16 = 0x1518;
const LF_VECTOR: u16 = 0x151B;
const LF_MATRIX: u16 = 0x151C;

// Field-list members. (LF_FRIENDCLS is 0x140A, as cvinfo.h and LLVM have
// it; 0x140B is LF_ONEMETHOD_ST.)
const LF_FRIENDCLS: u16 = 0x140A;
const LF_VFUNCOFF: u16 = 0x140C;
const LF_FRIENDFCN: u16 = 0x150C;
const LF_NESTTYPEEX: u16 = 0x1512;
const LF_MEMBERMODIFY: u16 = 0x1513;
const LF_MANAGED: u16 = 0x1514;
// The same members with length-prefixed names (`*_ST`).
const LF_ENUMERATE_ST: u16 = 0x0403;
const LF_FRIENDFCN_ST: u16 = 0x1403;
const LF_MEMBER_ST: u16 = 0x1405;
const LF_STMEMBER_ST: u16 = 0x1406;
const LF_METHOD_ST: u16 = 0x1407;
const LF_NESTTYPE_ST: u16 = 0x1408;
const LF_ONEMETHOD_ST: u16 = 0x140B;
const LF_NESTTYPEEX_ST: u16 = 0x140D;
const LF_MEMBERMODIFY_ST: u16 = 0x140E;
const LF_MANAGED_ST: u16 = 0x140F;

/// Skips a member's name: length-prefixed for the `*_ST` members, else
/// zero-terminated.
fn skip_name(r: &mut Reader<'_>, member: u16) -> turbonop_core::Result<()> {
    let st = matches!(
        member,
        LF_ENUMERATE_ST
            | LF_FRIENDFCN_ST
            | LF_MEMBER_ST
            | LF_STMEMBER_ST
            | LF_METHOD_ST
            | LF_NESTTYPE_ST
            | LF_ONEMETHOD_ST
            | LF_NESTTYPEEX_ST
            | LF_MEMBERMODIFY_ST
            | LF_MANAGED_ST
    );
    if st {
        let n = usize::from(r.u8()?);
        r.skip(n)
    } else {
        r.cstr().map(|_| ())
    }
}

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
            leaf::LF_ENUMERATE | LF_ENUMERATE_ST => {
                r.skip(2).map_err(m)?;
                read_numeric(&mut r).map_err(m)?;
                skip_name(&mut r, member).map_err(m)?;
            }
            // attributes(2) type(4) offset(numeric) name
            leaf::LF_MEMBER | LF_MEMBER_ST => {
                r.skip(2).map_err(m)?;
                out.at(at(&r), 1, false)?;
                r.skip(4).map_err(m)?;
                read_numeric(&mut r).map_err(m)?;
                skip_name(&mut r, member).map_err(m)?;
            }
            // attributes(2) type(4) name (pad(2) for friends and nested types)
            leaf::LF_STMEMBER
            | LF_STMEMBER_ST
            | leaf::LF_NESTTYPE
            | LF_NESTTYPE_ST
            | LF_NESTTYPEEX
            | LF_NESTTYPEEX_ST
            | LF_FRIENDFCN
            | LF_FRIENDFCN_ST
            | LF_MEMBERMODIFY
            | LF_MEMBERMODIFY_ST => {
                r.skip(2).map_err(m)?;
                out.at(at(&r), 1, false)?;
                r.skip(4).map_err(m)?;
                skip_name(&mut r, member).map_err(m)?;
            }
            // count(2) method list(4) name
            leaf::LF_METHOD | LF_METHOD_ST => {
                r.skip(2).map_err(m)?;
                out.at(at(&r), 1, false)?;
                r.skip(4).map_err(m)?;
                skip_name(&mut r, member).map_err(m)?;
            }
            // attributes(2) type(4) [vbaseoff(4)] name
            leaf::LF_ONEMETHOD | LF_ONEMETHOD_ST => {
                let attributes = r.u16_le().map_err(m)?;
                out.at(at(&r), 1, false)?;
                r.skip(4).map_err(m)?;
                if introduces_virtual(attributes) {
                    r.skip(4).map_err(m)?;
                }
                skip_name(&mut r, member).map_err(m)?;
            }
            // name
            LF_MANAGED | LF_MANAGED_ST => {
                skip_name(&mut r, member).map_err(m)?;
            }
            // Members before 32-bit type indices.
            0x0400..=0x0402 | 0x0404..=0x040D => return Err(RefError::Legacy(member)),
            _ => return Err(RefError::Unknown(member)),
        }
        debug_assert!(r.position() > start);
    }
    Ok(())
}

#[cfg(test)]
mod tests;

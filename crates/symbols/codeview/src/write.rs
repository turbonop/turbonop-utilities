//! Writing type and symbol records: the length, the kind, the body, and
//! the padding, as compilers emit them.
//!
//! A record is its kind and its body's bytes. Rewriting records (to
//! renumber their indices, say) copies the bytes and patches the fields
//! [`type_refs`](crate::type_refs) and [`symbol_refs`](crate::symbol_refs)
//! locate ([`remap`](crate::remap)), rather than decoding every kind into
//! a structure and encoding it again: every index field is 4 bytes, so a
//! patched record keeps its length, and one table of layouts serves both
//! finding fields and writing them. A second, independent encoder per kind
//! could disagree with that table, which is the error that corrupts debug
//! information silently. New records are built with [`RecordBuilder`] and
//! the constructors below, and written with the same framing.
//!
//! Framing:
//!
//! - **Type records** (`.debug$T`, `.debug$P`, a PDB's TPI and IPI) are
//!   padded to 4 bytes with `LF_PAD3 LF_PAD2 LF_PAD1` (as many of them as
//!   needed, ending in `LF_PAD1`), which the length includes. MSVC and
//!   clang both pad so.
//! - **Symbol records** in an object are as long as their contents: MSVC
//!   doesn't pad them, and clang pads them to 4 bytes with zeros. A PDB's
//!   module streams pad every record to 4. The padding is part of the body
//!   as read, so [`push_symbol`] takes the alignment to pad a new record
//!   to.

use crate::types::leaf;

/// The longest a record's length field can say: its kind, body, and
/// padding.
pub const MAX_RECORD_LEN: usize = 0xFFFF;

/// Why a record could not be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// The record (kind, body, and padding) would be longer than
    /// [`MAX_RECORD_LEN`]: this many bytes.
    TooLong(usize),
}

impl core::fmt::Display for WriteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WriteError::TooLong(n) => write!(f, "a record of {n} bytes is too long"),
        }
    }
}

/// How many `LF_PAD` bytes follow a type record's body of `len` bytes, so
/// that the record (with its length and kind) ends on 4 bytes.
pub fn type_padding(len: usize) -> usize {
    len.wrapping_neg() & 3
}

/// `body` without its trailing `LF_PAD` bytes: the longest run of
/// `LF_PADn ... LF_PAD1` (n at most 3) that ends it, if the record is
/// aligned. [`push_type`] of the result writes `body` again.
pub fn unpadded(body: &[u8]) -> &[u8] {
    if !body.len().is_multiple_of(4) {
        return body;
    }
    for n in (1..=3usize).rev() {
        let Some(tail) = body.len().checked_sub(n).map(|at| &body[at..]) else {
            continue;
        };
        if tail
            .iter()
            .enumerate()
            .all(|(i, &b)| b == leaf::LF_PAD0 as u8 | (n - i) as u8)
        {
            return &body[..body.len() - n];
        }
    }
    body
}

fn frame(out: &mut Vec<u8>, kind: u16, body: &[u8], pad: usize) -> Result<(), WriteError> {
    let len = 2 + body.len() + pad;
    if len > MAX_RECORD_LEN {
        return Err(WriteError::TooLong(len));
    }
    out.reserve(2 + len);
    out.extend_from_slice(&(len as u16).to_le_bytes());
    out.extend_from_slice(&kind.to_le_bytes());
    out.extend_from_slice(body);
    Ok(())
}

/// Appends the type record of `kind` with `body` (unpadded): its length,
/// kind, body, and `LF_PAD` padding.
pub fn push_type(out: &mut Vec<u8>, kind: u16, body: &[u8]) -> Result<(), WriteError> {
    let pad = type_padding(body.len());
    frame(out, kind, body, pad)?;
    for i in (1..=pad).rev() {
        out.push(leaf::LF_PAD0 as u8 | i as u8);
    }
    Ok(())
}

/// Appends the symbol record of `kind` with `body`: its length, kind, and
/// body, then zeros to make the record a multiple of `align` bytes (1 for
/// none, as MSVC's objects; 4 as clang's, and a PDB's).
pub fn push_symbol(
    out: &mut Vec<u8>,
    kind: u16,
    body: &[u8],
    align: usize,
) -> Result<(), WriteError> {
    debug_assert!(align.is_power_of_two());
    let pad = (4 + body.len()).wrapping_neg() & (align - 1);
    frame(out, kind, body, pad)?;
    out.resize(out.len() + pad, 0);
    Ok(())
}

/// Appends `value` as a numeric leaf, in the fewest bytes: itself below
/// `LF_NUMERIC`, else `LF_USHORT`, `LF_ULONG`, or `LF_UQUADWORD` and its
/// bytes (as LLVM encodes; MSVC's records read the same).
pub fn push_unsigned(out: &mut Vec<u8>, value: u64) {
    if value < u64::from(leaf::LF_NUMERIC) {
        out.extend_from_slice(&(value as u16).to_le_bytes());
    } else if let Ok(v) = u16::try_from(value) {
        out.extend_from_slice(&leaf::LF_USHORT.to_le_bytes());
        out.extend_from_slice(&v.to_le_bytes());
    } else if let Ok(v) = u32::try_from(value) {
        out.extend_from_slice(&leaf::LF_ULONG.to_le_bytes());
        out.extend_from_slice(&v.to_le_bytes());
    } else {
        out.extend_from_slice(&leaf::LF_UQUADWORD.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
}

/// Appends `value` as a numeric leaf: as [`push_unsigned`] if it isn't
/// negative, else `LF_CHAR`, `LF_SHORT`, `LF_LONG`, or `LF_QUADWORD`,
/// whichever is the smallest that holds it.
pub fn push_signed(out: &mut Vec<u8>, value: i64) {
    if let Ok(v) = u64::try_from(value) {
        return push_unsigned(out, v);
    }
    if let Ok(v) = i8::try_from(value) {
        out.extend_from_slice(&leaf::LF_CHAR.to_le_bytes());
        out.extend_from_slice(&v.to_le_bytes());
    } else if let Ok(v) = i16::try_from(value) {
        out.extend_from_slice(&leaf::LF_SHORT.to_le_bytes());
        out.extend_from_slice(&v.to_le_bytes());
    } else if let Ok(v) = i32::try_from(value) {
        out.extend_from_slice(&leaf::LF_LONG.to_le_bytes());
        out.extend_from_slice(&v.to_le_bytes());
    } else {
        out.extend_from_slice(&leaf::LF_QUADWORD.to_le_bytes());
        out.extend_from_slice(&value.to_le_bytes());
    }
}

/// A record's body, built field by field, then written as a type or a
/// symbol record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecordBuilder {
    kind: u16,
    body: Vec<u8>,
}

impl RecordBuilder {
    /// An empty record of `kind`.
    pub fn new(kind: u16) -> Self {
        Self {
            kind,
            body: Vec::new(),
        }
    }

    /// The record's kind.
    pub fn kind(&self) -> u16 {
        self.kind
    }

    /// The body so far.
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// Appends a byte.
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.body.push(v);
        self
    }

    /// Appends 2 bytes.
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.body.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Appends 4 bytes (an index, an offset, a count, ...).
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.body.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Appends bytes as they are.
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.body.extend_from_slice(v);
        self
    }

    /// Appends a zero-terminated name.
    pub fn name(&mut self, v: &[u8]) -> &mut Self {
        self.body.extend_from_slice(v);
        self.body.push(0);
        self
    }

    /// Appends an unsigned numeric leaf ([`push_unsigned`]).
    pub fn unsigned(&mut self, v: u64) -> &mut Self {
        push_unsigned(&mut self.body, v);
        self
    }

    /// Appends a signed numeric leaf ([`push_signed`]).
    pub fn signed(&mut self, v: i64) -> &mut Self {
        push_signed(&mut self.body, v);
        self
    }

    /// Appends the record as a type record ([`push_type`]).
    pub fn write_type(&self, out: &mut Vec<u8>) -> Result<(), WriteError> {
        push_type(out, self.kind, &self.body)
    }

    /// Appends the record as a symbol record ([`push_symbol`]).
    pub fn write_symbol(&self, out: &mut Vec<u8>, align: usize) -> Result<(), WriteError> {
        push_symbol(out, self.kind, &self.body, align)
    }
}

/// `LF_PRECOMP` (`lfPreComp`): the types from index `start`, `count` of
/// them, are the precompiled-header object's whose `LF_ENDPRECOMP` and
/// `S_OBJNAME` carry `signature`, at `name`.
pub fn precomp(start: u32, count: u32, signature: u32, name: &[u8]) -> RecordBuilder {
    let mut r = RecordBuilder::new(leaf::LF_PRECOMP);
    r.u32(start).u32(count).u32(signature).name(name);
    r
}

/// `LF_ENDPRECOMP` (`lfEndPreComp`): the end of a precompiled header's
/// shared types, and their `signature`.
pub fn end_precomp(signature: u32) -> RecordBuilder {
    let mut r = RecordBuilder::new(leaf::LF_ENDPRECOMP);
    r.u32(signature);
    r
}

/// `S_OBJNAME` (`OBJNAMESYM`): the object's `signature` (a precompiled
/// header's, or 0) and path.
pub fn obj_name(signature: u32, name: &[u8]) -> RecordBuilder {
    let mut r = RecordBuilder::new(crate::symbols::kind::S_OBJNAME);
    r.u32(signature).name(name);
    r
}

#[cfg(test)]
mod tests;

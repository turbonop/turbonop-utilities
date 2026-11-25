//! Renumbering records in place: their type and id indices, and their
//! string-table and file-checksum offsets.
//!
//! Every index and offset field is 4 bytes, so a renumbered record keeps
//! its length, and a section is renumbered by patching its bytes where
//! [`type_refs`] and [`symbol_refs`] (and [`c13::fields`]) say the fields
//! are. The one exception is a file checksum offset inside an inline
//! site's binary annotations, which is compressed: one whose new value
//! takes the same number of bytes is patched in place, and one that
//! doesn't is an error ([`Error::AnnotationLength`]), for the caller to
//! rewrite the record ([`annotations::rewrite_files`]).
//!
//! Only indices from `0x1000` are renumbered: those below are the built-in
//! (simple) types, the same everywhere.

use crate::annotations;
use crate::c13::{self, CV_SIGNATURE_C13};
use crate::lines::subsection as kind;
use crate::symbol_refs::{self, Field, FieldError};
use crate::type_refs::{self, Ref, RefError};
use crate::types::{TypeIndex, TypeRecord};

/// New numbers for what records refer to.
pub trait Remap {
    /// The new index of `index` (at least `0x1000`): an id if `id`, else a
    /// type. `None` if it has none (an index out of range, say).
    fn index(&mut self, index: u32, id: bool) -> Option<u32>;

    /// The new offset of a string at `offset` in the string table. The
    /// default leaves it.
    fn string(&mut self, offset: u32) -> Option<u32> {
        Some(offset)
    }

    /// The new offset of the file checksum entry at `offset`. The default
    /// leaves it.
    fn checksum(&mut self, offset: u32) -> Option<u32> {
        Some(offset)
    }
}

/// A function of (index, whether it is an id) renumbers indices only.
impl<F: FnMut(u32, bool) -> Option<u32>> Remap for F {
    fn index(&mut self, index: u32, id: bool) -> Option<u32> {
        self(index, id)
    }
}

/// Why records could not be renumbered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A type record's index fields (the record at `at`).
    Type {
        /// Where the record starts.
        at: usize,
        /// What was wrong.
        error: RefError,
    },
    /// A symbol record's fields (the record at `at`).
    Symbol {
        /// Where the record starts.
        at: usize,
        /// What was wrong.
        error: FieldError,
    },
    /// The subsections, or a subsection's contents.
    C13(c13::Error),
    /// Records that end early, at `at`.
    Truncated {
        /// Where.
        at: usize,
    },
    /// A field at `at` holds `value`, which [`Remap`] has no new number for.
    Unmapped {
        /// Where the field is.
        at: usize,
        /// What it holds.
        value: u32,
        /// What it is.
        field: FieldKind,
    },
    /// The binary annotations at `at` change to a file whose new offset
    /// takes a different number of bytes compressed: the record must be
    /// rewritten, not patched.
    AnnotationLength {
        /// Where the operand is.
        at: usize,
    },
}

/// What a field [`Error::Unmapped`] names is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// A type index.
    Type,
    /// An id index.
    Id,
    /// A string-table offset.
    String,
    /// A file checksum offset.
    Checksum,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Type { at, error } => write!(f, "type record at {at:#x}: {error}"),
            Error::Symbol { at, error } => write!(f, "symbol record at {at:#x}: {error}"),
            Error::C13(e) => e.fmt(f),
            Error::Truncated { at } => write!(f, "records truncated at {at:#x}"),
            Error::Unmapped { at, value, field } => {
                write!(f, "no new number for {field:?} {value:#x} (at {at:#x})")
            }
            Error::AnnotationLength { at } => {
                write!(f, "a file in annotations at {at:#x} changes length")
            }
        }
    }
}

impl From<c13::Error> for Error {
    fn from(e: c13::Error) -> Self {
        Error::C13(e)
    }
}

fn read(buf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([buf[at], buf[at + 1], buf[at + 2], buf[at + 3]])
}

fn write(buf: &mut [u8], at: usize, v: u32) {
    buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Renumbers the index at `at` in `buf`, if it isn't a built-in type.
fn patch_index(buf: &mut [u8], at: usize, id: bool, remap: &mut impl Remap) -> Result<(), Error> {
    let v = read(buf, at);
    if v < TypeIndex::FIRST_NON_SIMPLE {
        return Ok(());
    }
    let new = remap.index(v, id).ok_or(Error::Unmapped {
        at,
        value: v,
        field: if id { FieldKind::Id } else { FieldKind::Type },
    })?;
    write(buf, at, new);
    Ok(())
}

/// Renumbers the string or checksum offset at `at` in `buf`.
fn patch_offset(
    buf: &mut [u8],
    at: usize,
    field: FieldKind,
    remap: &mut impl Remap,
) -> Result<(), Error> {
    let v = read(buf, at);
    let new = match field {
        FieldKind::String => remap.string(v),
        _ => remap.checksum(v),
    }
    .ok_or(Error::Unmapped {
        at,
        value: v,
        field,
    })?;
    write(buf, at, new);
    Ok(())
}

/// Renumbers the file checksum offsets in the binary annotations at `at`
/// (to `end`) of `buf`.
fn patch_annotations(
    buf: &mut [u8],
    at: usize,
    end: usize,
    remap: &mut impl Remap,
) -> Result<(), Error> {
    let Some(files) = annotations::file_operands(&buf[at..end]) else {
        // (Annotations that don't decode name no files this can find; the
        // reader that decodes them stops at the same place.)
        return Ok(());
    };
    for (pos, len, v) in files {
        let place = at + pos;
        let new = remap.checksum(v).ok_or(Error::Unmapped {
            at: place,
            value: v,
            field: FieldKind::Checksum,
        })?;
        if new == v {
            continue;
        }
        let mut bytes = Vec::with_capacity(4);
        if annotations::compress(new, &mut bytes).is_none() || bytes.len() != len {
            return Err(Error::AnnotationLength { at: place });
        }
        buf[place..place + len].copy_from_slice(&bytes);
    }
    Ok(())
}

/// Renumbers the fields of the symbol record whose body (after its kind)
/// is `body`, of kind `kind`. `scratch` is reused between calls.
pub fn symbol_record(
    kind: u16,
    body: &mut [u8],
    scratch: &mut Vec<Field>,
    remap: &mut impl Remap,
) -> Result<(), Error> {
    scratch.clear();
    symbol_refs::fields(kind, body, |f| scratch.push(f))
        .map_err(|error| Error::Symbol { at: 0, error })?;
    patch_fields(body, scratch, body.len(), remap)
}

/// Patches each of `fields` in `buf`; annotations run to `end`.
fn patch_fields(
    buf: &mut [u8],
    fields: &[Field],
    end: usize,
    remap: &mut impl Remap,
) -> Result<(), Error> {
    for &f in fields {
        match f {
            Field::Type(at) => patch_index(buf, at as usize, false, remap)?,
            Field::Id(at) => patch_index(buf, at as usize, true, remap)?,
            Field::String(at) => patch_offset(buf, at as usize, FieldKind::String, remap)?,
            Field::Checksum(at) => patch_offset(buf, at as usize, FieldKind::Checksum, remap)?,
            Field::Annotations(at) => patch_annotations(buf, at as usize, end, remap)?,
            Field::Address { .. } | Field::Link(_) | Field::Token(_) | Field::Rva(_) => {}
        }
    }
    Ok(())
}

/// Renumbers the index fields of the type record whose body (after its
/// kind) is `body`, of kind `kind`. `scratch` is reused between calls.
pub fn type_record(
    kind: u16,
    body: &mut [u8],
    scratch: &mut Vec<Ref>,
    remap: &mut impl Remap,
) -> Result<(), Error> {
    scratch.clear();
    type_refs::refs(kind, body, scratch).map_err(|error| Error::Type { at: 0, error })?;
    for r in scratch.iter() {
        patch_index(body, r.offset as usize, r.id, remap)?;
    }
    Ok(())
}

/// The type records of `stream` (length-prefixed records: a `.debug$T`
/// after its signature, a TPI stream's records), each at its offset;
/// `Err` with the offset of a record that runs past the end.
pub fn type_records(stream: &[u8]) -> impl Iterator<Item = Result<(usize, TypeRecord<'_>), usize>> {
    let mut at = 0;
    std::iter::from_fn(move || {
        if at >= stream.len() {
            return None;
        }
        let start = at;
        let Some(len) = stream
            .get(at..at + 2)
            .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
            .filter(|&l| l >= 2)
        else {
            at = stream.len();
            return Some(Err(start));
        };
        let Some(body) = stream.get(at + 2..at + 2 + len) else {
            at = stream.len();
            return Some(Err(start));
        };
        at += 2 + len;
        Some(Ok((
            start,
            TypeRecord {
                kind: u16::from_le_bytes([body[0], body[1]]),
                data: &body[2..],
            },
        )))
    })
}

/// Renumbers every type record in `stream` (see [`type_records`]).
pub fn type_stream(stream: &mut [u8], remap: &mut impl Remap) -> Result<(), Error> {
    let mut scratch = Vec::new();
    let mut at = 0;
    while at < stream.len() {
        let len = stream
            .get(at..at + 2)
            .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
            .filter(|&l| l >= 2)
            .ok_or(Error::Truncated { at })?;
        let end = at + 2 + len;
        let record = stream.get_mut(at + 2..end).ok_or(Error::Truncated { at })?;
        let kind = u16::from_le_bytes([record[0], record[1]]);
        let body = &mut record[2..];
        scratch.clear();
        type_refs::refs(kind, body, &mut scratch).map_err(|error| Error::Type { at, error })?;
        for r in &scratch {
            patch_index(body, r.offset as usize, r.id, remap).map_err(|e| shift(e, at + 4))?;
        }
        at = end;
    }
    Ok(())
}

/// An error's position moved `by` bytes later.
fn shift(e: Error, by: usize) -> Error {
    match e {
        Error::Unmapped { at, value, field } => Error::Unmapped {
            at: at + by,
            value,
            field,
        },
        Error::AnnotationLength { at } => Error::AnnotationLength { at: at + by },
        Error::Truncated { at } => Error::Truncated { at: at + by },
        Error::Type { at, error } => Error::Type { at: at + by, error },
        Error::Symbol { at, error } => Error::Symbol { at: at + by, error },
        e => e,
    }
}

/// Renumbers a `.debug$T` or `.debug$P` section (its signature first).
pub fn debug_t(section: &mut [u8], remap: &mut impl Remap) -> Result<(), Error> {
    check_signature(section)?;
    type_stream(&mut section[4..], remap).map_err(|e| shift(e, 4))
}

fn check_signature(section: &[u8]) -> Result<(), Error> {
    if section.get(..4) != Some(&CV_SIGNATURE_C13.to_le_bytes()[..]) {
        return Err(Error::C13(c13::Error::Malformed {
            kind: u32::MAX,
            at: 0,
        }));
    }
    Ok(())
}

/// Renumbers C13 subsections in place (a `.debug$S` after its signature,
/// or a PDB module stream's C13 part): every field [`c13::fields`] finds,
/// in symbols, lines, file checksums, inlinee lines, frame data, and
/// cross-scope imports and exports.
pub fn subsections(c13: &mut [u8], remap: &mut impl Remap) -> Result<(), Error> {
    let mut fields = Vec::new();
    let mut at = 0;
    while at < c13.len() {
        let header = c13.get(at..at + 8).ok_or(Error::Truncated { at })?;
        let kind = read(header, 0);
        let len = read(header, 4) as usize;
        let start = at + 8;
        let end = start.checked_add(len).ok_or(Error::Truncated { at })?;
        let data = c13.get_mut(start..end).ok_or(Error::Truncated { at })?;
        match kind {
            kind::DEBUG_S_SYMBOLS => {
                symbols(data, &mut fields, remap).map_err(|e| shift(e, start))?;
            }
            kind::DEBUG_S_LINES
            | kind::DEBUG_S_IL_LINES
            | kind::DEBUG_S_FILECHKSMS
            | kind::DEBUG_S_INLINEELINES
            | kind::DEBUG_S_FRAMEDATA
            | kind::DEBUG_S_CROSSSCOPEIMPORTS
            | kind::DEBUG_S_CROSSSCOPEEXPORTS => {
                fields.clear();
                c13::fields(kind, data, |f| fields.push(f))?;
                let n = data.len();
                patch_fields(data, &fields, n, remap).map_err(|e| shift(e, start))?;
            }
            _ => {}
        }
        at = end + (len.wrapping_neg() & 3);
    }
    Ok(())
}

/// Renumbers a symbols subsection's records.
fn symbols(data: &mut [u8], fields: &mut Vec<Field>, remap: &mut impl Remap) -> Result<(), Error> {
    let mut at = 0;
    while at < data.len() {
        let len = data
            .get(at..at + 2)
            .map(|b| usize::from(u16::from_le_bytes([b[0], b[1]])))
            .filter(|&l| l >= 2)
            .ok_or(Error::Truncated { at })?;
        let end = at + 2 + len;
        let record = data.get_mut(at + 2..end).ok_or(Error::Truncated { at })?;
        let kind = u16::from_le_bytes([record[0], record[1]]);
        let body = &mut record[2..];
        fields.clear();
        symbol_refs::fields(kind, body, |f| fields.push(f))
            .map_err(|error| Error::Symbol { at, error })?;
        let n = body.len();
        patch_fields(body, fields, n, remap).map_err(|e| shift(e, at + 4))?;
        at = end;
    }
    Ok(())
}

/// Renumbers a `.debug$S` section (its signature first).
pub fn debug_s(section: &mut [u8], remap: &mut impl Remap) -> Result<(), Error> {
    check_signature(section)?;
    subsections(&mut section[4..], remap).map_err(|e| shift(e, 4))
}

#[cfg(test)]
mod tests;

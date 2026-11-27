//! Auxiliary records: which form a symbol's records take, and their
//! encoding.
//!
//! The layouts (the first 18 bytes; a big-obj record has 2 more, unused):
//!
//! | Form | Fields |
//! |---|---|
//! | Function definition | `TagIndex` 0, `TotalSize` 4, `PointerToLinenumber` 8, `PointerToNextFunction` 12 |
//! | `.bf` / `.ef` | `Linenumber` 4, `PointerToNextFunction` 12 |
//! | Weak external | `TagIndex` 0, `Characteristics` 4 |
//! | File | the name, across all the records |
//! | Section definition | `Length` 0, `NumberOfRelocations` 4, `NumberOfLinenumbers` 6, `CheckSum` 8, `Number` 12, `Selection` 14, `HighNumber` 16 (big-obj only) |
//!
//! (A section definition keeps its unused bytes, which a tool has been seen
//! to fill: see [`SectionDefinition::unused`].)
//! | CLR token | `bAuxType` 0, `SymbolTableIndex` 2 |
//!
//! Every other byte is unused, and a record is read in its typed form only
//! if those bytes are zero.

use alloc::borrow::Cow;

use crate::object::{
    Aux, BeginEnd, ClrToken, Format, FunctionDefinition, LineNumberRef, SectionDefinition,
    SectionId, SectionNumber, SymbolId, WeakExternal, storage_class, symbol_type,
};

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn zero(b: &[u8]) -> bool {
    b.iter().all(|&x| x == 0)
}

/// What a symbol's primary record says about the form of its auxiliary
/// records.
pub(crate) struct Primary {
    pub value: u32,
    pub section: SectionNumber,
    pub kind: u16,
    pub storage_class: u8,
}

/// How the reader turns the file's references into the model's.
pub(crate) trait Resolve {
    /// The symbol whose primary record is at table index `index`.
    fn symbol(&self, index: u32) -> Option<SymbolId>;
    /// The section with 1-based number `number`.
    fn section(&self, number: u32) -> Option<SectionId>;
    /// The line number record at file offset `offset`.
    fn line_number(&self, offset: u32) -> Option<LineNumberRef>;
}

/// The typed form of a symbol's auxiliary records `aux` (`count` records
/// of `format`'s size), or [`Aux::Raw`].
pub(crate) fn decode<'data>(
    primary: &Primary,
    aux: &'data [u8],
    count: usize,
    format: Format,
    resolve: &impl Resolve,
) -> Aux<'data> {
    if count == 0 {
        return Aux::None;
    }
    decode_typed(primary, aux, count, format, resolve).unwrap_or(Aux::Raw(Cow::Borrowed(aux)))
}

fn decode_typed<'data>(
    p: &Primary,
    aux: &'data [u8],
    count: usize,
    format: Format,
    resolve: &impl Resolve,
) -> Option<Aux<'data>> {
    use storage_class as sc;
    let stride = format.record_size();
    if p.storage_class == sc::FILE {
        let len = aux.iter().position(|&b| b == 0).unwrap_or(aux.len());
        let name = &aux[..len];
        let extra_records = u8::try_from(count - len.div_ceil(stride)).ok()?;
        return (len > 0 && zero(&aux[len..])).then_some(Aux::File {
            name: Cow::Borrowed(name),
            extra_records,
        });
    }
    if count != 1 {
        return None;
    }
    let a = aux;
    let tail_zero = zero(&a[16..]);
    let optional = |index: u32| match index {
        0 => Some(None),
        i => resolve.symbol(i).map(Some),
    };
    match p.storage_class {
        sc::FUNCTION => {
            let ok = zero(&a[..4]) && zero(&a[6..12]) && tail_zero;
            ok.then_some(())?;
            Some(Aux::BeginEnd(BeginEnd {
                line: u16_at(a, 4),
                next: optional(u32_at(a, 12))?,
            }))
        }
        sc::CLR_TOKEN => {
            (a[1] == 0 && zero(&a[6..])).then_some(())?;
            Some(Aux::ClrToken(ClrToken {
                aux_type: a[0],
                symbol: resolve.symbol(u32_at(a, 2))?,
            }))
        }
        _ if is_weak_external(p) => {
            zero(&a[8..]).then_some(())?;
            Some(Aux::WeakExternal(WeakExternal {
                tag: resolve.symbol(u32_at(a, 0))?,
                search: u32_at(a, 4),
            }))
        }
        _ if is_function_definition(p) => {
            tail_zero.then_some(())?;
            let line_numbers = match u32_at(a, 8) {
                0 => None,
                offset => Some(resolve.line_number(offset)?),
            };
            Some(Aux::Function(FunctionDefinition {
                tag: optional(u32_at(a, 0))?,
                total_size: u32_at(a, 4),
                line_numbers,
                next: optional(u32_at(a, 12))?,
            }))
        }
        _ if is_section_definition(p) => {
            let (high, unused) = match format {
                Format::Regular => (0, [a[15], a[16], a[17]]),
                Format::BigObj => (u32::from(u16_at(a, 16)), [a[15], a[18], a[19]]),
            };
            let number = match u32::from(u16_at(a, 12)) | (high << 16) {
                0 => None,
                n => Some(resolve.section(n)?),
            };
            Some(Aux::Section(SectionDefinition {
                length: u32_at(a, 0),
                relocations: u16_at(a, 4),
                line_numbers: u16_at(a, 6),
                checksum: u32_at(a, 8),
                number,
                selection: a[14],
                unused,
            }))
        }
        _ => None,
    }
}

/// A weak external: storage class `WEAK_EXTERNAL`, or (the form the PE
/// specification gives) an undefined external of value 0 with an
/// auxiliary record.
fn is_weak_external(p: &Primary) -> bool {
    p.storage_class == storage_class::WEAK_EXTERNAL
        || (p.storage_class == storage_class::EXTERNAL
            && p.section == SectionNumber::Undefined
            && p.value == 0)
}

/// A function definition: a function defined in a section, external (as
/// the PE specification has it) or a label (as `armasm64` writes for some).
fn is_function_definition(p: &Primary) -> bool {
    matches!(
        p.storage_class,
        storage_class::EXTERNAL | storage_class::LABEL
    ) && p.kind & symbol_type::COMPLEX_MASK == symbol_type::FUNCTION
        && matches!(p.section, SectionNumber::Section(_))
}

/// A section definition: a static symbol with an auxiliary record, or (as
/// C++/CLI writes for an appdomain global) an absolute external.
fn is_section_definition(p: &Primary) -> bool {
    p.storage_class == storage_class::STATIC
        || (p.storage_class == storage_class::EXTERNAL && p.section == SectionNumber::Absolute)
}

/// How the writer turns the model's references into the file's.
pub(crate) trait Place {
    /// The table index of symbol `id`.
    fn symbol(&self, id: SymbolId) -> u32;
    /// The file offset of a line number record.
    fn line_number(&self, at: LineNumberRef) -> u32;
}

/// Writes `aux` into `out` (exactly its records' bytes, zeroed).
pub(crate) fn encode(aux: &Aux<'_>, format: Format, place: &impl Place, out: &mut [u8]) {
    let put16 = |out: &mut [u8], o: usize, v: u16| out[o..o + 2].copy_from_slice(&v.to_le_bytes());
    let put32 = |out: &mut [u8], o: usize, v: u32| out[o..o + 4].copy_from_slice(&v.to_le_bytes());
    let optional = |id: Option<SymbolId>| id.map_or(0, |id| place.symbol(id));
    match aux {
        Aux::None => {}
        Aux::Function(f) => {
            put32(out, 0, optional(f.tag));
            put32(out, 4, f.total_size);
            put32(out, 8, f.line_numbers.map_or(0, |at| place.line_number(at)));
            put32(out, 12, optional(f.next));
        }
        Aux::BeginEnd(b) => {
            put16(out, 4, b.line);
            put32(out, 12, optional(b.next));
        }
        Aux::WeakExternal(w) => {
            put32(out, 0, place.symbol(w.tag));
            put32(out, 4, w.search);
        }
        Aux::File { name, .. } => out[..name.len()].copy_from_slice(name),
        Aux::Section(s) => {
            let number = s.number.map_or(0, SectionId::number);
            put32(out, 0, s.length);
            put16(out, 4, s.relocations);
            put16(out, 6, s.line_numbers);
            put32(out, 8, s.checksum);
            // The writer has checked that the number fits the format.
            put16(out, 12, (number & 0xFFFF) as u16);
            out[14] = s.selection;
            out[15] = s.unused[0];
            match format {
                Format::Regular => out[16..18].copy_from_slice(&s.unused[1..]),
                Format::BigObj => {
                    put16(out, 16, (number >> 16) as u16);
                    out[18..20].copy_from_slice(&s.unused[1..]);
                }
            }
        }
        Aux::ClrToken(c) => {
            out[0] = c.aux_type;
            put32(out, 2, place.symbol(c.symbol));
        }
        Aux::Raw(bytes) => out.copy_from_slice(bytes),
    }
}

#[cfg(test)]
mod tests;

//! The WebAssembly backend.
//!
//! A module's code has no addresses of its own. Its tools (DWARF, source
//! maps, symbolizers) address code by offset into the code section's
//! contents, so here the code section has an extent in memory: at address
//! 0, of its contents' size. Linear memory, where data segments go, is
//! another address space, which DWARF's variables' addresses are in: here
//! it is at [`MEMORY`], above every code offset, and each active data
//! segment of a linked module is a section there, named as the `name`
//! section names it (wasm-ld's `.rodata`, `.data`, `.tdata`).

use alloc::vec::Vec;

use hexcavator_wasm::{
    Init, Module, NAME_DATA_SEGMENT, SECTION_CODE, SECTION_CUSTOM, SECTION_DATA, SegmentOffset,
};

/// Where a module's linear memory is in its addresses: the data at memory
/// address `a` is at `MEMORY + a`. (A code section is smaller than 4 GiB:
/// its offsets are all below.)
pub const MEMORY: u64 = 1 << 32;

/// An active data segment, where it goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Placed {
    /// Its index in the data section.
    pub index: u32,
    /// Its address (in linear memory, at [`MEMORY`]).
    pub addr: u64,
    /// Where its contents are in the file, and their size.
    pub file_offset: u64,
    pub size: u64,
}

/// The module's active data segments whose addresses are constant (an
/// `i32.const` or `i64.const`, or an immutable global initialized to one),
/// in segment order. (A passive segment goes where code copies it, and a
/// position-independent module's are relative to its imported
/// `__memory_base`: neither is placed.)
pub(crate) fn placed_segments(m: &Module<'_>) -> Vec<Placed> {
    let mut out = Vec::new();
    if m.is_object() {
        return out;
    }
    let globals = m.globals();
    let pieces = m.data_segments();
    for (piece, d) in pieces.iter().zip(m.data_segment_details()) {
        let at = match d.offset {
            SegmentOffset::I32(v) => u64::from(v.cast_unsigned()),
            SegmentOffset::I64(v) => v.cast_unsigned(),
            SegmentOffset::Global(g) => match globals.iter().find(|x| x.index == g) {
                Some(x) if !x.mutable => match x.init {
                    Init::I32(v) => u64::from(v.cast_unsigned()),
                    Init::I64(v) => v.cast_unsigned(),
                    _ => continue,
                },
                _ => continue,
            },
            SegmentOffset::Other | SegmentOffset::Passive => continue,
        };
        // (Its contents end the segment's entry.)
        let Some(file_offset) = (piece.offset + piece.size).checked_sub(d.size) else {
            continue;
        };
        out.push(Placed {
            index: piece.index,
            addr: MEMORY.wrapping_add(at),
            file_offset,
            size: d.size,
        });
    }
    out
}

use crate::{DebugId, Kind, Section, SectionFlags};

pub(crate) fn kind(m: &Module<'_>) -> Kind {
    if m.is_object() {
        Kind::Object
    } else {
        Kind::Linked
    }
}

pub(crate) fn flags(id: u8) -> SectionFlags {
    SectionFlags(match id {
        SECTION_CODE => SectionFlags::READ | SectionFlags::EXECUTE | SectionFlags::CODE,
        SECTION_DATA => SectionFlags::READ | SectionFlags::WRITE | SectionFlags::DATA,
        // Custom sections (names, DWARF, producers, linking) are read by
        // tools, not by the engine.
        SECTION_CUSTOM => SectionFlags::NOT_LOADED,
        // Types, imports, functions, tables, memories, globals, exports,
        // the start function, elements, tags: what the engine reads.
        _ => SectionFlags::READ,
    })
}

pub(crate) fn sections<'data>(m: &Module<'data>) -> Vec<Section<'data>> {
    let mut out: Vec<Section<'data>> = m
        .sections()
        .iter()
        .enumerate()
        .map(|(index, s)| {
            // (A custom section's data follows its name.)
            let name = s.data_offset - s.contents_offset;
            Section {
                index,
                name: s.name,
                addr: 0,
                vm_size: if s.id == SECTION_CODE {
                    s.contents_size
                } else {
                    0
                },
                file_offset: s.data_offset,
                file_size: s.contents_size - name,
                flags: flags(s.id),
                raw_flags: u32::from(s.id),
            }
        })
        .collect();
    // The data segments, in linear memory.
    let names = m.names(NAME_DATA_SEGMENT);
    let first = out.len();
    for (i, p) in placed_segments(m).into_iter().enumerate() {
        let name: &'data [u8] = names
            .iter()
            .find(|n| n.index == p.index)
            .map_or(b"data", |n| n.name);
        // (wasm-ld names read-only data `.rodata`; the rest is writable.)
        let mut f = SectionFlags::READ | SectionFlags::DATA;
        if !name.starts_with(b".rodata") {
            f |= SectionFlags::WRITE;
        }
        out.push(Section {
            index: first + i,
            name,
            addr: p.addr,
            vm_size: p.size,
            file_offset: p.file_offset,
            file_size: p.size,
            flags: SectionFlags(f),
            raw_flags: u32::from(SECTION_DATA),
        });
    }
    out
}

pub(crate) fn debug_ids<'data>(m: &Module<'data>) -> Vec<DebugId<'data>> {
    let mut out = Vec::new();
    if let Some(id) = m.build_id() {
        out.push(DebugId::BuildId(id));
    }
    if let Some(path) = m.external_debug_info() {
        out.push(DebugId::DebugLink { path, crc: None });
    }
    out
}

/// The address of the module's function `index`, if it defines it: its
/// body's offset into the code section's contents (after the entry's size
/// field), as DWARF's addresses are.
pub(crate) fn function_address(m: &Module<'_>, index: u32) -> Option<u64> {
    let f = m.functions().into_iter().find(|f| f.index == index)?;
    Some(function_range(m, &f)?.0)
}

/// A code section entry's body, as `(address, size)` (see
/// [`function_address`]).
pub(crate) fn function_range(m: &Module<'_>, f: &hexcavator_wasm::Piece) -> Option<(u64, u64)> {
    let code = m.sections().iter().find(|s| s.id == SECTION_CODE)?;
    let (at, body) = m.body(f)?;
    Some((at.checked_sub(code.contents_offset)?, body.len() as u64))
}

/// Bytes of the code section, by offset into its contents; or of a data
/// segment, by its address in linear memory (see [`MEMORY`]).
pub(crate) fn bytes_at<'data>(m: &Module<'data>, addr: u64, len: u64) -> Option<&'data [u8]> {
    if m.is_object() {
        return None;
    }
    if addr >= MEMORY {
        let p = placed_segments(m)
            .into_iter()
            .find(|p| addr >= p.addr && addr - p.addr < p.size)?;
        if len > p.size - (addr - p.addr) {
            return None;
        }
        let start = usize::try_from(p.file_offset + (addr - p.addr)).ok()?;
        return m
            .data()
            .get(start..start.checked_add(usize::try_from(len).ok()?)?);
    }
    let s = m.sections().iter().find(|s| s.id == SECTION_CODE)?;
    if addr >= s.contents_size || len > s.contents_size - addr {
        return None;
    }
    let start = usize::try_from(s.contents_offset.checked_add(addr)?).ok()?;
    m.data()
        .get(start..start.checked_add(usize::try_from(len).ok()?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_flags() {
        assert!(flags(SECTION_CODE).contains(SectionFlags::CODE | SectionFlags::EXECUTE));
        assert!(flags(SECTION_DATA).contains(SectionFlags::DATA | SectionFlags::WRITE));
        assert!(flags(SECTION_CUSTOM).contains(SectionFlags::NOT_LOADED));
        assert_eq!(
            flags(hexcavator_wasm::SECTION_TYPE),
            SectionFlags(SectionFlags::READ)
        );
    }
}

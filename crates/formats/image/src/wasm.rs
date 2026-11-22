//! The WebAssembly backend.
//!
//! A module's code has no addresses of its own. Its tools (DWARF, source
//! maps, symbolizers) address code by offset into the code section's
//! contents, so here the code section has an extent in memory: at address
//! 0, of its contents' size. Linear memory, where data segments go, is
//! another address space, which DWARF's variables' addresses are in: here
//! it is at [`MEMORY`], above every code offset, and each data segment of
//! a linked module is a section there, named as the `name` section names
//! it (wasm-ld's `.rodata`, `.data`, `.tdata`): an active one at its
//! offset, a passive one (shared memory's) where `__wasm_init_memory`
//! copies it; and what that function zeroes (`.bss`, which no segment
//! holds) is a section too. A position-independent module's are placed
//! as if its `__memory_base` were 0.

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

/// The module's data segments whose addresses are constant, in segment
/// order: an active one's (an `i32.const` or `i64.const`, an immutable
/// global initialized to one, or an imported global, `__memory_base`,
/// taken as 0), and a passive one's where `__wasm_init_memory` copies it
/// (see [`init_memory`]). (An object's are active, each at its offset in
/// the object's own layout of its data, which its symbols' offsets and
/// its DWARF's relocated addresses are in.)
pub(crate) fn placed_segments<'m>(m: &'m Module<'_>) -> alloc::borrow::Cow<'m, [Placed]> {
    match m.derived(|| Derived::of(m)) {
        Some(d) => alloc::borrow::Cow::Borrowed(&d.placed),
        None => alloc::borrow::Cow::Owned(placed_segments_of(m)),
    }
}

/// What reading a module needs often (its data's places, which reading
/// any datum needs; `__wasm_init_memory`'s copies and fills), made once.
struct Derived {
    placed: Vec<Placed>,
    init: InitMemory,
}

impl Derived {
    fn of(m: &Module<'_>) -> Self {
        // (Not through `init_memory`: that would wait on this.)
        let init = init_memory_of(m);
        Self {
            placed: placed_segments_with(m, &init),
            init,
        }
    }
}

fn placed_segments_of(m: &Module<'_>) -> Vec<Placed> {
    let init = init_memory_of(m);
    placed_segments_with(m, &init)
}

/// The placed segments, with `init`, the module's `init_memory`.
fn placed_segments_with(m: &Module<'_>, init: &InitMemory) -> Vec<Placed> {
    let mut out = Vec::new();
    let globals = m.globals();
    let imported = m.imported_globals();
    let pieces = m.data_segments();
    let details = m.data_segment_details();
    let copies: &[(u32, u64)] = if details.iter().any(|d| d.offset == SegmentOffset::Passive) {
        &init.copies
    } else {
        &[]
    };
    for (piece, d) in pieces.iter().zip(details) {
        let at = match d.offset {
            SegmentOffset::I32(v) => u64::from(v.cast_unsigned()),
            SegmentOffset::I64(v) => v.cast_unsigned(),
            SegmentOffset::Global(g) if g < imported => 0,
            SegmentOffset::Global(g) => match globals.iter().find(|x| x.index == g) {
                Some(x) if !x.mutable => match x.init {
                    Init::I32(v) => u64::from(v.cast_unsigned()),
                    Init::I64(v) => v.cast_unsigned(),
                    _ => continue,
                },
                _ => continue,
            },
            SegmentOffset::Passive => match copies.iter().find(|c| c.0 == piece.index) {
                Some(&(_, at)) => at,
                None => continue,
            },
            SegmentOffset::Other => continue,
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

/// What `__wasm_init_memory` does (see [`init_memory`]).
#[derive(Clone)]
pub(crate) struct InitMemory {
    /// Each `memory.init`: its segment, and where it copies it.
    pub copies: Vec<(u32, u64)>,
    /// Each `memory.fill`: its destination and size.
    pub fills: Vec<(u64, u64)>,
}

/// What a module's `__wasm_init_memory` (wasm-ld's, for shared memory or
/// a position-independent module) does, where its operands are constant
/// (`i32.const`s and `i64.const`s, an imported global, `__memory_base`,
/// as 0, and their sums): each `memory.init`'s segment and where it copies
/// it (less its offset into the segment), and each `memory.fill`'s
/// destination and size (the zeroed `.bss`). The function is read as
/// wasm-ld writes it (see [`InitCode`]): an instruction it doesn't write
/// ends the reading, with what was found.
pub(crate) fn init_memory(m: &Module<'_>) -> InitMemory {
    match m.derived(|| Derived::of(m)) {
        Some(d) => d.init.clone(),
        None => init_memory_of(m),
    }
}

fn init_memory_of(m: &Module<'_>) -> InitMemory {
    let mut copies = Vec::new();
    let mut fills = Vec::new();
    let names = m.names(hexcavator_wasm::NAME_FUNCTION);
    let Some(f) = names
        .iter()
        .find(|n| n.name == b"__wasm_init_memory")
        .and_then(|n| m.functions().into_iter().find(|f| f.index == n.index))
    else {
        return InitMemory { copies, fills };
    };
    let Some((_, body)) = m.body(&f) else {
        return InitMemory { copies, fills };
    };
    let imported = m.imported_globals();
    let mut code = InitCode { data: body, at: 0 };
    // (Its locals: groups of a count and a type.)
    let Some(groups) = code.uleb() else {
        return InitMemory { copies, fills };
    };
    for _ in 0..groups.min(16) {
        if code.uleb().is_none() || code.byte().is_none() {
            return InitMemory { copies, fills };
        }
    }
    // The operands' values, where constant.
    let mut stack: Vec<Option<i64>> = Vec::new();
    let top3 = |stack: &[Option<i64>]| -> Option<(i64, i64, i64)> {
        let n = stack.len();
        Some((
            stack.get(n.checked_sub(3)?)?.as_ref().copied()?,
            stack[n - 2]?,
            stack[n - 1]?,
        ))
    };
    while let Some(op) = code.byte() {
        let ok = match op {
            // i32.const, i64.const
            0x41 | 0x42 => code.sleb().map(|v| stack.push(Some(v))),
            // global.get: an imported one is the memory base (0).
            0x23 => code
                .uleb()
                .map(|g| stack.push((g < u64::from(imported)).then_some(0))),
            // local.get
            0x20 => code.uleb().map(|_| stack.push(None)),
            // local.set, global.set
            0x21 | 0x24 => code.uleb().map(|_| {
                stack.pop();
            }),
            // local.tee (its value stays)
            0x22 => code.uleb().map(|_| ()),
            // i32.add, i64.add
            0x6A | 0x7C => {
                let (b, a) = (stack.pop().flatten(), stack.pop().flatten());
                stack.push(a.zip(b).map(|(a, b)| a.wrapping_add(b)));
                Some(())
            }
            // block (no result), end, drop, unreachable
            0x02 => code.byte().map(|_| stack.clear()),
            0x0B | 0x1A | 0x00 => {
                stack.clear();
                Some(())
            }
            // br, call
            0x0C | 0x10 => code.uleb().map(|_| stack.clear()),
            // br_table: its labels, then its default
            0x0E => code.uleb().and_then(|n| {
                for _ in 0..=n.min(1 << 16) {
                    code.uleb()?;
                }
                stack.clear();
                Some(())
            }),
            // The atomics (a memory argument each: wasm-ld's cmpxchg,
            // store, notify, and wait).
            0xFE => code
                .uleb()
                .and_then(|_| code.uleb())
                .and_then(|_| code.uleb())
                .map(|_| stack.clear()),
            // The bulk memory instructions.
            0xFC => match code.uleb() {
                // memory.init: a segment, a memory
                Some(8) => code.uleb().and_then(|segment| {
                    code.uleb()?;
                    if let Some((dst, src, _)) = top3(&stack) {
                        copies.push((
                            u32::try_from(segment).ok()?,
                            dst.wrapping_sub(src).cast_unsigned(),
                        ));
                    }
                    stack.clear();
                    Some(())
                }),
                // data.drop: a segment
                Some(9) => code.uleb().map(|_| stack.clear()),
                // memory.fill: a memory
                Some(11) => code.uleb().map(|_| {
                    if let Some((dst, _, size)) = top3(&stack) {
                        fills.push((dst.cast_unsigned(), size.cast_unsigned()));
                    }
                    stack.clear();
                }),
                _ => None,
            },
            _ => None,
        };
        if ok.is_none() {
            break;
        }
    }
    InitMemory { copies, fills }
}

/// A function body's bytes, read as instructions' operands are encoded.
struct InitCode<'d> {
    data: &'d [u8],
    at: usize,
}

impl InitCode<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.data.get(self.at)?;
        self.at += 1;
        Some(b)
    }

    fn uleb(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.byte()?;
            v |= u64::from(b & 0x7F) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    fn sleb(&mut self) -> Option<i64> {
        let mut v = 0i64;
        let mut shift = 0;
        loop {
            let b = self.byte()?;
            if shift < 64 {
                v |= i64::from(b & 0x7F) << shift;
            }
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    v |= -1i64 << shift;
                }
                return Some(v);
            }
            if shift > 70 {
                return None;
            }
        }
    }
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
    // The data segments, in linear memory: named as the `name` section
    // names them, or an object's `linking` section (`.rodata.x`).
    let names = m.names(NAME_DATA_SEGMENT);
    let info = if m.is_object() {
        m.segment_info()
    } else {
        Vec::new()
    };
    let first = out.len();
    for (i, p) in placed_segments(m).iter().copied().enumerate() {
        let name: &'data [u8] = names
            .iter()
            .find(|n| n.index == p.index)
            .map(|n| n.name)
            .or_else(|| info.get(p.index as usize).map(|s| s.name))
            .unwrap_or(b"data");
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
    // What `__wasm_init_memory` zeroes: `.bss`, in no segment.
    if !m.is_object() {
        for (dst, size) in init_memory(m).fills {
            out.push(Section {
                index: out.len(),
                name: b".bss",
                addr: MEMORY.wrapping_add(dst),
                vm_size: size,
                file_offset: 0,
                file_size: 0,
                flags: SectionFlags(SectionFlags::READ | SectionFlags::WRITE | SectionFlags::DATA),
                raw_flags: u32::from(SECTION_DATA),
            });
        }
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
    if addr >= MEMORY {
        let p = *placed_segments(m)
            .iter()
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

//! The PE/COFF backend.

use alloc::vec::Vec;

use turbonop_core::Rva;
use turbonop_pe::consts::section_characteristics as sc;
use turbonop_pe::{CodeView, FileKind, PeFile};

use crate::{DebugId, Format, Section, SectionFlags};

pub(crate) fn format(pe: &PeFile<'_>) -> Format {
    match pe.kind() {
        FileKind::Image => Format::Pe,
        FileKind::Object => Format::Coff,
    }
}

/// Translates PE section characteristics to [`SectionFlags`].
pub fn flags(characteristics: u32) -> SectionFlags {
    const MAP: [(u32, u32); 9] = [
        (sc::MEM_READ, SectionFlags::READ),
        (sc::MEM_WRITE, SectionFlags::WRITE),
        (sc::MEM_EXECUTE, SectionFlags::EXECUTE),
        (sc::CNT_CODE, SectionFlags::CODE),
        (sc::CNT_INITIALIZED_DATA, SectionFlags::DATA),
        (sc::CNT_UNINITIALIZED_DATA, SectionFlags::ZERO_FILL),
        // Discardable sections (debug info, relocations of drivers) and
        // linker-only sections (`.drectve`) are not part of the loaded
        // image's lasting state.
        (
            sc::MEM_DISCARDABLE | sc::LNK_REMOVE | sc::LNK_INFO,
            SectionFlags::NOT_LOADED,
        ),
        (sc::LNK_COMDAT, SectionFlags::COMDAT),
        (sc::LNK_INFO, SectionFlags::LINKER_INFO),
    ];
    SectionFlags(
        MAP.iter()
            .filter(|(pe_bits, _)| characteristics & pe_bits != 0)
            .fold(0, |acc, (_, flag)| acc | flag),
    )
}

pub(crate) fn sections<'data>(pe: &PeFile<'data>) -> Vec<Section<'data>> {
    let object = pe.kind() == FileKind::Object;
    let addresses = pe.section_addresses();
    pe.sections()
        .iter()
        .enumerate()
        .map(|(index, h)| {
            let raw_size = u64::from(h.size_of_raw_data.get());
            let characteristics = h.characteristics.get();
            // Zero-fill object sections have a size but no data.
            let file_size = if h.pointer_to_raw_data.get() == 0 {
                0
            } else {
                raw_size
            };
            let (addr, vm_size) = if object {
                // (Laid out as a linker would: `section_addresses`.)
                (addresses.get(index).copied().unwrap_or(0), raw_size)
            } else {
                // A virtual size of 0 means the raw size (old linkers).
                let vsize = match h.virtual_size.get() {
                    0 => raw_size,
                    v => u64::from(v),
                };
                (u64::from(h.virtual_address.get()), vsize)
            };
            Section {
                index,
                name: pe.section_name(h),
                addr,
                vm_size,
                file_offset: u64::from(h.pointer_to_raw_data.get()),
                file_size,
                flags: flags(characteristics),
                raw_flags: characteristics,
            }
        })
        .collect()
}

pub(crate) fn debug_ids<'data>(pe: &PeFile<'data>) -> Vec<DebugId<'data>> {
    let mut out: Vec<DebugId<'data>> = pe
        .debug_entries()
        .filter_map(|e| match e.codeview()? {
            CodeView::Rsds { guid, age, path } => Some(DebugId::Pdb { guid, age, path }),
            CodeView::Nb10 {
                signature,
                age,
                path,
                ..
            } => Some(DebugId::Pdb20 {
                signature,
                age,
                path,
            }),
            // (Other forms, and forms yet to come, name no PDB.)
            _ => None,
        })
        .collect();
    // A MinGW image's separate DWARF, as `objcopy --add-gnu-debuglink`
    // names it: the file's name, NUL-padded to 4 bytes, and its CRC-32.
    if let Some(link) = gnu_debuglink(pe) {
        out.push(link);
    }
    out
}

/// A `.gnu_debuglink` section's file and CRC.
fn gnu_debuglink<'data>(pe: &PeFile<'data>) -> Option<DebugId<'data>> {
    let s = pe
        .sections()
        .iter()
        .find(|s| pe.section_name(s) == b".gnu_debuglink")?;
    let at = usize::try_from(s.pointer_to_raw_data.get()).ok()?;
    let size = s.virtual_size.get().min(s.size_of_raw_data.get());
    let bytes = pe
        .data()
        .get(at..at.checked_add(usize::try_from(size).ok()?)?)?;
    let nul = bytes.iter().position(|&b| b == 0)?;
    let crc_at = (nul + 1).next_multiple_of(4);
    let crc = bytes.get(crc_at..crc_at + 4)?;
    (nul > 0).then(|| DebugId::DebugLink {
        path: &bytes[..nul],
        crc: Some(u32::from_le_bytes([crc[0], crc[1], crc[2], crc[3]])),
    })
}

/// Whether it is a separate debug file of a MinGW image (`objcopy
/// --only-keep-debug`'s): its sections' contents gone but its DWARF's (and
/// its build ID's).
pub(crate) fn debug_only(pe: &PeFile<'_>) -> bool {
    let mut emptied = false;
    let mut dwarf = false;
    for s in pe.sections() {
        let name = pe.section_name(s);
        if name.starts_with(b".debug_") {
            dwarf = true;
        } else if name == b".buildid" || name == b".gnu_debuglink" {
        } else if s.virtual_size.get() > 0 {
            if s.size_of_raw_data.get() != 0 {
                return false;
            }
            emptied = true;
        }
    }
    emptied && dwarf
}

pub(crate) fn pointer_slots(pe: &PeFile<'_>) -> Vec<crate::PointerSlot> {
    use turbonop_pe::reloc::base_reloc;
    if pe.kind() == FileKind::Object {
        return Vec::new();
    }
    let base = pe.optional_header().map_or(0, |h| h.image_base().get());
    pe.base_relocs()
        .filter_map(|r| {
            let width: u8 = match r.kind {
                base_reloc::DIR64 => 8,
                base_reloc::HIGHLOW => 4,
                _ => return None,
            };
            let at = u64::from(r.rva.0);
            // The address the file holds, at the image's preferred base.
            let target = bytes_at(pe, at, u64::from(width)).and_then(|b| {
                let va = b.iter().rev().fold(0u64, |v, &x| (v << 8) | u64::from(x));
                va.checked_sub(base)
            });
            Some(crate::PointerSlot {
                addr: at,
                width,
                target,
            })
        })
        .collect()
}

pub(crate) fn bytes_at<'data>(pe: &PeFile<'data>, addr: u64, len: u64) -> Option<&'data [u8]> {
    if pe.kind() == FileKind::Object {
        // (Where its sections are laid out: `section_addresses`.)
        let addresses = pe.section_addresses();
        let (h, base) = pe.sections().iter().zip(addresses).find(|(h, base)| {
            let size = u64::from(h.size_of_raw_data.get());
            *base != 0
                && h.pointer_to_raw_data.get() != 0
                && addr >= *base
                && addr - base < size
                && len <= size - (addr - base)
        })?;
        let at = usize::try_from(u64::from(h.pointer_to_raw_data.get()) + (addr - base)).ok()?;
        return pe
            .data()
            .get(at..at.checked_add(usize::try_from(len).ok()?)?);
    }
    let rva = u32::try_from(addr).ok()?;
    let len = u32::try_from(len).ok()?;
    pe.bytes_at_rva(Rva(rva), len).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_flags() {
        let f = flags(sc::CNT_CODE | sc::MEM_EXECUTE | sc::MEM_READ);
        assert!(f.contains(SectionFlags::CODE | SectionFlags::EXECUTE | SectionFlags::READ));
        assert!(!f.contains(SectionFlags::WRITE));
        assert!(!f.contains(SectionFlags::NOT_LOADED));
    }

    #[test]
    fn bss_flags() {
        let f = flags(sc::CNT_UNINITIALIZED_DATA | sc::MEM_READ | sc::MEM_WRITE);
        assert!(f.contains(SectionFlags::ZERO_FILL | SectionFlags::WRITE));
        assert!(!f.contains(SectionFlags::DATA));
    }

    #[test]
    fn linker_only_sections_are_not_loaded() {
        assert!(flags(sc::LNK_INFO | sc::LNK_REMOVE).contains(SectionFlags::NOT_LOADED));
        assert!(flags(sc::LNK_INFO).contains(SectionFlags::NOT_LOADED | SectionFlags::LINKER_INFO));
        assert!(flags(sc::LNK_REMOVE).contains(SectionFlags::NOT_LOADED));
        assert!(!flags(sc::LNK_REMOVE).contains(SectionFlags::LINKER_INFO));
        assert!(flags(sc::MEM_DISCARDABLE).contains(SectionFlags::NOT_LOADED));
        assert!(flags(sc::LNK_COMDAT).contains(SectionFlags::COMDAT));
        assert_eq!(flags(0), SectionFlags(0));
    }
}

//! The lists of functions an ELF or Mach-O image's loader calls (and its
//! single functions): what [`crate::startup`] orders into phases, and what
//! [`crate::xref`] roots its graph at, read once, the same way.
//!
//! - ELF: `.preinit_array`, `.init_array`, `.fini_array` (by their section
//!   types), `.ctors` and `.dtors` (by their names: `.ctors` is called
//!   last first, and has -1 and 0 at its ends), and the dynamic table's
//!   `DT_INIT` and `DT_FINI`.
//! - Mach-O: `S_MOD_INIT_FUNC_POINTERS` and `S_MOD_TERM_FUNC_POINTERS`
//!   sections' pointers, and `S_INIT_FUNC_OFFSETS` sections' offsets from
//!   the image's base.
//!
//! Each list's entries are read from the file, through the loader's
//! relocations where it writes them (see
//! [`hexcavator_image::Image::pointer_slots`]); a Thumb function's address
//! is without its low bit.
//!
//! (A PE image's CRT groups, `.CRT$X*`, are the debug information's COFF
//! groups: the model's, not the image's alone.)

use hexcavator_image::Image;

/// The most entries read of one list (its size is what its header says,
/// which a damaged file can make anything).
pub const MAX_ENTRIES: u64 = 1 << 20;

/// When the loader calls a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// Before any library's initializers (an ELF program's
    /// `.preinit_array`).
    PreInit,
    /// When the image is loaded: `DT_INIT`, then `.ctors`, then
    /// `.init_array`; Mach-O's initializers.
    Init,
    /// When it is unloaded (or the program exits).
    Fini,
}

/// A list of functions the loader calls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct List {
    /// Its section's name (`.init_array`, `__mod_init_func`), or the
    /// dynamic table's entry's (`DT_INIT`).
    pub name: String,
    /// Its name with its segment's, for a Mach-O section
    /// (`__DATA,__mod_init_func`); else its name.
    pub full_name: String,
    pub when: When,
    /// Its section's extent (none for a dynamic table's entry).
    pub section: Option<(u64, u64)>,
    /// Each function, in the order the loader calls them: where its
    /// address is (a list's slot), and the address.
    pub entries: Vec<(Option<u64>, u64)>,
}

/// The image's lists (none for formats without them here).
pub fn lists(image: &Image<'_>) -> Vec<List> {
    if let Some(elf) = image.elf() {
        return elf_lists(image, elf);
    }
    if let Some(m) = image.macho() {
        return macho_lists(image, m);
    }
    Vec::new()
}

/// The words of `[start, end)`, `wide` (8 bytes) or not, each with its
/// value: as the loader relocates it (`slots`), else as it is in the
/// file.
pub fn words(
    image: &Image<'_>,
    start: u64,
    end: u64,
    wide: bool,
    slots: &[hexcavator_image::PointerSlot],
) -> Vec<(u64, u64)> {
    let w: u64 = if wide { 8 } else { 4 };
    let big = image.endian() == hexcavator_core::Endian::Big;
    let mut out = Vec::new();
    let count = (end.saturating_sub(start) / w).min(MAX_ENTRIES);
    for i in 0..count {
        let at = start + i * w;
        let relocated = slots
            .binary_search_by_key(&at, |p| p.addr)
            .ok()
            .and_then(|i| slots[i].target);
        let raw = image.bytes_at(at, w).map(|b| {
            let mut v = [0u8; 8];
            if big {
                v[8 - b.len()..].copy_from_slice(b);
                u64::from_be_bytes(v)
            } else {
                v[..b.len()].copy_from_slice(b);
                u64::from_le_bytes(v)
            }
        });
        if let Some(value) = relocated.or(raw) {
            out.push((at, value));
        }
    }
    out
}

fn elf_lists(image: &Image<'_>, elf: &hexcavator_elf::ElfFile<'_>) -> Vec<List> {
    use hexcavator_elf::consts::*;
    let h = elf.header();
    let wide = h.class == hexcavator_elf::Class::Elf64;
    let slots = image.pointer_slots();
    let thumb = h.machine == EM_ARM;
    let code = |v: u64| if thumb { v & !1 } else { v };
    let all = if wide { u64::MAX } else { u64::from(u32::MAX) };
    let mut out = Vec::new();
    for (tag, name, when) in [
        (DT_INIT, "DT_INIT", When::Init),
        (DT_FINI, "DT_FINI", When::Fini),
    ] {
        let entries: Vec<(Option<u64>, u64)> = elf
            .dynamic()
            .iter()
            .filter(|d| d.tag == tag && d.value != 0)
            .map(|d| (None, code(d.value)))
            .collect();
        if !entries.is_empty() {
            out.push(List {
                name: name.into(),
                full_name: name.into(),
                when,
                section: None,
                entries,
            });
        }
    }
    for s in elf.sections().filter(hexcavator_elf::Section::is_alloc) {
        let (when, reverse) = match (s.kind, s.name) {
            (SHT_PREINIT_ARRAY, _) => (When::PreInit, false),
            (SHT_INIT_ARRAY, _) => (When::Init, false),
            (SHT_FINI_ARRAY, _) => (When::Fini, false),
            (_, b".ctors") => (When::Init, true),
            (_, b".dtors") => (When::Fini, false),
            _ => continue,
        };
        let mut w = words(image, s.addr, s.addr.saturating_add(s.size), wide, &slots);
        if reverse {
            w.reverse();
        }
        let name = String::from_utf8_lossy(s.name).into_owned();
        out.push(List {
            full_name: name.clone(),
            name,
            when,
            section: Some((s.addr, s.addr.saturating_add(s.size))),
            entries: w
                .into_iter()
                .filter(|&(_, v)| v != 0 && v != all)
                .map(|(slot, v)| (Some(slot), code(v)))
                .collect(),
        });
    }
    out
}

fn macho_lists(image: &Image<'_>, m: &hexcavator_macho::MachO<'_>) -> Vec<List> {
    use hexcavator_macho::consts::*;
    let wide = m.header().is64;
    let slots = image.pointer_slots();
    let base = m
        .segments()
        .iter()
        .find(|s| s.fileoff == 0 && s.filesize > 0)
        .map_or(0, |s| s.vmaddr);
    let mut out = Vec::new();
    for s in m.sections() {
        let (start, end) = (s.addr, s.addr.saturating_add(s.size));
        let (when, entries): (When, Vec<(Option<u64>, u64)>) = match s.kind() {
            S_MOD_INIT_FUNC_POINTERS | S_MOD_TERM_FUNC_POINTERS => (
                if s.kind() == S_MOD_INIT_FUNC_POINTERS {
                    When::Init
                } else {
                    When::Fini
                },
                words(image, start, end, wide, &slots)
                    .into_iter()
                    .filter(|&(_, v)| v != 0)
                    .map(|(slot, v)| (Some(slot), v))
                    .collect(),
            ),
            S_INIT_FUNC_OFFSETS => {
                let bytes = image.bytes_at(start, s.size).unwrap_or_default();
                let entries = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .enumerate()
                    .take(MAX_ENTRIES as usize)
                    .map(|(i, w)| {
                        let off = u64::from(u32::from_le_bytes(*w));
                        (Some(start + 4 * i as u64), base.wrapping_add(off))
                    })
                    .collect();
                (When::Init, entries)
            }
            _ => continue,
        };
        out.push(List {
            name: String::from_utf8_lossy(s.sectname).into_owned(),
            full_name: format!(
                "{},{}",
                String::from_utf8_lossy(s.segname),
                String::from_utf8_lossy(s.sectname)
            ),
            when,
            section: Some((start, end)),
            entries,
        });
    }
    out
}

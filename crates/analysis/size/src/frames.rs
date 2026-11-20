//! An image's call frame information, as its unwinder reads it: each
//! function's FDE in `.eh_frame` (an ELF image's) or `__eh_frame` (a
//! Mach-O image's), with its size, its LSDA (its language's exception
//! table: catch clauses, cleanups), and its personality routine.

use hexcavator_dwarf::cfi::{Cfi, CfiKind, Entry};
use hexcavator_image::Image;

/// A function's frame description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The function's address.
    pub function: u64,
    /// Its FDE's bytes (to the next entry).
    pub size: u64,
    /// Its LSDA's address, if it has one.
    pub lsda: Option<u64>,
    /// Its CIE's personality routine's address, if any.
    pub personality: Option<u64>,
}

/// The image's frame descriptions (none for a format without
/// `.eh_frame`).
pub fn frames(image: &Image<'_>) -> Vec<Frame> {
    let mut out = Vec::new();
    if image.elf().is_none() && image.macho().is_none() {
        return out;
    }
    for s in image
        .sections()
        .into_iter()
        .filter(|s| (s.name == b".eh_frame" || s.name == b"__eh_frame") && s.file_size > 0)
    {
        let Some(data) = image.bytes_at(s.addr, s.file_size.min(s.vm_size)) else {
            continue;
        };
        let cfi = Cfi {
            data,
            kind: CfiKind::EhFrame,
            endian: image.endian(),
            address: s.addr,
            address_size: image.arch().pointer_size().unwrap_or(8),
            text_base: None,
            data_base: None,
        };
        let entries: Vec<Entry<'_>> = cfi.entries().collect();
        let offset = |e: &Entry<'_>| match e {
            Entry::Cie(c) => c.offset,
            Entry::Fde(f) => f.offset,
        };
        let personality = |cie: u64| {
            entries.iter().find_map(|e| match e {
                Entry::Cie(c) if c.offset == cie => c.personality,
                _ => None,
            })
        };
        for (i, e) in entries.iter().enumerate() {
            if let Entry::Fde(f) = e {
                let end = entries.get(i + 1).map_or(data.len() as u64, offset);
                out.push(Frame {
                    function: f.pc_begin,
                    size: end.saturating_sub(f.offset),
                    lsda: f.lsda,
                    personality: personality(f.cie),
                });
            }
        }
    }
    out
}

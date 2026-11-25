//! Module identities read from a target's memory: an ELF module's build
//! ID from its headers and notes where they were loaded (which Linux's
//! core files keep, and minidumps sometimes do).

use alloc::vec;
use alloc::vec::Vec;

use turbonop_elf::ElfFile;
use turbonop_elf::consts::{NT_GNU_BUILD_ID, PT_LOAD, PT_NOTE};

use crate::Memory;
use crate::model::read_exact;

/// The most of a module's first bytes read for its headers.
const HEADERS: usize = 4096;
/// The most of a note segment read.
const NOTES: u64 = 64 * 1024;

/// What `memory` says of the ELF module loaded at `base`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ElfInMemory {
    /// Its headers aren't in memory.
    Unknown,
    /// What's at `base` isn't an ELF file's header.
    NotElf,
    /// An ELF module, and its build ID if its notes are in memory and
    /// have one.
    Elf { build_id: Option<Vec<u8>> },
}

/// Reads the ELF module loaded at `base` from `memory`.
pub fn elf_in_memory(memory: &dyn Memory, base: u64) -> ElfInMemory {
    let mut page = vec![0u8; HEADERS];
    let mut n = 0;
    while n < page.len() {
        let got = memory.read_at(base + n as u64, &mut page[n..]);
        if got == 0 {
            break;
        }
        n += got;
    }
    page.truncate(n);
    if page.len() < 4 {
        return ElfInMemory::Unknown;
    }
    if !page.starts_with(b"\x7fELF") {
        return ElfInMemory::NotElf;
    }
    let Ok(elf) = ElfFile::parse(&page) else {
        return ElfInMemory::Elf { build_id: None };
    };
    // Where the file's first byte was linked to be: its lowest loaded
    // segment's address less that segment's offset.
    let link = elf
        .segments()
        .filter(|s| s.kind == PT_LOAD)
        .min_by_key(|s| s.vaddr)
        .map_or(0, |s| s.vaddr.wrapping_sub(s.offset));
    for s in elf.segments().filter(|s| s.kind == PT_NOTE) {
        let at = base.wrapping_add(s.vaddr.wrapping_sub(link));
        let Ok(len) = usize::try_from(s.filesz.min(NOTES)) else {
            continue;
        };
        let mut notes = vec![0u8; len];
        if !read_exact(memory, at, &mut notes) {
            continue;
        }
        if let Some(n) = elf
            .notes(&notes)
            .into_iter()
            .find(|n| n.name == b"GNU" && n.kind == NT_GNU_BUILD_ID)
        {
            return ElfInMemory::Elf {
                build_id: Some(n.desc.to_vec()),
            };
        }
    }
    ElfInMemory::Elf { build_id: None }
}

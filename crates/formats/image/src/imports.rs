//! The slots an image's loader fills with other modules' addresses, and
//! what each is of: PE's import address tables (and delay-load ones),
//! ELF's GOT and PLT entries for symbols it doesn't define, Mach-O's binds.

use alloc::vec::Vec;

use crate::Image;

/// What an [`ImportSlot`] holds the address of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Imported<'data> {
    /// A symbol, by its name (as linked: decorated, mangled).
    Name(&'data [u8]),
    /// A PE export by its ordinal.
    Ordinal(u16),
}

/// A slot the loader fills with an address in another module.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImportSlot<'data> {
    /// Where the slot is.
    pub addr: u64,
    /// Its width in bytes.
    pub width: u8,
    /// The library it's from, if the format says (PE's DLL, Mach-O's
    /// dylib's install name; ELF's symbols are looked up in every library).
    pub library: Option<&'data [u8]>,
    /// What it's the address of.
    pub imported: Imported<'data>,
    /// Whether it's filled on first use (PE's delay-load imports), not
    /// when the image loads.
    pub delayed: bool,
}

impl<'data> ImportSlot<'data> {
    /// The import's name as one string: `library!name` (or
    /// `library!#ordinal`) where the library is known, else the name.
    pub fn qualified_name(&self) -> alloc::string::String {
        use alloc::format;
        use alloc::string::String;
        let what = match self.imported {
            Imported::Name(n) => String::from_utf8_lossy(n).into_owned(),
            Imported::Ordinal(o) => format!("#{o}"),
        };
        match self.library {
            Some(l) => format!("{}!{what}", String::from_utf8_lossy(l)),
            None => what,
        }
    }
}

impl<'data> Image<'data> {
    /// The slots its loader fills with other modules' addresses, by
    /// address (each once): what [`Image::pointer_slots`] has no target
    /// for, named. Objects have none.
    pub fn import_slots(&self) -> Vec<ImportSlot<'data>> {
        let mut out = match self {
            Image::Pe(pe) => pe_slots(pe),
            Image::Elf(elf) => elf_slots(elf),
            Image::MachO(m) => macho_slots(m),
            // (A module's imports are functions and globals by index, not
            // slots in its memory.)
            Image::Wasm(_) => Vec::new(),
        };
        out.sort_by_key(|s| s.addr);
        out.dedup_by_key(|s| s.addr);
        out
    }
}

fn pe_slots<'data>(pe: &hexcavator_pe::PeFile<'data>) -> Vec<ImportSlot<'data>> {
    use hexcavator_pe::imports::ImportName;
    let mut out = Vec::new();
    if pe.kind() == hexcavator_pe::FileKind::Object {
        return out;
    }
    let width = if pe.optional_header().is_some_and(|h| h.is_64()) {
        8
    } else {
        4
    };
    let imported = |i: ImportName<'data>| match i {
        ImportName::Name { name, .. } => Imported::Name(name),
        ImportName::Ordinal(o) => Imported::Ordinal(o),
    };
    for dll in pe.imports() {
        for t in dll.thunks() {
            out.push(ImportSlot {
                addr: u64::from(t.iat_rva.0),
                width,
                library: Some(dll.name),
                imported: imported(t.import),
                delayed: false,
            });
        }
    }
    for dll in pe.delay_imports() {
        for t in dll.thunks() {
            out.push(ImportSlot {
                addr: u64::from(t.iat_rva.0),
                width,
                library: Some(dll.name),
                imported: imported(t.import),
                delayed: true,
            });
        }
    }
    out
}

fn elf_slots<'data>(elf: &hexcavator_elf::ElfFile<'data>) -> Vec<ImportSlot<'data>> {
    use hexcavator_elf::consts::{SHN_UNDEF, SHT_REL, SHT_RELA};
    let mut out = Vec::new();
    if elf.is_object() {
        return out;
    }
    let width = match elf.class() {
        hexcavator_elf::Class::Elf32 => 4,
        hexcavator_elf::Class::Elf64 => 8,
    };
    let machine = elf.header().machine;
    let sections: Vec<_> = elf.sections().collect();
    for s in sections
        .iter()
        .filter(|s| s.is_alloc() && (s.kind == SHT_REL || s.kind == SHT_RELA))
    {
        let symbols = sections
            .get(s.link as usize)
            .map(|t| elf.symbols(t))
            .unwrap_or_default();
        for r in elf.relocations(s) {
            if hexcavator_elf::address_relocation(machine, r.kind)
                != Some(hexcavator_elf::AddressRelocation::Symbol)
            {
                continue;
            }
            if let Some(sym) = symbols
                .get(r.symbol as usize)
                .filter(|sym| sym.shndx == u32::from(SHN_UNDEF) && !sym.name.is_empty())
            {
                out.push(ImportSlot {
                    addr: r.offset,
                    width,
                    library: None,
                    imported: Imported::Name(sym.name),
                    delayed: false,
                });
            }
        }
    }
    out
}

fn macho_slots<'data>(m: &hexcavator_macho::MachO<'data>) -> Vec<ImportSlot<'data>> {
    let mut out = Vec::new();
    if m.is_object() {
        return out;
    }
    let dylibs = m.dylibs();
    for f in m.fixups() {
        let hexcavator_macho::FixupKind::Bind {
            symbol, library, ..
        } = f.kind
        else {
            continue;
        };
        if symbol.is_empty() {
            continue;
        }
        // (Ordinals count the libraries from 1; 0 and below are the image
        // itself, the main executable, and a flat lookup.)
        let library = usize::try_from(library)
            .ok()
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| dylibs.get(i))
            .map(|d| d.name);
        out.push(ImportSlot {
            addr: f.addr,
            width: f.width,
            library,
            imported: Imported::Name(symbol),
            delayed: false,
        });
    }
    out
}

//! An image's own symbols: what it names without debug information (PE
//! exports and COFF symbols, ELF symbol tables, Mach-O's, WebAssembly's
//! `name` section), and the extent of the image in memory.

use alloc::vec::Vec;

use crate::Image;

/// What an [`ImageSymbol`] names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum SymbolKind {
    /// Code.
    Function,
    /// Data.
    Data,
    /// Something else, or not said: in a linked image, only what is in no
    /// section (the others are known by their sections).
    Other,
}

/// Where an [`ImageSymbol`] is from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum SymbolSource {
    /// The full symbol table (ELF `.symtab`, Mach-O's, COFF's).
    SymbolTable,
    /// What the dynamic linker sees (ELF `.dynsym`, PE exports).
    Exported,
    /// WebAssembly's `name` section.
    Names,
}

/// A symbol the image names itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageSymbol<'data> {
    /// Its address (see the crate's addresses).
    pub addr: u64,
    /// Its size: as the format records it (ELF, WebAssembly), or else, in
    /// a linked image, to the next symbol in its section or the section's
    /// end (see [`ImageSymbol::size_recorded`]).
    pub size: Option<u64>,
    /// Whether the format recorded its size (not measured to the next
    /// symbol).
    pub size_recorded: bool,
    /// Its name, as linked (decorated, mangled).
    pub name: &'data [u8],
    /// What it names.
    pub kind: SymbolKind,
    /// Where it is from.
    pub source: SymbolSource,
    /// Whether it is global (seen from other modules).
    pub global: bool,
}

impl<'data> Image<'data> {
    /// The image's own symbols with addresses, sorted by address; at an
    /// address named in more than one table, the full table's first. Files
    /// and sections' names, and undefined and absolute symbols, are left
    /// out.
    ///
    /// In a linked image every format's are alike: each symbol is code or
    /// data by its section if its format doesn't say (a Mach-O symbol, a
    /// PE export), has a size (to the next symbol in its section, if its
    /// format records none), and a 32-bit ARM function's address is
    /// without its Thumb bit.
    pub fn symbols(&self) -> Vec<ImageSymbol<'data>> {
        let mut out = match self {
            Image::Pe(pe) => pe_symbols(pe),
            Image::Elf(elf) => elf_symbols(elf),
            Image::MachO(m) => macho_symbols(m),
            Image::Wasm(m) => wasm_symbols(m),
        };
        if self.kind() == crate::Kind::Linked {
            self.complete(&mut out);
        }
        sort(&mut out);
        out
    }

    /// `symbols`' kinds, Thumb bits, and sizes, by their sections.
    fn complete(&self, symbols: &mut Vec<ImageSymbol<'data>>) {
        use crate::section::SectionFlags as F;
        let mut sections: Vec<crate::Section<'data>> = self
            .sections()
            .into_iter()
            .filter(|s| s.vm_size > 0 && !s.flags.contains(F::NOT_LOADED))
            .collect();
        sections.sort_by_key(|s| s.addr);
        let section_of = |addr: u64| {
            let i = sections
                .partition_point(|s| s.addr <= addr)
                .checked_sub(1)?;
            sections.get(i).filter(|s| s.contains(addr))
        };
        let thumb = self.arch() == crate::Arch::Arm;
        for s in symbols.iter_mut() {
            let Some(section) = section_of(s.addr) else {
                continue;
            };
            if s.kind == SymbolKind::Other {
                // (Code by what the section holds, not its mapping: a
                // Mach-O image's `__TEXT,__const` is executable data.)
                s.kind = if section.flags.contains(F::CODE) {
                    SymbolKind::Function
                } else {
                    SymbolKind::Data
                };
            }
            if thumb && s.kind == SymbolKind::Function {
                s.addr &= !1;
            }
        }
        sort(symbols);
        // Sizes: to the next address named in the same section, or its end.
        let starts: Vec<u64> = symbols.iter().map(|s| s.addr).collect();
        for s in symbols.iter_mut().filter(|s| s.size.is_none()) {
            let Some(section) = section_of(s.addr) else {
                continue;
            };
            let end = section.end().unwrap_or(u64::MAX);
            let next = starts[starts.partition_point(|&a| a <= s.addr)..]
                .first()
                .copied()
                .filter(|&n| n < end)
                .unwrap_or(end);
            s.size = Some(next - s.addr).filter(|&n| n > 0);
        }
    }

    /// The image's extent in memory, as `[start, end)` in the format's
    /// tools' addresses (PE images with their preferred base added): where
    /// its first loaded byte goes, and past its last. The start is what a
    /// `module+offset` address counts from. `None` for objects, and for
    /// images that load nothing.
    pub fn extent(&self) -> Option<(u64, u64)> {
        match self {
            Image::Pe(pe) => {
                let h = pe.optional_header()?;
                let base = h.image_base().get();
                Some((base, base.checked_add(u64::from(h.size_of_image()))?))
            }
            Image::Elf(elf) => {
                use hexcavator_elf::consts::PT_LOAD;
                let mut range: Option<(u64, u64)> = None;
                for s in elf.segments().filter(|s| s.kind == PT_LOAD && s.memsz > 0) {
                    // (The first page holds the headers: a load segment
                    // starts at its alignment.)
                    let align = if s.align.is_power_of_two() {
                        s.align
                    } else {
                        1
                    };
                    let start = s.vaddr & !(align - 1);
                    let end = s.vaddr.checked_add(s.memsz)?;
                    range = Some(match range {
                        Some((a, b)) => (a.min(start), b.max(end)),
                        None => (start, end),
                    });
                }
                range
            }
            Image::MachO(m) => {
                let mut range: Option<(u64, u64)> = None;
                for s in m.segments() {
                    // (`__PAGEZERO` maps no file and guards address 0.)
                    if s.vmsize == 0 || (s.filesize == 0 && s.initprot == 0) {
                        continue;
                    }
                    let end = s.vmaddr.checked_add(s.vmsize)?;
                    range = Some(match range {
                        Some((a, b)) => (a.min(s.vmaddr), b.max(end)),
                        None => (s.vmaddr, end),
                    });
                }
                range
            }
            Image::Wasm(_) => {
                let code = self.sections().into_iter().find(|s| s.vm_size > 0)?;
                Some((0, code.vm_size))
            }
        }
    }
}

/// By address; at an address, the full symbol table's first, then those
/// with sizes; each name at an address once.
fn sort(symbols: &mut Vec<ImageSymbol<'_>>) {
    symbols.sort_by(|a, b| {
        (a.addr, a.source, !a.size_recorded, b.size.is_some())
            .cmp(&(b.addr, b.source, !b.size_recorded, a.size.is_some()))
            .then_with(|| a.name.cmp(b.name))
    });
    symbols.dedup_by(|a, b| a.addr == b.addr && a.name == b.name);
}

fn pe_symbols<'data>(pe: &hexcavator_pe::PeFile<'data>) -> Vec<ImageSymbol<'data>> {
    use hexcavator_pe::exports::ExportTarget;
    let mut out = Vec::new();
    if let Some(exports) = pe.exports() {
        for e in exports.iter() {
            if let (Some(name), ExportTarget::Rva(rva)) = (e.name, e.target) {
                out.push(ImageSymbol {
                    addr: u64::from(rva.0),
                    size: None,
                    size_recorded: false,
                    name,
                    kind: SymbolKind::Other,
                    source: SymbolSource::Exported,
                    global: true,
                });
            }
        }
    }
    // A COFF symbol table (MinGW's images keep one).
    if let Ok(Some(table)) = pe.symbols() {
        let sections = pe.sections();
        let mut i = 0;
        while let Some(s) = table.get(i) {
            i += 1 + s.aux_count();
            let Some(header) = usize::try_from(s.section - 1)
                .ok()
                .and_then(|n| sections.get(n))
            else {
                continue;
            };
            // External, static, and label symbols; not files, sections,
            // or functions' begin and end markers.
            const EXTERNAL: u8 = 2;
            const STATIC: u8 = 3;
            if !matches!(s.storage_class, EXTERNAL | STATIC)
                || (s.storage_class == STATIC && s.value == 0 && s.aux_count() > 0)
            {
                continue;
            }
            let Some(name) = table.name(&s).filter(|n| !n.is_empty()) else {
                continue;
            };
            let code = s.kind >> 4 == 2;
            out.push(ImageSymbol {
                addr: u64::from(header.virtual_address.get()) + u64::from(s.value),
                size: None,
                size_recorded: false,
                name,
                kind: if code {
                    SymbolKind::Function
                } else {
                    SymbolKind::Other
                },
                source: SymbolSource::SymbolTable,
                global: s.storage_class == EXTERNAL,
            });
        }
    }
    out
}

fn elf_symbols<'data>(elf: &hexcavator_elf::ElfFile<'data>) -> Vec<ImageSymbol<'data>> {
    use hexcavator_elf::consts::{SHT_DYNSYM, SHT_SYMTAB, STB_LOCAL, STT_FUNC, STT_OBJECT};
    /// `STT_GNU_IFUNC`: an indirect function (a resolver's).
    const STT_GNU_IFUNC: u8 = 10;
    /// `STT_TLS`: thread-local data, at an offset, not an address.
    const STT_TLS: u8 = 6;
    let mut out = Vec::new();
    for table in elf
        .sections()
        .filter(|s| s.kind == SHT_SYMTAB || s.kind == SHT_DYNSYM)
    {
        let source = if table.kind == SHT_SYMTAB {
            SymbolSource::SymbolTable
        } else {
            SymbolSource::Exported
        };
        for s in elf.symbols(&table) {
            // Defined in a section (not undefined, absolute, or common).
            if s.shndx == 0 || s.shndx >= 0xFF00 || s.name.is_empty() || s.kind == STT_TLS {
                continue;
            }
            // (ARM's mapping symbols mark code and data, naming nothing.)
            if s.name.starts_with(b"$") {
                continue;
            }
            let kind = match s.kind {
                STT_FUNC | STT_GNU_IFUNC => SymbolKind::Function,
                STT_OBJECT => SymbolKind::Data,
                0 if s.bind != STB_LOCAL => SymbolKind::Other,
                _ => continue,
            };
            out.push(ImageSymbol {
                // (Thumb functions' addresses have their low bit set.)
                addr: if kind == SymbolKind::Function
                    && elf.header().machine == hexcavator_elf::consts::EM_ARM
                {
                    s.value & !1
                } else {
                    s.value
                },
                size: (s.size > 0).then_some(s.size),
                size_recorded: s.size > 0,
                name: s.name,
                kind,
                source,
                global: s.bind != STB_LOCAL,
            });
        }
    }
    out
}

fn macho_symbols<'data>(m: &hexcavator_macho::MachO<'data>) -> Vec<ImageSymbol<'data>> {
    use hexcavator_macho::consts::N_EXT;
    let mut out = Vec::new();
    for s in m.symbols() {
        if !s.is_section_defined() || s.name.is_empty() {
            continue;
        }
        out.push(ImageSymbol {
            addr: s.n_value,
            size: None,
            size_recorded: false,
            name: s.name,
            kind: SymbolKind::Other,
            source: SymbolSource::SymbolTable,
            global: s.n_type & N_EXT != 0,
        });
    }
    out
}

/// A module's functions, at their bodies (as DWARF's addresses are: see
/// `wasm::function_range`).
fn wasm_symbols<'data>(m: &hexcavator_wasm::Module<'data>) -> Vec<ImageSymbol<'data>> {
    let mut out = Vec::new();
    let functions = m.functions();
    let first = functions.first().map_or(0, |f| f.index);
    let function = |index: u32| {
        index
            .checked_sub(first)
            .and_then(|i| functions.get(usize::try_from(i).ok()?))
    };
    // (Exported functions are global: seen from the host, by their export
    // names too.)
    /// An export's kind: a function.
    const EXPORT_FUNCTION: u8 = 0;
    let exports: Vec<hexcavator_wasm::Export<'data>> = m
        .exports()
        .into_iter()
        .filter(|e| e.kind == EXPORT_FUNCTION)
        .collect();
    let exported = |index: u32| exports.iter().any(|e| e.index == index);
    for n in m.names(hexcavator_wasm::NAME_FUNCTION) {
        let Some((addr, size)) = function(n.index).and_then(|f| crate::wasm::function_range(m, f))
        else {
            continue;
        };
        out.push(ImageSymbol {
            addr,
            size: Some(size),
            size_recorded: true,
            name: n.name,
            kind: SymbolKind::Function,
            source: SymbolSource::Names,
            global: exported(n.index),
        });
    }
    for e in &exports {
        let Some((addr, size)) = function(e.index).and_then(|f| crate::wasm::function_range(m, f))
        else {
            continue;
        };
        out.push(ImageSymbol {
            addr,
            size: Some(size),
            size_recorded: true,
            name: e.name,
            kind: SymbolKind::Function,
            source: SymbolSource::Exported,
            global: true,
        });
    }
    // Exported globals that hold addresses in linear memory (wasm-ld's for
    // exported data, `__data_end`, `__heap_base`): data there. (A mutable
    // one, the stack pointer, holds no data's address.)
    /// An export's kind: a global.
    const EXPORT_GLOBAL: u8 = 3;
    if !m.is_object() {
        let globals = m.globals();
        for e in m.exports().into_iter().filter(|e| e.kind == EXPORT_GLOBAL) {
            let Some(g) = globals.iter().find(|g| g.index == e.index && !g.mutable) else {
                continue;
            };
            let at = match g.init {
                hexcavator_wasm::Init::I32(v) => u64::from(v.cast_unsigned()),
                hexcavator_wasm::Init::I64(v) => v.cast_unsigned(),
                _ => continue,
            };
            out.push(ImageSymbol {
                addr: crate::wasm::MEMORY.wrapping_add(at),
                size: None,
                size_recorded: false,
                name: e.name,
                kind: SymbolKind::Data,
                source: SymbolSource::Exported,
                global: true,
            });
        }
    }
    out
}

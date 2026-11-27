//! A COFF object, read once into what the link needs: its sections (with
//! their COMDAT selections), its symbols (one entry per symbol table
//! record, so that relocations' symbol indices index it directly), and
//! its directives. Names and contents borrow from the input's bytes,
//! which live as long as the link (`'static`: inputs are never unmapped).

use turbonop_core::read_slice_at;
use turbonop_pe::raw::Relocation;
use turbonop_pe::{PeFile, comdat_select};

use crate::args::Arg;

/// `IMAGE_SCN_*` characteristics the link looks at.
pub mod scn {
    /// Code.
    pub const CNT_CODE: u32 = 0x0000_0020;
    /// Initialized data.
    pub const CNT_INITIALIZED_DATA: u32 = 0x0000_0040;
    /// Uninitialized data (`.bss`): no contents in the file.
    pub const CNT_UNINITIALIZED_DATA: u32 = 0x0000_0080;
    /// Comments or other information (`.drectve`, `.debug$S`).
    pub const LNK_INFO: u32 = 0x0000_0200;
    /// Not part of the image.
    pub const LNK_REMOVE: u32 = 0x0000_0800;
    /// A COMDAT.
    pub const LNK_COMDAT: u32 = 0x0000_1000;
    /// The alignment field's mask (`IMAGE_SCN_ALIGN_*`).
    pub const ALIGN_MASK: u32 = 0x00F0_0000;
    /// More relocations than 0xFFFF: the first one's address is the count.
    pub const LNK_NRELOC_OVFL: u32 = 0x0100_0000;
    /// Discardable.
    pub const MEM_DISCARDABLE: u32 = 0x0200_0000;
    /// Not cached.
    pub const MEM_NOT_CACHED: u32 = 0x0400_0000;
    /// Not paged.
    pub const MEM_NOT_PAGED: u32 = 0x0800_0000;
    /// Shared.
    pub const MEM_SHARED: u32 = 0x1000_0000;
    /// Executable.
    pub const MEM_EXECUTE: u32 = 0x2000_0000;
    /// Readable.
    pub const MEM_READ: u32 = 0x4000_0000;
    /// Writable.
    pub const MEM_WRITE: u32 = 0x8000_0000;
    /// What an image's section header keeps of a section's characteristics.
    pub const OUTPUT_MASK: u32 = CNT_CODE
        | CNT_INITIALIZED_DATA
        | CNT_UNINITIALIZED_DATA
        | MEM_DISCARDABLE
        | MEM_NOT_CACHED
        | MEM_NOT_PAGED
        | MEM_SHARED
        | MEM_EXECUTE
        | MEM_READ
        | MEM_WRITE;
}

/// `IMAGE_SYM_CLASS_*` storage classes the link looks at.
pub mod class {
    /// A function's end (`.ef`) or a label in old objects.
    pub const END_OF_FUNCTION: u8 = 0xFF;
    /// External: defined, common, or undefined.
    pub const EXTERNAL: u8 = 2;
    /// Static: section symbols, `static` functions and data, labels.
    pub const STATIC: u8 = 3;
    /// A label.
    pub const LABEL: u8 = 6;
    /// `.file`.
    pub const FILE: u8 = 103;
    /// A section (old objects).
    pub const SECTION: u8 = 104;
    /// A weak external.
    pub const WEAK_EXTERNAL: u8 = 105;
}

/// A COMDAT section's selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Only one may be defined (`NODUPLICATES`).
    NoDuplicates,
    /// Any one (`ANY`).
    Any,
    /// Any one, if all are the same size (`SAME_SIZE`).
    SameSize,
    /// Any one, if all are identical (`EXACT_MATCH`).
    ExactMatch,
    /// Kept if and only if `section` is (`ASSOCIATIVE`: unwind data,
    /// debug information, of a COMDAT function). 0-based.
    Associative(u32),
    /// The largest (`LARGEST`).
    Largest,
}

/// One section of an object.
#[derive(Clone, Debug)]
pub struct Section {
    /// Its name (`.text$mn`).
    pub name: &'static [u8],
    /// Its characteristics.
    pub characteristics: u32,
    /// Its contents (empty for uninitialized data).
    pub data: &'static [u8],
    /// Its size (for uninitialized data, the size it occupies).
    pub size: u32,
    /// Its relocations.
    pub relocations: &'static [Relocation],
    /// For a COMDAT, its selection, and its checksum (for `EXACT_MATCH`).
    pub comdat: Option<(Selection, u32)>,
}

impl Section {
    /// Its alignment in bytes (1 if none is given; a COMDAT or data
    /// section without one is aligned to 16, as link.exe does).
    pub fn alignment(&self) -> u32 {
        match (self.characteristics & scn::ALIGN_MASK) >> 20 {
            0 => 16,
            n => 1 << (n - 1),
        }
    }

    /// Whether the section is part of the image: not removed, not
    /// information for the linker or a debugger.
    pub fn is_image(&self) -> bool {
        self.characteristics & (scn::LNK_REMOVE | scn::LNK_INFO) == 0
            && !self.name.starts_with(b".debug$")
    }
}

/// What a symbol is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    /// An auxiliary record, not a symbol.
    Aux,
    /// External, defined in `section` (0-based) at `value`.
    Defined {
        /// 0-based.
        section: u32,
        /// Offset in the section.
        value: u32,
    },
    /// External, undefined.
    Undefined,
    /// External, common (uninitialized) data of this size.
    Common(u32),
    /// External or static, absolute.
    Absolute(u32),
    /// A weak external: if nothing defines the name, `default` (a
    /// symbol index) is used. `search` is the auxiliary record's
    /// characteristics (1 no library search, 2 library search, 3 alias).
    Weak {
        /// The symbol index of the default.
        default: u32,
        /// `IMAGE_WEAK_EXTERN_SEARCH_*`.
        search: u32,
    },
    /// Local to the object, defined in `section` (0-based) at `value`
    /// (section symbols, statics, labels).
    Local {
        /// 0-based.
        section: u32,
        /// Offset in the section.
        value: u32,
    },
    /// Nothing the link uses (`.file`, debug symbols, `@feat.00`'s
    /// absolute value aside).
    Other,
}

/// One record of the symbol table.
#[derive(Clone, Copy, Debug)]
pub struct Symbol {
    /// Its name (empty for auxiliary records).
    pub name: &'static [u8],
    /// What it is.
    pub kind: SymbolKind,
    /// The `Type` field (0x20 for functions).
    pub ty: u16,
    /// The storage class (2 external, 3 static, 6 label, ...).
    pub class: u8,
    /// How many auxiliary records follow it (a section's definition has
    /// one).
    pub aux: u8,
}

impl Symbol {
    /// Whether the symbol is external (in the link's symbol table).
    pub fn is_external(&self) -> bool {
        matches!(
            self.kind,
            SymbolKind::Defined { .. }
                | SymbolKind::Undefined
                | SymbolKind::Common(_)
                | SymbolKind::Weak { .. }
        )
    }
}

/// A COFF object, read.
#[derive(Debug)]
pub struct Object {
    /// The machine.
    pub machine: u16,
    /// The sections, in order (0-based; COFF numbers them from 1).
    pub sections: Vec<Section>,
    /// The symbols, one per symbol table record.
    pub symbols: Vec<Symbol>,
    /// For each COMDAT section, the symbol index of its leader (the first
    /// symbol after its section symbol in it); `u32::MAX` otherwise.
    pub comdat_leaders: Vec<u32>,
    /// The `.drectve` sections' options, in order.
    pub directives: Vec<Arg>,
    /// `@feat.00`'s value (`/SAFESEH`, `/GUARD:CF`, `/GUARD:EHCONT`
    /// compatibility bits), if present.
    pub feat_00: Option<u32>,
    /// `@comp.id`'s value: the tool that made the object (its product
    /// in the high 16 bits, its build in the low), for the Rich header.
    pub comp_id: Option<u32>,
}

/// An object that can't be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadObject(pub String);

impl Object {
    /// Reads the object `data` (a regular or `/bigobj` COFF object).
    pub fn parse(data: &'static [u8]) -> Result<Object, BadObject> {
        let bad = |what: &str| BadObject(what.to_owned());
        let file = PeFile::parse_object(data).map_err(|e| BadObject(e.to_string()))?;
        let headers = file.sections();
        let mut sections = Vec::with_capacity(headers.len());
        let mut directives = Vec::new();
        for h in headers {
            let characteristics = h.characteristics.get();
            let name = file.section_name(h);
            let size = h.size_of_raw_data.get();
            let contents = if characteristics & scn::CNT_UNINITIALIZED_DATA != 0 {
                &[][..]
            } else {
                turbonop_core::bytes_at(
                    data,
                    u64::from(h.pointer_to_raw_data.get()),
                    u64::from(size),
                )
                .map_err(|_| bad("section contents past the end of the file"))?
            };
            let mut count = u64::from(h.number_of_relocations.get());
            let mut at = u64::from(h.pointer_to_relocations.get());
            if characteristics & scn::LNK_NRELOC_OVFL != 0 && count == 0xFFFF {
                // The first record's address is the count, itself included.
                let first: &[Relocation] =
                    read_slice_at(data, at, 1).map_err(|_| bad("relocations"))?;
                count = u64::from(first[0].virtual_address.get()).saturating_sub(1);
                at += 10;
            }
            let relocations = read_slice_at(data, at, count).map_err(|_| {
                BadObject(format!(
                    "section {}: {count} relocations at {at:#x}, past the end",
                    String::from_utf8_lossy(name)
                ))
            })?;
            if name == b".drectve" && characteristics & scn::LNK_INFO != 0 {
                directives.extend(crate::args::directives(contents));
            }
            sections.push(Section {
                name,
                characteristics,
                data: contents,
                size,
                relocations,
                comdat: None,
            });
        }

        let table = file
            .symbols()
            .map_err(|_| bad("symbol table"))?
            .unwrap_or_default();
        let mut symbols = vec![
            Symbol {
                name: b"",
                kind: SymbolKind::Aux,
                ty: 0,
                class: 0,
                aux: 0,
            };
            table.len()
        ];
        let mut comdat_leaders = vec![u32::MAX; sections.len()];
        // Sections whose definition has been seen, and whose leader not yet.
        let mut awaiting_leader = vec![false; sections.len()];
        let mut feat_00 = None;
        let mut comp_id = None;
        let section_index = |n: i32| -> Option<u32> {
            let i = u32::try_from(n).ok()?.checked_sub(1)?;
            (usize::try_from(i).ok()? < headers.len()).then_some(i)
        };
        for s in table.iter() {
            let name = table.name(&s).unwrap_or_default();
            let index = s.index;
            let kind = match s.storage_class {
                class::EXTERNAL => match s.section {
                    0 if s.value == 0 => SymbolKind::Undefined,
                    0 => SymbolKind::Common(s.value),
                    -1 => SymbolKind::Absolute(s.value),
                    n => match section_index(n) {
                        Some(section) => SymbolKind::Defined {
                            section,
                            value: s.value,
                        },
                        None => return Err(bad("a symbol in a section that isn't there")),
                    },
                },
                class::WEAK_EXTERNAL => {
                    let aux = s
                        .aux()
                        .next()
                        .ok_or_else(|| bad("a weak external without its record"))?;
                    let u32_at =
                        |o: usize| u32::from_le_bytes([aux[o], aux[o + 1], aux[o + 2], aux[o + 3]]);
                    SymbolKind::Weak {
                        default: u32_at(0),
                        search: u32_at(4),
                    }
                }
                class::STATIC | class::LABEL | class::SECTION | class::END_OF_FUNCTION => {
                    match s.section {
                        -1 => {
                            if name == b"@feat.00" {
                                feat_00 = Some(s.value);
                            } else if name == b"@comp.id" {
                                comp_id = Some(s.value);
                            }
                            SymbolKind::Absolute(s.value)
                        }
                        n => match section_index(n) {
                            Some(section) => SymbolKind::Local {
                                section,
                                // A SECTION-class symbol's value is the
                                // section's characteristics, not an offset.
                                value: if s.storage_class == class::SECTION {
                                    0
                                } else {
                                    s.value
                                },
                            },
                            None => SymbolKind::Other,
                        },
                    }
                }
                _ => SymbolKind::Other,
            };
            // A section's definition: its COMDAT selection; the next
            // symbol in it is the COMDAT's leader.
            if let Some(def) = s.section_definition()
                && let Some(i) = section_index(s.section)
            {
                let i = i as usize;
                if sections[i].characteristics & scn::LNK_COMDAT != 0 {
                    let selection = match def.selection {
                        comdat_select::NODUPLICATES => Selection::NoDuplicates,
                        comdat_select::SAME_SIZE => Selection::SameSize,
                        comdat_select::EXACT_MATCH => Selection::ExactMatch,
                        comdat_select::ASSOCIATIVE => {
                            match def
                                .number
                                .checked_sub(1)
                                .filter(|&p| (p as usize) < headers.len())
                            {
                                Some(parent) => Selection::Associative(parent),
                                None => return Err(bad("an associative COMDAT of no section")),
                            }
                        }
                        comdat_select::LARGEST => Selection::Largest,
                        // ANY, and the unknown (as link.exe, any).
                        _ => Selection::Any,
                    };
                    sections[i].comdat = Some((selection, def.checksum));
                    awaiting_leader[i] = !matches!(selection, Selection::Associative(_));
                }
            } else if let SymbolKind::Defined { section, .. } | SymbolKind::Local { section, .. } =
                kind
                && awaiting_leader[section as usize]
            {
                awaiting_leader[section as usize] = false;
                comdat_leaders[section as usize] = u32::try_from(index).unwrap_or(u32::MAX);
            }
            symbols[index] = Symbol {
                name,
                kind,
                ty: s.kind,
                class: s.storage_class,
                aux: u8::try_from(s.aux_count()).unwrap_or(u8::MAX),
            };
        }
        Ok(Object {
            machine: file.machine(),
            sections,
            symbols,
            comdat_leaders,
            directives,
            feat_00,
            comp_id,
        })
    }
}

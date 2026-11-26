//! ELF relocatable objects as values: read an object into an editable
//! model, change it, and write it back.
//!
//! [`Object`] holds an object's header and its [`Section`]s, each with its
//! contents in a typed form where turbolib edits them ([`Data`]): symbol
//! tables ([`Symbol`]), relocations ([`Relocation`], `SHT_REL` and
//! `SHT_RELA`), COMDAT groups ([`Group`]), and extended section indexes
//! (`SHT_SYMTAB_SHNDX`, kept in step with the symbols); everything else is
//! bytes. Both classes and both byte orders are covered, and extended
//! numbering (more than 0xFF00 sections) in both directions.
//!
//! An object read and written back unchanged is the same file, byte for
//! byte, whichever tool wrote it: the reader keeps the file, and the writer
//! re-encodes every part into its place in a copy of it while every part
//! keeps its size and the section count is the same (so the padding and
//! any bytes between the parts are the original's). String tables are kept
//! as they were while every name is found at its offset in them, so a
//! table's order and its shared suffixes (which GNU `as` and LLVM's
//! assembler each make in their own way) survive. Otherwise the writer
//! lays the object out afresh: the header, the sections in file order,
//! each at its alignment, then the section headers ([`Object::relayout`]).
//!
//! Edits ([`Object::retain_sections`], [`Object::retain_symbols`]) renumber
//! every reference the model holds: section links and infos, groups'
//! members, symbols' sections, relocations' symbols. What they can't see:
//! references inside bytes (`.debug_*` sections name their sections only
//! through relocations, which are renumbered; `.ARM.exidx` and the like
//! name theirs through `sh_link`, which is too).

#![no_std]
// ELF fields are unpacked by masks and shifts, and the writer checks that a
// 32-bit object's values fit before it narrows them.
#![allow(clippy::cast_possible_truncation)]

#[cfg(test)]
use turbonop_global_alloc as _;

extern crate alloc;

mod read;
mod relocs;
mod write;

use alloc::borrow::Cow;
use alloc::vec::Vec;
use core::fmt;

pub use relocs::{
    CrelEntry, Form as RelocationForm, Iter as RelocationIter, Relocations, decode_crel,
    encode_crel,
};
pub use turbonop_core::Endian;
pub use turbonop_elf::Class;

/// `SHN_XINDEX`: the symbol's section index is in `SHT_SYMTAB_SHNDX`.
pub const SHN_XINDEX: u16 = 0xFFFF;
/// The first reserved section index.
pub const SHN_LORESERVE: u32 = 0xFF00;
/// `SHN_UNDEF`.
pub const SHN_UNDEF: u32 = 0;
/// `SHN_ABS`.
pub const SHN_ABS: u32 = 0xFFF1;
/// `SHN_COMMON`.
pub const SHN_COMMON: u32 = 0xFFF2;

/// Section types (`SHT_*`) the model decodes.
pub mod sht {
    pub const NULL: u32 = 0;
    pub const PROGBITS: u32 = 1;
    pub const SYMTAB: u32 = 2;
    pub const STRTAB: u32 = 3;
    pub const RELA: u32 = 4;
    pub const NOTE: u32 = 7;
    pub const NOBITS: u32 = 8;
    pub const REL: u32 = 9;
    pub const INIT_ARRAY: u32 = 14;
    pub const FINI_ARRAY: u32 = 15;
    pub const PREINIT_ARRAY: u32 = 16;
    pub const GROUP: u32 = 17;
    pub const SYMTAB_SHNDX: u32 = 18;
    pub const CREL: u32 = 0x4000_0014;
    pub const LLVM_ADDRSIG: u32 = 0x6FFF_4C03;
    pub const GNU_ATTRIBUTES: u32 = 0x6FFF_FFF5;
    pub const ARM_EXIDX: u32 = 0x7000_0001;
    pub const ARM_ATTRIBUTES: u32 = 0x7000_0003;
    pub const RISCV_ATTRIBUTES: u32 = 0x7000_0003;
    pub const MIPS_ABIFLAGS: u32 = 0x7000_002A;
    pub const X86_64_UNWIND: u32 = 0x7000_0001;
}

/// Section flags (`SHF_*`).
pub mod shf {
    pub const WRITE: u64 = 0x1;
    pub const ALLOC: u64 = 0x2;
    pub const EXECINSTR: u64 = 0x4;
    pub const MERGE: u64 = 0x10;
    pub const STRINGS: u64 = 0x20;
    pub const INFO_LINK: u64 = 0x40;
    pub const LINK_ORDER: u64 = 0x80;
    pub const GROUP: u64 = 0x200;
    pub const TLS: u64 = 0x400;
    pub const COMPRESSED: u64 = 0x800;
    pub const GNU_RETAIN: u64 = 0x20_0000;
    pub const EXCLUDE: u64 = 0x8000_0000;
}

/// The file header's fields the model keeps (the counts and offsets are
/// the layout's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// `e_ident`, all 16 bytes (class, byte order, version, OS ABI, ABI
    /// version, padding).
    pub ident: [u8; 16],
    /// `e_type` (`ET_REL`, 1, for an object).
    pub kind: u16,
    /// `e_machine`.
    pub machine: u16,
    /// `e_version`.
    pub version: u32,
    /// `e_entry`.
    pub entry: u64,
    /// `e_flags`.
    pub flags: u32,
}

impl Header {
    /// The class `e_ident` gives.
    pub fn class(&self) -> Class {
        if self.ident[4] == 1 {
            Class::Elf32
        } else {
            Class::Elf64
        }
    }

    /// The byte order `e_ident` gives.
    pub fn endian(&self) -> Endian {
        if self.ident[5] == 2 {
            Endian::Big
        } else {
            Endian::Little
        }
    }
}

/// A symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol<'a> {
    /// Its name.
    pub name: Cow<'a, [u8]>,
    /// Where its name was in the string table (kept while the name is
    /// still there).
    pub name_offset: u32,
    pub value: u64,
    pub size: u64,
    /// `st_info`: binding (high 4 bits) and type.
    pub info: u8,
    /// `st_other`: visibility (low 2 bits) and the rest.
    pub other: u8,
    /// Its section's index, or a special index (`SHN_UNDEF`, `SHN_ABS`,
    /// `SHN_COMMON`, a processor's).
    pub shndx: u32,
    /// Whether `st_shndx` is `SHN_XINDEX` (the index is in the extended
    /// table).
    pub xindex: bool,
}

impl Symbol<'_> {
    /// `STB_*`.
    pub fn bind(&self) -> u8 {
        self.info >> 4
    }

    /// `STT_*`.
    pub fn kind(&self) -> u8 {
        self.info & 0xF
    }

    /// `STV_*`.
    pub fn visibility(&self) -> u8 {
        self.other & 3
    }

    /// Whether `shndx` names a section (not undefined, absolute, common, or
    /// another special index).
    pub fn in_section(&self) -> bool {
        self.shndx != SHN_UNDEF && (self.xindex || self.shndx < SHN_LORESERVE)
    }

    /// Puts the symbol in section `index` (with `SHN_XINDEX` when the index
    /// is a reserved one's).
    pub fn set_section(&mut self, index: u32) {
        self.shndx = index;
        self.xindex = index >= SHN_LORESERVE;
    }

    /// Makes it a special index (`SHN_UNDEF`, `SHN_ABS`, `SHN_COMMON`).
    pub fn set_special(&mut self, index: u32) {
        self.shndx = index;
        self.xindex = false;
    }
}

/// A relocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Relocation {
    /// `r_offset`.
    pub offset: u64,
    /// `r_info`, as the file holds it (read in its byte order): use
    /// [`Object::reloc_symbol`] and [`Object::reloc_type`].
    pub info: u64,
    /// `r_addend` (`SHT_RELA` only).
    pub addend: i64,
}

/// A COMDAT group (`SHT_GROUP`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// Its flag word (`GRP_COMDAT`, 1).
    pub flags: u32,
    /// Its member sections' indexes.
    pub members: Vec<u32>,
}

/// A section's contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Data<'a> {
    /// Bytes (any type the model doesn't decode, and string tables).
    Bytes(Cow<'a, [u8]>),
    /// `SHT_NOBITS`: a size and no bytes.
    NoBits,
    /// `SHT_SYMTAB` (or `SHT_DYNSYM`): the symbols, index 0 included.
    Symbols(Vec<Symbol<'a>>),
    /// `SHT_REL` or `SHT_RELA`.
    Relocations(Relocations<'a>),
    /// `SHT_GROUP`.
    Group(Group),
    /// `SHT_SYMTAB_SHNDX`: written from the symbol table it names
    /// (`sh_link`).
    SymtabShndx,
}

/// A section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section<'a> {
    /// Its name.
    pub name: Cow<'a, [u8]>,
    /// Where its name was in the section-name string table.
    pub name_offset: u32,
    /// `sh_type`.
    pub kind: u32,
    pub flags: u64,
    pub addr: u64,
    pub link: u32,
    pub info: u32,
    pub addralign: u64,
    pub entsize: u64,
    /// The size of `SHT_NOBITS` (and of section 0, which holds an extended
    /// section count); others' sizes are their contents'.
    pub nobits_size: u64,
    pub data: Data<'a>,
}

impl Section<'_> {
    /// Whether it holds relocations (`SHT_REL`, `SHT_RELA`, or `SHT_CREL`).
    pub fn is_relocations(&self) -> bool {
        matches!(self.kind, sht::REL | sht::RELA | sht::CREL)
    }
}

/// Where the parts of the object read were, for writing it back the same.
#[derive(Clone, Debug)]
struct Original<'a> {
    file: &'a [u8],
    /// Each section's offset and size in the file.
    sections: Vec<(u64, u64)>,
    shoff: u64,
    /// `e_shnum` and `e_shstrndx` as they were.
    shnum: u16,
    shstrndx: u16,
    /// `e_phoff` and `e_phentsize` (with no program headers, whatever
    /// the writer left there).
    phoff: u64,
    phentsize: u16,
}

/// An ELF relocatable object.
#[derive(Clone, Debug)]
pub struct Object<'a> {
    pub header: Header,
    /// Every section, section 0 included.
    pub sections: Vec<Section<'a>>,
    /// The section-name string table's index.
    pub shstrndx: u32,
    original: Option<Original<'a>>,
}

/// Why an object can't be read into the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Not ELF, or a class or byte order the header doesn't give.
    NotElf,
    /// Program headers, or header sizes not the standard ones.
    Unsupported(&'static str),
    /// A part that isn't inside the file, or doesn't decode.
    Malformed(&'static str),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::NotElf => f.write_str("not an ELF file"),
            ReadError::Unsupported(what) => write!(f, "unsupported: {what}"),
            ReadError::Malformed(what) => write!(f, "malformed: {what}"),
        }
    }
}

impl core::error::Error for ReadError {}

/// Why an object can't be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// A value doesn't fit a 32-bit object's field.
    TooLarge(&'static str),
    /// A section index, link, or symbol that isn't there.
    BadReference(&'static str),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::TooLarge(what) => write!(f, "{what} doesn't fit"),
            WriteError::BadReference(what) => write!(f, "{what} names nothing"),
        }
    }
}

impl core::error::Error for WriteError {}

impl<'a> Object<'a> {
    /// An object of `sections` (section 0 included), laid out afresh when
    /// written; `shstrndx` is to be set.
    pub fn new(header: Header, sections: Vec<Section<'a>>) -> Self {
        Self {
            header,
            sections,
            shstrndx: 0,
            original: None,
        }
    }

    /// Reads an object (any ELF file without program headers).
    pub fn parse(data: &'a [u8]) -> Result<Self, ReadError> {
        read::parse(data)
    }

    /// Its class.
    pub fn class(&self) -> Class {
        self.header.class()
    }

    /// Its byte order.
    pub fn endian(&self) -> Endian {
        self.header.endian()
    }

    /// Whether `r_info` packs MIPS64's form (a 32-bit symbol, then `r_ssym`
    /// and three types).
    fn mips64(&self) -> bool {
        self.header.machine == 8 && self.class() == Class::Elf64
    }

    /// A relocation's symbol index (of a relocation or a reference to one).
    pub fn reloc_symbol(&self, r: impl core::borrow::Borrow<Relocation>) -> u32 {
        let r = r.borrow();
        match (self.class(), self.mips64(), self.endian()) {
            (Class::Elf32, ..) => (r.info >> 8) as u32 & 0x00FF_FFFF,
            (Class::Elf64, true, Endian::Little) => (r.info & 0xFFFF_FFFF) as u32,
            (Class::Elf64, ..) => (r.info >> 32) as u32,
        }
    }

    /// A relocation's type (MIPS64's three, packed as `r_info`'s low 32
    /// bits hold them: the first in the low byte).
    pub fn reloc_type(&self, r: impl core::borrow::Borrow<Relocation>) -> u32 {
        let r = r.borrow();
        match (self.class(), self.mips64(), self.endian()) {
            (Class::Elf32, ..) => (r.info & 0xFF) as u32,
            (Class::Elf64, true, Endian::Little) => {
                let hi = (r.info >> 32) as u32;
                // Bytes: ssym, type3, type2, type.
                let [_, t3, t2, t] = hi.to_le_bytes();
                u32::from_le_bytes([t, t2, t3, 0])
            }
            (Class::Elf64, true, Endian::Big) => {
                let [_, t3, t2, t] = ((r.info & 0xFFFF_FFFF) as u32).to_be_bytes();
                u32::from_le_bytes([t, t2, t3, 0])
            }
            (Class::Elf64, false, _) => (r.info & 0xFFFF_FFFF) as u32,
        }
    }

    /// Gives a relocation another symbol, its type kept.
    pub fn set_reloc_symbol(&self, r: &mut Relocation, symbol: u32) {
        match (self.class(), self.mips64(), self.endian()) {
            (Class::Elf32, ..) => r.info = (r.info & 0xFF) | (u64::from(symbol) << 8),
            (Class::Elf64, true, Endian::Little) => {
                r.info = (r.info & 0xFFFF_FFFF_0000_0000) | u64::from(symbol);
            }
            (Class::Elf64, ..) => r.info = (r.info & 0xFFFF_FFFF) | (u64::from(symbol) << 32),
        }
    }

    /// The `r_info` of a CREL entry's symbol and type, as a `REL` entry
    /// of this object holds them (`None` for MIPS64's packing, or a 32-bit
    /// object's type past a byte).
    pub fn crel_info(&self, symbol: u32, kind: u32) -> Option<u64> {
        match (self.class(), self.mips64()) {
            (_, true) => None,
            (Class::Elf32, _) => {
                (kind <= 0xFF && symbol <= 0x00FF_FFFF).then(|| u64::from(symbol << 8 | kind))
            }
            (Class::Elf64, _) => Some(u64::from(symbol) << 32 | u64::from(kind)),
        }
    }

    /// How this object encodes `SHT_RELA` (`rela`) or `SHT_REL` entries.
    pub fn relocation_form(&self, rela: bool) -> RelocationForm {
        RelocationForm {
            class: self.class(),
            endian: self.endian(),
            rela,
        }
    }

    /// The symbol table's index (the first `SHT_SYMTAB`), if any.
    pub fn symtab(&self) -> Option<usize> {
        self.sections.iter().position(|s| s.kind == sht::SYMTAB)
    }

    /// The symbols of the symbol table, if any.
    pub fn symbols(&self) -> &[Symbol<'a>] {
        match self.symtab().map(|i| &self.sections[i].data) {
            Some(Data::Symbols(s)) => s,
            _ => &[],
        }
    }

    /// The symbols of the symbol table, to change.
    pub fn symbols_mut(&mut self) -> Option<&mut Vec<Symbol<'a>>> {
        let i = self.symtab()?;
        match &mut self.sections[i].data {
            Data::Symbols(s) => Some(s),
            _ => None,
        }
    }

    /// Whether the object will be written in the layout it was read with.
    pub fn keeps_layout(&self) -> bool {
        write::keeps_layout(self)
    }

    /// Forgets the layout read, so the next write lays the object out
    /// afresh.
    pub fn relayout(&mut self) {
        self.original = None;
    }

    /// The object's bytes.
    pub fn write(&self) -> Result<Vec<u8>, WriteError> {
        write::write(self)
    }

    /// Keeps the sections `keep` says (section 0 always), renumbering every
    /// reference: links, infos (of `SHF_INFO_LINK` sections and relocation
    /// sections), group members (a removed member leaves its group),
    /// symbols' sections (a symbol in a removed section becomes undefined:
    /// the caller removes or rewrites it), and `shstrndx`. Returns each old
    /// index's new one (`None`: removed).
    pub fn retain_sections(&mut self, keep: &[bool]) -> Vec<Option<u32>> {
        let mut map = Vec::with_capacity(self.sections.len());
        let mut next = 0u32;
        for i in 0..self.sections.len() {
            if i == 0 || keep.get(i).copied().unwrap_or(true) {
                map.push(Some(next));
                next += 1;
            } else {
                map.push(None);
            }
        }
        let renumber = |i: u32| -> Option<u32> { map.get(i as usize).copied().flatten() };
        let old = core::mem::take(&mut self.sections);
        for (i, mut s) in old.into_iter().enumerate() {
            if map[i].is_none() {
                continue;
            }
            if s.link != 0 {
                s.link = renumber(s.link).unwrap_or(0);
            }
            let info_is_section = s.flags & shf::INFO_LINK != 0 || s.is_relocations();
            if info_is_section && s.info != 0 {
                s.info = renumber(s.info).unwrap_or(0);
            }
            match &mut s.data {
                Data::Group(g) => {
                    g.members = g.members.iter().filter_map(|&m| renumber(m)).collect();
                }
                Data::Symbols(symbols) => {
                    for sym in symbols.iter_mut() {
                        if sym.in_section() {
                            match renumber(sym.shndx) {
                                Some(n) => sym.set_section(n),
                                None => sym.set_special(SHN_UNDEF),
                            }
                        }
                    }
                }
                _ => {}
            }
            self.sections.push(s);
        }
        self.shstrndx = renumber(self.shstrndx).unwrap_or(0);
        self.original = None;
        map
    }

    /// Keeps the symbols `keep` says (symbol 0 always) of the symbol table,
    /// renumbering relocations' symbols and groups' signatures
    /// (`sh_info`); a relocation naming a removed symbol gets symbol 0.
    /// Sets the table's `sh_info` (its first global) from the bindings.
    /// Returns each old index's new one.
    pub fn retain_symbols(&mut self, keep: &[bool]) -> Vec<Option<u32>> {
        let Some(t) = self.symtab() else {
            return Vec::new();
        };
        let Data::Symbols(symbols) = &mut self.sections[t].data else {
            return Vec::new();
        };
        let mut map = Vec::with_capacity(symbols.len());
        let mut next = 0u32;
        let mut kept = Vec::with_capacity(symbols.len());
        for (i, s) in core::mem::take(symbols).into_iter().enumerate() {
            if i == 0 || keep.get(i).copied().unwrap_or(true) {
                map.push(Some(next));
                next += 1;
                kept.push(s);
            } else {
                map.push(None);
            }
        }
        let first_global = kept
            .iter()
            .position(|s| s.bind() != 0)
            .unwrap_or(kept.len());
        *symbols = kept;
        self.sections[t].info = u32::try_from(first_global).unwrap_or(u32::MAX);
        self.renumber_symbols(t, &map);
        self.original = None;
        map
    }

    /// Renumbers every reference to the symbols of table `t` by `map`.
    pub fn renumber_symbols(&mut self, t: usize, map: &[Option<u32>]) {
        let t32 = u32::try_from(t).unwrap_or(u32::MAX);
        let renumber = |i: u32| map.get(i as usize).copied().flatten().unwrap_or(0);
        let (class, mips64, endian) = (self.class(), self.mips64(), self.endian());
        let probe = Object {
            header: self.header,
            sections: Vec::new(),
            shstrndx: 0,
            original: None,
        };
        debug_assert!(
            probe.class() == class && probe.mips64() == mips64 && probe.endian() == endian
        );
        for s in &mut self.sections {
            if s.link != t32 {
                continue;
            }
            match &mut s.data {
                Data::Relocations(rs) => rs.map_info(|info| {
                    let mut r = Relocation {
                        offset: 0,
                        info,
                        addend: 0,
                    };
                    let sym = probe.reloc_symbol(&r);
                    probe.set_reloc_symbol(&mut r, renumber(sym));
                    r.info
                }),
                Data::Group(_) => s.info = renumber(s.info),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests;

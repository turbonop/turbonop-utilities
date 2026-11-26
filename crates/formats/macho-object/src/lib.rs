//! Mach-O relocatable objects (`MH_OBJECT`) as values: read an object into
//! an editable model, change it, and write it back.
//!
//! [`Object`] holds an object's [`Header`], its load commands in file order
//! ([`Command`]), and the tables the commands locate: segments with their
//! [`Section`]s (contents, or a zero-fill size, and each section's
//! [`Relocation`]s, plain and scattered), the symbol table ([`Symbol`]s
//! with their names), the dynamic symbol table's ranges ([`Dysymtab`]) and
//! indirect symbol table, and link-edit data (`LC_DATA_IN_CODE`,
//! `LC_LINKER_OPTIMIZATION_HINT`, any other `linkedit_data_command`) as
//! bytes. Every other command (`LC_BUILD_VERSION`, `LC_VERSION_MIN_*`,
//! `LC_LINKER_OPTION`, unknown ones) is kept as its bytes. Both widths and
//! both byte orders are covered.
//!
//! An object read and written back unchanged is the same file, byte for
//! byte: the reader keeps the file, and the writer re-encodes every part
//! into its place in a copy of it while every part keeps its size (so the
//! padding and any bytes between the parts are the original's). The string
//! table is kept as it was while every symbol's name is found at its
//! offset in it. Otherwise the writer lays the object out afresh, as
//! LLVM's `MachObjectWriter` does: the header, the load commands, the
//! sections' contents at their alignments (zero-fill sections last), the
//! relocations section by section, the link-edit data, the indirect
//! symbols, the symbols, and a string table built as LLVM's
//! `StringTableBuilder` builds one (names sorted by their reversed bytes,
//! suffixes shared, padded to the pointer size) ([`Object::relayout`]).
//!
//! Edits ([`Object::retain_sections`], [`Object::retain_symbols`],
//! [`Object::sort_symbols`]) renumber every reference the model holds:
//! symbols' sections (`n_sect`), relocations' symbols (`r_symbolnum` of an
//! external relocation) and sections (of a local one: section ordinals,
//! from 1), indirect symbols, and the dynamic symbol table's ranges; and
//! they drop the data-in-code entries and optimization hints in removed
//! sections.
//!
//! In an object a section's address is its offset in the one segment, and
//! the bytes hold addresses the model doesn't decode: a local relocation
//! (`r_extern` 0) leaves its target's address in the section's bytes (the
//! debug sections' relocations, `__DWARF`'s, are local whenever they can
//! be; so are i386's scattered ones), and `__eh_frame`'s FDEs hold their
//! functions' and LSDAs' addresses as PC-relative differences LLVM
//! resolves itself, with no relocation. Data-in-code entries and
//! optimization hints hold addresses too. [`Object::assign_addresses`]
//! gives the sections LLVM's addresses (after an edit, the ones LLVM would
//! have given them) and moves everything with them: symbols' values,
//! scattered relocations' values, link-edit data, `__eh_frame`'s pointers,
//! and the relocations' encoded addresses where [`reloc`] knows the
//! encoding (x86_64, arm64, arm64_32, i386; not arm's or PowerPC's
//! instruction fields), listing the relocations it couldn't. What it
//! can't see: an address in bytes with no relocation and outside
//! `__eh_frame`, which LLVM doesn't write; a removed section's FDEs and
//! indirect symbol entries, which stay (the caller removes them; see
//! [`ehframe`]); and what an edit does to the code itself.
//!
//! [`Object::cut`] removes byte ranges from inside sections (atoms, for
//! folding) by the same means: one map of old addresses to new ones moves
//! relocations' places and encoded targets, symbols, `__eh_frame`'s
//! pointers and its FDEs' links to their CIEs, and link-edit data, and
//! reports what was in the ranges. It is sound for ranges that are whole
//! atoms (an atom's symbol to the next one's): the references within a
//! section that the assembler resolved itself, with no relocation (a
//! function's branches to its own labels), stay within an atom, which
//! moves as one. A range that splits an atom moves such references'
//! targets unseen; and cut only moves bytes, it doesn't pad: what follows
//! a range whose length isn't a multiple of its alignment is reported
//! ([`Cut::misaligned`]).

#![no_std]
// Mach-O fields are unpacked by masks and shifts, and the writer checks
// that a 32-bit object's values fit before it narrows them.
#![allow(clippy::cast_possible_truncation)]

#[cfg(test)]
use turbonop_global_alloc as _;

extern crate alloc;

mod cut;
pub mod ehframe;
pub mod linkedit;
mod read;
pub mod reloc;
mod write;

use alloc::borrow::Cow;
use alloc::vec::Vec;
use core::fmt;

pub use cut::{Cut, CutError};
pub use turbonop_core::Endian;
pub use turbonop_macho::consts;
pub use write::llvm_string_table;

use consts::*;

/// `INDIRECT_SYMBOL_LOCAL`: an indirect symbol entry for a local symbol
/// (no index).
pub const INDIRECT_SYMBOL_LOCAL: u32 = 0x8000_0000;
/// `INDIRECT_SYMBOL_ABS`: an indirect symbol entry for an absolute symbol.
pub const INDIRECT_SYMBOL_ABS: u32 = 0x4000_0000;
/// `MH_SUBSECTIONS_VIA_SYMBOLS`: the sections can be split at symbols.
pub const MH_SUBSECTIONS_VIA_SYMBOLS: u32 = 0x2000;
/// `N_INDR`: a symbol that is another's alias (its value is the other's
/// name's offset in the string table).
pub const N_INDR: u8 = 0xA;
/// `NO_SECT`: a symbol in no section.
pub const NO_SECT: u8 = 0;

/// The file header's fields the model keeps (the command count and size
/// are the layout's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// `MH_MAGIC` or `MH_MAGIC_64`, as read in the file's byte order.
    pub magic: u32,
    /// The byte order.
    pub endian: Endian,
    pub cputype: u32,
    pub cpusubtype: u32,
    /// `MH_OBJECT`.
    pub filetype: u32,
    /// `MH_SUBSECTIONS_VIA_SYMBOLS` and the like.
    pub flags: u32,
    /// The 64-bit header's last word (0).
    pub reserved: u32,
}

impl Header {
    /// Whether it is a 64-bit object (`MH_MAGIC_64`).
    pub fn is_64(&self) -> bool {
        self.magic == MH_MAGIC_64
    }

    /// The header's size: 28 bytes, or 32 for a 64-bit object.
    pub fn size(&self) -> u64 {
        if self.is_64() { 32 } else { 28 }
    }
}

/// A relocation entry: a plain one (`relocation_info`), or a scattered one
/// (`scattered_relocation_info`, 32-bit targets' only), which has `value`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Relocation {
    /// The offset in its section of what it relocates (`r_address`: 24
    /// bits in a scattered entry; in some pairs' second entry, a value).
    pub address: u32,
    /// A plain entry's `r_symbolnum` (24 bits): if `external`, a symbol's
    /// index; else a section's ordinal (from 1; `R_ABS`, 0, for none), or,
    /// for the second of some pairs (`ARM64_RELOC_ADDEND`), an addend. A
    /// scattered entry's is 0.
    pub symbolnum: u32,
    /// Relative to the PC (`r_pcrel`).
    pub pcrel: bool,
    /// Its length, as a power of two: 0 a byte, 1 two, 2 four, 3 eight
    /// (`r_length`).
    pub length: u8,
    /// Whether `symbolnum` is a symbol's (`r_extern`; plain entries only).
    pub external: bool,
    /// Its type (`r_type`, 4 bits): see [`reloc`].
    pub kind: u8,
    /// A scattered entry's `r_value`: the address it refers to.
    pub value: Option<u32>,
}

impl Relocation {
    /// A plain entry.
    pub fn plain(address: u32, symbolnum: u32, external: bool, kind: u8, length: u8) -> Self {
        Self {
            address,
            symbolnum,
            pcrel: false,
            length,
            external,
            kind,
            value: None,
        }
    }

    /// Whether it is a scattered entry.
    pub fn is_scattered(&self) -> bool {
        self.value.is_some()
    }

    /// Whether it is a plain, local entry (`r_extern` 0): its `symbolnum`
    /// is a section's ordinal, `R_ABS`, or, for some pairs' second entry,
    /// something else ([`reloc::names_section`] tells which).
    pub fn is_local(&self) -> bool {
        self.value.is_none() && !self.external
    }
}

/// A section's contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Contents<'a> {
    /// Its bytes.
    Bytes(Cow<'a, [u8]>),
    /// A zero-fill section (`S_ZEROFILL`, `S_GB_ZEROFILL`,
    /// `S_THREAD_LOCAL_ZEROFILL`): its size, and no bytes in the file.
    Zerofill(u64),
}

/// A section (`section` or `section_64`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section<'a> {
    /// `sectname`, all 16 bytes (the name, then NULs).
    pub sectname: [u8; 16],
    /// `segname`, all 16 bytes.
    pub segname: [u8; 16],
    /// Its address (in an object, its offset in the one segment).
    pub addr: u64,
    /// Its alignment, as a power of two.
    pub align: u32,
    /// Its type (the low byte) and attributes.
    pub flags: u32,
    /// For symbol pointer and stub sections, the index of its first entry
    /// in the indirect symbol table.
    pub reserved1: u32,
    /// For stub sections, a stub's size.
    pub reserved2: u32,
    /// The 64-bit section's last word.
    pub reserved3: u32,
    pub contents: Contents<'a>,
    /// Its relocations, in file order.
    pub relocations: Vec<Relocation>,
}

/// A fixed-size name field without its padding.
fn name16(b: &[u8; 16]) -> &[u8] {
    let n = b.iter().position(|&c| c == 0).unwrap_or(16);
    &b[..n]
}

/// A 16-byte name field holding `name` (cut at 16 bytes).
pub fn name_field(name: &[u8]) -> [u8; 16] {
    let mut f = [0u8; 16];
    let n = name.len().min(16);
    f[..n].copy_from_slice(&name[..n]);
    f
}

impl Section<'_> {
    /// Its name (`__text`), without the padding.
    pub fn name(&self) -> &[u8] {
        name16(&self.sectname)
    }

    /// Its segment's name (`__TEXT`).
    pub fn segment_name(&self) -> &[u8] {
        name16(&self.segname)
    }

    /// Its type (`S_*`, the low byte of its flags).
    pub fn kind(&self) -> u32 {
        self.flags & SECTION_TYPE
    }

    /// Whether its type is a zero-fill one (LLVM's virtual sections, laid
    /// out after the others).
    pub fn is_zerofill_kind(&self) -> bool {
        is_zerofill(self.flags)
    }

    /// Its size in memory.
    pub fn size(&self) -> u64 {
        match &self.contents {
            Contents::Bytes(b) => b.len() as u64,
            Contents::Zerofill(n) => *n,
        }
    }

    /// Its bytes (none for zero-fill).
    pub fn bytes(&self) -> Option<&[u8]> {
        match &self.contents {
            Contents::Bytes(b) => Some(b),
            Contents::Zerofill(_) => None,
        }
    }

    /// Whether it holds debug information (`S_ATTR_DEBUG`, the `__DWARF`
    /// segment's sections).
    pub fn is_debug(&self) -> bool {
        self.flags & S_ATTR_DEBUG != 0
    }
}

/// Whether section flags give a zero-fill type.
pub fn is_zerofill(flags: u32) -> bool {
    matches!(
        flags & SECTION_TYPE,
        S_ZEROFILL | S_GB_ZEROFILL | S_THREAD_LOCAL_ZEROFILL
    )
}

/// A segment (`LC_SEGMENT` or `LC_SEGMENT_64`, by the object's width) and
/// its sections. Its size and file range are the layout's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment<'a> {
    /// `segname`, all 16 bytes (an object's one segment's is empty).
    pub segname: [u8; 16],
    /// Its address (0).
    pub vmaddr: u64,
    pub maxprot: u32,
    pub initprot: u32,
    pub flags: u32,
    pub sections: Vec<Section<'a>>,
}

/// A symbol (`nlist` or `nlist_64`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol<'a> {
    /// Its name.
    pub name: Cow<'a, [u8]>,
    /// Where its name was in the string table (`n_strx`; kept while the
    /// table is).
    pub name_offset: u32,
    /// `n_type`: `N_STAB` bits, `N_PEXT`, the type (`N_TYPE`), `N_EXT`.
    pub n_type: u8,
    /// `n_sect`: its section's ordinal (from 1), or `NO_SECT`.
    pub n_sect: u8,
    /// `n_desc`: reference type, `N_WEAK_DEF` and the like (a common
    /// symbol's alignment).
    pub n_desc: u16,
    /// `n_value`: its address (a common symbol's size; an alias's, its
    /// target's name's offset in the string table).
    pub n_value: u64,
    /// For an alias (`N_INDR`), the name of the symbol it is an alias of;
    /// the writer sets `n_value` from it when it builds the string table.
    pub indirect: Option<Cow<'a, [u8]>>,
}

/// Where a symbol goes in the dynamic symbol table's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SymbolClass {
    /// A local (not `N_EXT`, or a debugger entry).
    Local,
    /// An external definition (`N_EXT`, defined).
    External,
    /// An undefined or common symbol (`N_EXT` and `N_UNDF`), or an alias
    /// (`N_EXT` and `N_INDR`): LLVM's undefined symbols.
    Undefined,
}

impl Symbol<'_> {
    /// Whether it is a debugger (STABS) entry.
    pub fn is_stab(&self) -> bool {
        self.n_type & N_STAB != 0
    }

    /// Whether `N_EXT` is set (and it isn't a debugger entry).
    pub fn is_external(&self) -> bool {
        !self.is_stab() && self.n_type & N_EXT != 0
    }

    /// Whether it is undefined (or common: `N_UNDF` with a size).
    pub fn is_undefined(&self) -> bool {
        !self.is_stab() && self.n_type & N_TYPE == N_UNDF
    }

    /// Whether it is defined in a section (`N_SECT`).
    pub fn in_section(&self) -> bool {
        !self.is_stab() && self.n_type & N_TYPE == N_SECT
    }

    /// Where it goes in the dynamic symbol table's order.
    pub fn class(&self) -> SymbolClass {
        if !self.is_external() {
            SymbolClass::Local
        } else if self.is_undefined() || self.n_type & N_TYPE == N_INDR {
            SymbolClass::Undefined
        } else {
            SymbolClass::External
        }
    }
}

/// The dynamic symbol table's ranges (`LC_DYSYMTAB`): locals, external
/// definitions, and undefined symbols, each a run of the symbol table. Its
/// other tables (indirect symbols aside, which [`Object`] holds) aren't in
/// objects.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Dysymtab {
    pub ilocalsym: u32,
    pub nlocalsym: u32,
    pub iextdefsym: u32,
    pub nextdefsym: u32,
    pub iundefsym: u32,
    pub nundefsym: u32,
}

/// A load command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    /// `LC_SEGMENT` or `LC_SEGMENT_64`.
    Segment(Segment<'a>),
    /// `LC_SYMTAB`: the symbols and strings are [`Object::symbols`] and
    /// [`Object::strings`].
    Symtab,
    /// `LC_DYSYMTAB`: its ranges; the indirect symbols are
    /// [`Object::indirect_symbols`].
    Dysymtab(Dysymtab),
    /// A `linkedit_data_command` (`LC_DATA_IN_CODE`,
    /// `LC_LINKER_OPTIMIZATION_HINT`, ...): the command and its data.
    LinkeditData { cmd: u32, data: Cow<'a, [u8]> },
    /// Any other command: its `cmd` and the bytes after `cmdsize`.
    Other { cmd: u32, body: Cow<'a, [u8]> },
}

impl Command<'_> {
    /// Its `cmd` (`LC_SEGMENT` for a segment whichever the width).
    pub fn cmd(&self) -> u32 {
        match self {
            Command::Segment(_) => LC_SEGMENT,
            Command::Symtab => LC_SYMTAB,
            Command::Dysymtab(_) => LC_DYSYMTAB,
            Command::LinkeditData { cmd, .. } | Command::Other { cmd, .. } => *cmd,
        }
    }
}

/// Where a section's parts were in the file read.
#[derive(Clone, Copy, Debug)]
struct PlacedSection {
    offset: u32,
    /// Its size in the file (0 for zero-fill).
    size: u64,
    reloff: u32,
    nreloc: u32,
}

/// Where a command's parts were in the file read.
#[derive(Clone, Debug)]
enum Placed {
    Segment {
        vmsize: u64,
        fileoff: u64,
        filesize: u64,
        sections: Vec<PlacedSection>,
    },
    Symtab {
        symoff: u32,
        nsyms: u32,
        stroff: u32,
        strsize: u32,
    },
    /// All 18 words after `cmdsize`.
    Dysymtab([u32; 18]),
    Linkedit {
        dataoff: u32,
        datasize: u32,
    },
    Other,
}

/// Where the parts of the object read were, for writing it back the same.
#[derive(Clone, Debug)]
struct Original<'a> {
    file: &'a [u8],
    /// Each command's offset in the file and its `cmdsize`.
    commands: Vec<(u64, u32)>,
    placed: Vec<Placed>,
}

/// A Mach-O relocatable object.
#[derive(Clone, Debug)]
pub struct Object<'a> {
    pub header: Header,
    /// The load commands, in file order.
    pub commands: Vec<Command<'a>>,
    /// The symbol table (`LC_SYMTAB`'s), in file order.
    pub symbols: Vec<Symbol<'a>>,
    /// The string table read, written back while every symbol's name is at
    /// its offset in it; `None` to build one afresh, as LLVM does.
    pub strings: Option<Cow<'a, [u8]>>,
    /// The indirect symbol table (`LC_DYSYMTAB`'s): symbol indexes, or
    /// `INDIRECT_SYMBOL_LOCAL` and `INDIRECT_SYMBOL_ABS`.
    pub indirect_symbols: Vec<u32>,
    original: Option<Original<'a>>,
}

/// Why an object can't be read into the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// Not a thin Mach-O file.
    NotMachO,
    /// Not an object, or tables objects don't have.
    Unsupported(&'static str),
    /// A part that isn't inside the file, or doesn't decode.
    Malformed(&'static str),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReadError::NotMachO => f.write_str("not a Mach-O file"),
            ReadError::Unsupported(what) => write!(f, "unsupported: {what}"),
            ReadError::Malformed(what) => write!(f, "malformed: {what}"),
        }
    }
}

impl core::error::Error for ReadError {}

/// Why an object can't be written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteError {
    /// A value doesn't fit its field (a 32-bit object's, a 24-bit index,
    /// a 32-bit offset).
    TooLarge(&'static str),
}

impl fmt::Display for WriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::TooLarge(what) => write!(f, "{what} doesn't fit"),
        }
    }
}

impl core::error::Error for WriteError {}

/// A relocation whose encoded address [`Object::assign_addresses`] could
/// not rewrite (an encoding [`reloc`] doesn't handle), though a section it
/// involves moved; or an `__eh_frame` section whose records don't decode
/// ([`ehframe::pointers`]), or hold a pointer of an encoding it doesn't
/// move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unadjusted {
    /// Its section's index (from 0: the ordinal less 1).
    pub section: usize,
    /// Its index in the section's relocations (`None`: the section's
    /// `__eh_frame` records).
    pub relocation: Option<usize>,
}

impl<'a> Object<'a> {
    /// An object of `commands`, with no symbols, laid out afresh when
    /// written.
    pub fn new(header: Header, commands: Vec<Command<'a>>) -> Self {
        Self {
            header,
            commands,
            symbols: Vec::new(),
            strings: None,
            indirect_symbols: Vec::new(),
            original: None,
        }
    }

    /// Reads an object (`MH_OBJECT`, not a universal file: see
    /// `turbonop_macho::fat_archs`).
    pub fn parse(data: &'a [u8]) -> Result<Self, ReadError> {
        read::parse(data)
    }

    /// Whether it is a 64-bit object.
    pub fn is_64(&self) -> bool {
        self.header.is_64()
    }

    /// Its byte order.
    pub fn endian(&self) -> Endian {
        self.header.endian
    }

    /// The sections, in file order (each one's ordinal is its index plus
    /// 1).
    pub fn sections(&self) -> impl Iterator<Item = &Section<'a>> {
        self.commands.iter().flat_map(|c| match c {
            Command::Segment(s) => s.sections.as_slice(),
            _ => &[],
        })
    }

    /// The sections, to change.
    pub fn sections_mut(&mut self) -> impl Iterator<Item = &mut Section<'a>> {
        self.commands.iter_mut().flat_map(|c| match c {
            Command::Segment(s) => s.sections.as_mut_slice(),
            _ => &mut [],
        })
    }

    /// The section of index `i` (its ordinal less 1).
    pub fn section(&self, i: usize) -> Option<&Section<'a>> {
        self.sections().nth(i)
    }

    /// The address local relocation `relocation` of section `section` (by
    /// index) encodes in the section's bytes, where the encoding is plain
    /// ([`reloc::local_address`]).
    pub fn local_address(&self, section: usize, relocation: usize) -> Option<u64> {
        let s = self.section(section)?;
        reloc::local_address(self.header.cputype, self.endian(), s, relocation)
    }

    /// The section count.
    pub fn section_count(&self) -> usize {
        self.sections().count()
    }

    /// The dynamic symbol table's ranges, if there is one.
    pub fn dysymtab(&self) -> Option<&Dysymtab> {
        self.commands.iter().find_map(|c| match c {
            Command::Dysymtab(d) => Some(d),
            _ => None,
        })
    }

    /// The data of the first link-edit data command `cmd`, if any.
    pub fn linkedit_data(&self, cmd: u32) -> Option<&[u8]> {
        self.commands.iter().find_map(|c| match c {
            Command::LinkeditData { cmd: c, data } if *c == cmd => Some(&**data),
            _ => None,
        })
    }

    /// Whether the object will be written in the layout it was read with.
    pub fn keeps_layout(&self) -> bool {
        write::keeps_layout(self)
    }

    /// Forgets the layout and the string table read, so the next write
    /// lays the object out afresh and builds its string table as LLVM
    /// does. The sections keep their addresses ([`Self::assign_addresses`]
    /// gives them LLVM's).
    pub fn relayout(&mut self) {
        self.original = None;
        self.strings = None;
    }

    /// The object's bytes.
    pub fn write(&self) -> Result<Vec<u8>, WriteError> {
        write::write(self)
    }

    /// The dynamic symbol table's ranges for the symbols as they are:
    /// each class's count, the classes in their order. They describe the
    /// table only if it is in that order ([`Self::symbols_in_order`]).
    pub fn computed_dysymtab(&self) -> Dysymtab {
        let count = |c| {
            u32::try_from(self.symbols.iter().filter(|s| s.class() == c).count())
                .unwrap_or(u32::MAX)
        };
        let nlocalsym = count(SymbolClass::Local);
        let nextdefsym = count(SymbolClass::External);
        Dysymtab {
            ilocalsym: 0,
            nlocalsym,
            iextdefsym: nlocalsym,
            nextdefsym,
            iundefsym: nlocalsym + nextdefsym,
            nundefsym: count(SymbolClass::Undefined),
        }
    }

    /// Whether the symbols are in the dynamic symbol table's order: the
    /// locals, the external definitions, then the undefined symbols.
    pub fn symbols_in_order(&self) -> bool {
        self.symbols.is_sorted_by_key(Symbol::class)
    }

    /// Sets the dynamic symbol table's ranges (if there is one) from the
    /// symbols ([`Self::computed_dysymtab`]).
    pub fn update_dysymtab(&mut self) {
        let d = self.computed_dysymtab();
        for c in &mut self.commands {
            if let Command::Dysymtab(r) = c {
                *r = d;
            }
        }
    }

    /// Renumbers every reference to the symbols by `map` (each old index's
    /// new one): external relocations' `r_symbolnum` (a removed symbol's
    /// become 0) and indirect symbols (a removed symbol's become
    /// `INDIRECT_SYMBOL_LOCAL`).
    pub fn renumber_symbols(&mut self, map: &[Option<u32>]) {
        let renumber = |i: u32| map.get(i as usize).copied().flatten();
        for s in self.sections_mut() {
            for r in &mut s.relocations {
                if r.value.is_none() && r.external {
                    r.symbolnum = renumber(r.symbolnum).unwrap_or(0);
                }
            }
        }
        for i in &mut self.indirect_symbols {
            if *i & (INDIRECT_SYMBOL_LOCAL | INDIRECT_SYMBOL_ABS) == 0 {
                *i = renumber(*i).unwrap_or(INDIRECT_SYMBOL_LOCAL);
            }
        }
    }

    /// Keeps the symbols `keep` says (by index; those past its end are
    /// kept), in their order, renumbering every reference
    /// ([`Self::renumber_symbols`]) and the dynamic symbol table's ranges;
    /// the string table is built afresh when written. Returns each old
    /// index's new one.
    pub fn retain_symbols(&mut self, keep: &[bool]) -> Vec<Option<u32>> {
        let mut map = Vec::with_capacity(self.symbols.len());
        let mut next = 0u32;
        let mut kept = Vec::with_capacity(self.symbols.len());
        for (i, s) in core::mem::take(&mut self.symbols).into_iter().enumerate() {
            if keep.get(i).copied().unwrap_or(true) {
                map.push(Some(next));
                next += 1;
                kept.push(s);
            } else {
                map.push(None);
            }
        }
        self.symbols = kept;
        self.renumber_symbols(&map);
        self.update_dysymtab();
        self.relayout();
        map
    }

    /// Puts the symbols in LLVM's order: the locals as they are, then the
    /// external definitions and then the undefined symbols, each sorted by
    /// name; renumbers every reference and the dynamic symbol table's
    /// ranges. Returns each old index's new one.
    pub fn sort_symbols(&mut self) -> Vec<Option<u32>> {
        let mut order: Vec<usize> = (0..self.symbols.len()).collect();
        let symbols = &self.symbols;
        order.sort_by(|&a, &b| {
            let (x, y) = (&symbols[a], &symbols[b]);
            x.class().cmp(&y.class()).then_with(|| {
                if x.class() == SymbolClass::Local {
                    a.cmp(&b)
                } else {
                    x.name.cmp(&y.name).then(a.cmp(&b))
                }
            })
        });
        let mut map = alloc::vec![None; order.len()];
        for (new, &old) in order.iter().enumerate() {
            map[old] = Some(u32::try_from(new).unwrap_or(u32::MAX));
        }
        if order.iter().enumerate().all(|(i, &o)| i == o) {
            self.update_dysymtab();
            return map;
        }
        let mut old: Vec<Option<Symbol<'a>>> = core::mem::take(&mut self.symbols)
            .into_iter()
            .map(Some)
            .collect();
        self.symbols = order.iter().filter_map(|&o| old[o].take()).collect();
        self.renumber_symbols(&map);
        self.update_dysymtab();
        self.relayout();
        map
    }

    /// Keeps the sections `keep` says (by index, the ordinal less 1; those
    /// past its end are kept), renumbering every reference: symbols'
    /// `n_sect` (a symbol in a removed section becomes undefined, `N_UNDF`
    /// and `NO_SECT`: the caller removes or rewrites it), local
    /// relocations' section ordinals (a removed section's become `R_ABS`,
    /// 0), and dropping the data-in-code entries and optimization hints in
    /// removed sections. Segments keep their places, emptied or not. The
    /// indirect symbol table is the caller's: a removed symbol pointer or
    /// stub section leaves its entries. Returns each old index's new one.
    pub fn retain_sections(&mut self, keep: &[bool]) -> Vec<Option<u32>> {
        let n = self.section_count();
        let mut map = Vec::with_capacity(n);
        let mut next = 0u32;
        for i in 0..n {
            if keep.get(i).copied().unwrap_or(true) {
                map.push(Some(next));
                next += 1;
            } else {
                map.push(None);
            }
        }
        // The address ranges removed, for the link-edit data.
        let removed: Vec<(u64, u64)> = self
            .sections()
            .zip(&map)
            .filter(|(_, m)| m.is_none())
            .map(|(s, _)| (s.addr, s.addr.saturating_add(s.size())))
            .collect();
        let cputype = self.header.cputype;
        let mut index = 0usize;
        for c in &mut self.commands {
            if let Command::Segment(seg) = c {
                let old = core::mem::take(&mut seg.sections);
                for s in old {
                    let kept = map[index].is_some();
                    index += 1;
                    if kept {
                        seg.sections.push(s);
                    }
                }
            }
        }
        let renumber = |ordinal: u32| -> u32 {
            ordinal
                .checked_sub(1)
                .and_then(|i| map.get(i as usize).copied().flatten())
                .map_or(0, |n| n + 1)
        };
        for s in self.sections_mut() {
            for r in &mut s.relocations {
                if reloc::names_section(cputype, r) {
                    r.symbolnum = renumber(r.symbolnum);
                }
            }
        }
        for sym in &mut self.symbols {
            if sym.n_sect != NO_SECT && (sym.in_section() || sym.is_stab()) {
                let n = renumber(u32::from(sym.n_sect));
                if n == 0 && !sym.is_stab() {
                    sym.n_type = (sym.n_type & !N_TYPE) | N_UNDF;
                    sym.n_value = 0;
                }
                sym.n_sect = u8::try_from(n).unwrap_or(NO_SECT);
            }
        }
        if !removed.is_empty() {
            let gone = |a: u64| removed.iter().any(|&(lo, hi)| a >= lo && a < hi);
            self.edit_linkedit(|a| if gone(a) { None } else { Some(a) });
        }
        self.relayout();
        map
    }

    /// Applies `f` to every address the data-in-code entries and the
    /// optimization hints hold; `None` drops the entry (or the hint). Data
    /// that doesn't decode is left as it is. Returns whether anything
    /// changed.
    pub(crate) fn edit_linkedit(&mut self, f: impl Fn(u64) -> Option<u64>) -> bool {
        let endian = self.endian();
        let wide = self.is_64();
        let mut changed = false;
        for c in &mut self.commands {
            let Command::LinkeditData { cmd, data } = c else {
                continue;
            };
            match *cmd {
                LC_DATA_IN_CODE => {
                    let Some(entries) = linkedit::decode_data_in_code(data, endian) else {
                        continue;
                    };
                    let mut out = Vec::with_capacity(entries.len());
                    for mut e in entries.iter().copied() {
                        if let Some(a) = f(u64::from(e.offset)).and_then(|a| u32::try_from(a).ok())
                        {
                            e.offset = a;
                            out.push(e);
                        }
                    }
                    if out != entries {
                        *data = Cow::Owned(linkedit::encode_data_in_code(&out, endian));
                        changed = true;
                    }
                }
                LC_LINKER_OPTIMIZATION_HINT => {
                    let Some(hints) = linkedit::decode_hints(data) else {
                        continue;
                    };
                    let out: Vec<linkedit::Hint> = hints
                        .iter()
                        .filter_map(|h| {
                            let args: Option<Vec<u64>> = h.args.iter().map(|&a| f(a)).collect();
                            Some(linkedit::Hint {
                                kind: h.kind,
                                args: args?,
                            })
                        })
                        .collect();
                    if out != hints {
                        *data = Cow::Owned(linkedit::encode_hints(&out, wide));
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        changed
    }

    /// The addresses LLVM gives the sections as they are: in file order,
    /// the zero-fill ones after the rest, each at the end of the one
    /// before it in that order aligned to its alignment (by index).
    pub fn llvm_addresses(&self) -> Vec<u64> {
        write::data_offsets(self)
    }

    /// Gives every section LLVM's address ([`Self::llvm_addresses`]) and
    /// moves what the model holds with it: symbols' values (`N_SECT`
    /// symbols' and debugger entries' with a section), scattered
    /// relocations' `r_value`s, data-in-code entries, optimization hints,
    /// the pointers in `__TEXT,__eh_frame` that no relocation covers, and
    /// the addresses relocations encode in the sections' bytes where
    /// [`reloc`] knows the encoding. Returns the relocations whose bytes it
    /// couldn't rewrite though a section they involve moved (and any
    /// `__eh_frame` it couldn't decode); their bytes are as they were.
    pub fn assign_addresses(&mut self) -> Vec<Unadjusted> {
        let new = self.llvm_addresses();
        let old: Vec<(u64, u64)> = self.sections().map(|s| (s.addr, s.size())).collect();
        self.move_sections(&new, &old)
    }

    /// Moves each section to `new` (by index; each was at `old`'s address,
    /// with `old`'s size, which locate the addresses the bytes and
    /// link-edit data hold), as [`Self::assign_addresses`] does.
    pub fn move_sections(&mut self, new: &[u64], old: &[(u64, u64)]) -> Vec<Unadjusted> {
        let n = old.len().min(new.len());
        let cuts = alloc::vec![Vec::new(); n];
        cut::remap(self, &old[..n], &new[..n], &cuts).unadjusted
    }

    /// Removes byte ranges from inside sections (as folding removes atoms):
    /// `cuts` gives each section (by index) offset ranges in it, disjoint
    /// and within it. A section's bytes (or zero-fill size) lose the
    /// ranges; its relocations whose place is in one go, a pair as a
    /// whole; then every section gets LLVM's address for its new size
    /// ([`Self::assign_addresses`]), and everything that holds an address
    /// moves by one map of old addresses to new ones: an address in a
    /// removed range has none, an address after one moves back by the
    /// bytes removed before it in its section, and every address moves
    /// with its section. That is relocations' places (`r_address`) and the
    /// addresses they encode (local relocations' targets, from
    /// [`reloc`]'s encodings: for a PC-relative one, the move of the
    /// target less that of the place), scattered relocations' `r_value`s,
    /// symbols' values, `__eh_frame`'s pointers no relocation covers,
    /// data-in-code entries and optimization hints (dropped if in a
    /// removed range). Returns what it left for the caller: the symbols
    /// and relocations whose addresses were removed, and the relocations
    /// whose encoding it doesn't know. Nothing changes if the ranges are
    /// bad.
    ///
    /// A `SUBTRACTOR` pair whose halves are both local holds only their
    /// difference: when a section either names is cut (not just moved),
    /// it is unadjusted. A local `SUBTRACTOR` whose `UNSIGNED` is external
    /// is taken to hold the subtrahend's address negated (the minuend at
    /// its symbol, no addend), as LLVM writes it.
    pub fn cut(&mut self, cuts: &[(usize, Vec<core::ops::Range<u64>>)]) -> Result<Cut, CutError> {
        let per = cut::gather(self, cuts)?;
        let old: Vec<(u64, u64)> = self.sections().map(|s| (s.addr, s.size())).collect();
        let new = cut::new_addresses(self, &per);
        Ok(cut::remap(self, &old, &new, &per))
    }
}

#[cfg(test)]
mod tests;

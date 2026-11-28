//! The symbol table LTO links with (`SYMTAB_BLOCK`, LLVM's `irsymtab`): a
//! header, then modules, comdats, symbols, and their rarely used fields,
//! all little-endian 32-bit words, with strings in the string table.

use alloc::vec::Vec;

/// Symbol flag bits (`irsymtab::storage::Symbol::FlagBits`).
pub mod flags {
    /// The visibility (two bits): 0 default, 1 hidden, 2 protected.
    pub const VISIBILITY_SHIFT: u32 = 0;
    /// It has an `Uncommon` entry.
    pub const HAS_UNCOMMON: u32 = 1 << 2;
    /// It's undefined.
    pub const UNDEFINED: u32 = 1 << 3;
    /// It's weak.
    pub const WEAK: u32 = 1 << 4;
    /// It's common.
    pub const COMMON: u32 = 1 << 5;
    /// It's indirect (an alias, or a COFF weak external).
    pub const INDIRECT: u32 = 1 << 6;
    /// It's used (`llvm.used`).
    pub const USED: u32 = 1 << 7;
    /// It's thread-local.
    pub const TLS: u32 = 1 << 8;
    /// It may be left out of the symbol table.
    pub const MAY_OMIT: u32 = 1 << 9;
    /// It's global.
    pub const GLOBAL: u32 = 1 << 10;
    /// It's format-specific (not for LTO).
    pub const FORMAT_SPECIFIC: u32 = 1 << 11;
    /// Its address isn't significant.
    pub const UNNAMED_ADDR: u32 = 1 << 12;
    /// It's code.
    pub const EXECUTABLE: u32 = 1 << 13;
}

/// A module's range of symbols.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SymtabModule {
    /// Its first symbol.
    pub begin: u32,
    /// Past its last.
    pub end: u32,
    /// Its first `Uncommon` entry.
    pub uncommon_begin: u32,
}

/// A comdat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SymtabComdat<'d> {
    /// Its name.
    pub name: &'d [u8],
    /// Its selection kind (`llvm::Comdat::SelectionKind`: 0 any, 1 exact
    /// match, 2 largest, 3 no duplicates, 4 same size).
    pub selection: u32,
}

/// A symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SymtabSymbol<'d> {
    /// Its (mangled) name.
    pub name: &'d [u8],
    /// Its IR name (empty if it isn't an IR symbol: from inline assembly).
    pub ir_name: &'d [u8],
    /// Its comdat (an index into [`Symtab::comdats`]), if any.
    pub comdat: Option<u32>,
    /// Its flags ([`flags`]).
    pub flags: u32,
    /// A common symbol's size.
    pub common_size: u64,
    /// A common symbol's alignment.
    pub common_align: u32,
    /// COFF: the symbol a weak external falls back to.
    pub coff_weak_fallback: &'d [u8],
    /// Its section, if given.
    pub section: &'d [u8],
}

impl SymtabSymbol<'_> {
    /// Its visibility (0 default, 1 hidden, 2 protected).
    pub fn visibility(&self) -> u32 {
        (self.flags >> flags::VISIBILITY_SHIFT) & 3
    }

    /// Whether `flag` is set.
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

/// A symbol table, with the string table it refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Symtab<'d> {
    /// The table's bytes.
    pub data: &'d [u8],
    /// The string table.
    pub strtab: &'d [u8],
}

const HEADER: usize = 4 + 9 * 8;
const SYMBOL: usize = 24;
const UNCOMMON: usize = 28;
const RANGE_ENTRY: usize = 12;

impl<'d> Symtab<'d> {
    /// A table of `data`, its strings in `strtab`.
    pub fn new(data: &'d [u8], strtab: &'d [u8]) -> Self {
        Self { data, strtab }
    }

    fn word(&self, at: usize) -> Option<u32> {
        let b = self.data.get(at..at.checked_add(4)?)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn string_at(&self, at: usize) -> &'d [u8] {
        let (Some(off), Some(size)) = (self.word(at), self.word(at + 4)) else {
            return &[];
        };
        let (off, size) = (off as usize, size as usize);
        self.strtab
            .get(off..off.saturating_add(size))
            .unwrap_or(&[])
    }

    /// A range's entries: their offsets, each `size` bytes, those that fit.
    fn range(&self, at: usize, size: usize) -> impl Iterator<Item = usize> + use<> {
        let off = self.word(at).unwrap_or(0) as usize;
        let n = self.word(at + 4).unwrap_or(0) as usize;
        let fit = self.data.len().saturating_sub(off) / size;
        (0..n.min(fit)).map(move |i| off + i * size)
    }

    /// Whether the header is all there.
    pub fn is_valid(&self) -> bool {
        self.data.len() >= HEADER
    }

    /// The format's version (4).
    pub fn version(&self) -> Option<u32> {
        self.word(0)
    }

    /// The producer (`24.0.0git <revision>`).
    pub fn producer(&self) -> &'d [u8] {
        self.string_at(4)
    }

    /// The modules.
    pub fn modules(&self) -> Vec<SymtabModule> {
        self.range(12, RANGE_ENTRY)
            .map(|at| SymtabModule {
                begin: self.word(at).unwrap_or(0),
                end: self.word(at + 4).unwrap_or(0),
                uncommon_begin: self.word(at + 8).unwrap_or(0),
            })
            .collect()
    }

    /// The comdats.
    pub fn comdats(&self) -> Vec<SymtabComdat<'d>> {
        self.range(20, RANGE_ENTRY)
            .map(|at| SymtabComdat {
                name: self.string_at(at),
                selection: self.word(at + 8).unwrap_or(0),
            })
            .collect()
    }

    /// The symbols, each with its `Uncommon` fields (a module's symbols
    /// that have them take its entries in order).
    pub fn symbols(&self) -> Vec<SymtabSymbol<'d>> {
        let uncommons: Vec<usize> = self.range(36, UNCOMMON).collect();
        let at: Vec<usize> = self.range(28, SYMBOL).collect();
        let flags_of = |i: usize| self.word(at[i] + 20).unwrap_or(0);
        let mut uncommon_of: Vec<Option<usize>> = alloc::vec![None; at.len()];
        for m in self.modules() {
            let mut u = m.uncommon_begin as usize;
            let end = (m.end as usize).min(at.len());
            let begin = (m.begin as usize).min(end);
            for (i, slot) in uncommon_of.iter_mut().enumerate().take(end).skip(begin) {
                if flags_of(i) & flags::HAS_UNCOMMON != 0 {
                    *slot = uncommons.get(u).copied();
                    u += 1;
                }
            }
        }
        at.iter()
            .zip(uncommon_of)
            .map(|(&at, uncommon)| {
                let mut s = SymtabSymbol {
                    name: self.string_at(at),
                    ir_name: self.string_at(at + 8),
                    comdat: self.word(at + 16).filter(|&c| c != u32::MAX),
                    flags: self.word(at + 20).unwrap_or(0),
                    common_size: 0,
                    common_align: 0,
                    coff_weak_fallback: &[],
                    section: &[],
                };
                if let Some(u) = uncommon {
                    let lo = u64::from(self.word(u).unwrap_or(0));
                    let hi = u64::from(self.word(u + 4).unwrap_or(0));
                    s.common_size = lo | (hi << 32);
                    s.common_align = self.word(u + 8).unwrap_or(0);
                    s.coff_weak_fallback = self.string_at(u + 12);
                    s.section = self.string_at(u + 20);
                }
                s
            })
            .collect()
    }

    /// The target triple.
    pub fn target_triple(&self) -> &'d [u8] {
        self.string_at(44)
    }

    /// The source file's name.
    pub fn source_filename(&self) -> &'d [u8] {
        self.string_at(52)
    }

    /// COFF: the linker directives.
    pub fn coff_linker_opts(&self) -> &'d [u8] {
        self.string_at(60)
    }

    /// The dependent libraries.
    pub fn dependent_libraries(&self) -> Vec<&'d [u8]> {
        self.range(68, 8).map(|at| self.string_at(at)).collect()
    }
}

//! The symbols GNU `ar` and `llvm-ar` index for a member, and whether they
//! count it as an object (an archive with an object in it gets a symbol
//! table, even an empty one).
//!
//! **An ELF object** (of any machine, class, and byte order): the symbols
//! of its `.symtab`, in order, from index 1, that are
//!
//! | Symbol | GNU `ar` | `llvm-ar` |
//! |---|---|---|
//! | `STB_GLOBAL`, `STB_WEAK`, `STB_GNU_UNIQUE`, defined (absolute too) | listed | listed |
//! | the same, common (`SHN_COMMON`; x86-64's `SHN_X86_64_LCOMMON`) | listed | listed |
//! | the same, undefined | not | not |
//! | another binding but `STB_LOCAL` (`STB_LOOS` and up), defined | not | listed |
//! | `STB_LOCAL`, common | listed | not |
//! | `STT_FILE` or `STT_SECTION`, not local | listed | not |
//! | not local, named as a mapping symbol: `$d` and `$x` (AArch64, RISC-V), `$a`, `$d`, `$t`, and the empty name (ARM), `$d` and `$t` (C-SKY), `.L0 ` (RISC-V) | listed | not |
//!
//! (Read in binutils 2.46's `elf_slurp_symbol_table` and LLVM 24's
//! `ELFObjectFile::getSymbolFlags`; the first three rows are also what the
//! corpus's archives show. The others are rare enough that no compiler
//! writes them for C or C++.) An ELF file of another type (a shared
//! object, an executable) is indexed alike, from its `.symtab`; a file
//! without one counts as an object with no symbols.
//!
//! **Not objects**, to either tool: archives, text, and anything that is
//! no object format either reads. (GNU `ar` would also read S-records,
//! Intel hex, Tektronix hex, and Verilog hex as objects without symbols:
//! not recognized here.)
//!
//! **A Mach-O file** (thin, any file type, either width and byte order),
//! for `llvm-ar`: the entries of its `LC_SYMTAB`, in order, that are
//! external (`N_EXT`, private externs included), not debugger entries
//! (`N_STAB`), and not undefined (`N_UNDF` with a value of 0; a common
//! symbol, `N_UNDF` with a size, is listed), as LLVM 24's
//! `MachOObjectFile::getSymbolFlags` and `isArchiveSymbol` decide. A
//! universal file isn't an object to it (no symbols, not counted). A name
//! past the string table is an error (`llvm-ar` refuses it). GNU `ar`'s
//! Mach-O index isn't modeled.
//!
//! **Not computed**: COFF, wasm, and XCOFF objects, Mach-O for GNU `ar`, LLVM bitcode,
//! and GCC's LTO objects (`.gnu.lto_` sections), whose symbols GNU `ar`
//! takes from its LTO plugin. Their indexes aren't modeled here.

use alloc::vec::Vec;
use core::fmt;

use turbonop_elf::ElfFile;
use turbonop_elf::consts::{SHT_SYMTAB, SHT_SYMTAB_SHNDX, STB_GLOBAL, STB_LOCAL, STB_WEAK};

use super::Archiver;

#[cfg(test)]
mod tests;

/// What a member is to the index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemberSymbols<'a> {
    /// Whether the tool counts it as an object.
    pub object: bool,
    /// The symbols the index lists for it, in its order.
    pub symbols: Vec<&'a [u8]>,
}

/// Why a member's symbols weren't computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolsError {
    /// A format whose index isn't modeled here (COFF, Mach-O, wasm,
    /// XCOFF, LLVM bitcode, a GCC LTO object for GNU `ar`).
    Unsupported(&'static str),
    /// An ELF file whose symbol table can't be read (`llvm-ar` refuses
    /// it).
    Malformed,
}

impl fmt::Display for SymbolsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SymbolsError::Unsupported(what) => write!(f, "{what}: its index isn't modeled"),
            SymbolsError::Malformed => f.write_str("its symbol table is unreadable"),
        }
    }
}

impl core::error::Error for SymbolsError {}

const STB_GNU_UNIQUE: u8 = 10;
const SHN_UNDEF: u32 = 0;
const SHN_COMMON: u32 = 0xFFF2;
const SHN_X86_64_LCOMMON: u32 = 0xFF02;
const STT_SECTION: u8 = 3;
const STT_FILE: u8 = 4;
const EM_ARM: u16 = 40;
const EM_X86_64: u16 = 62;
const EM_AARCH64: u16 = 183;
const EM_RISCV: u16 = 243;
const EM_CSKY: u16 = 252;

/// What kind of file `data` is, by its first bytes, when not ELF.
fn other_format(data: &[u8]) -> Option<&'static str> {
    let le32 = |b: &[u8]| {
        b.get(..4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let be32 = |b: &[u8]| {
        b.get(..4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    if data.starts_with(b"BC\xC0\xDE") || le32(data) == Some(0x0B17_C0DE) {
        return Some("LLVM bitcode");
    }
    if data.starts_with(b"\0asm") {
        return Some("a wasm object");
    }
    if matches!(le32(data), Some(0xFEED_FACE | 0xFEED_FACF))
        || matches!(be32(data), Some(0xFEED_FACE | 0xFEED_FACF))
    {
        return Some("a Mach-O object");
    }
    if data.len() >= 2 && matches!(u16::from_be_bytes([data[0], data[1]]), 0x01DF | 0x01F7) {
        return Some("an XCOFF object");
    }
    // A COFF object: a known machine, and a header that fits; or an import
    // object, or a big-obj or anonymous object.
    if data.len() >= 20 {
        let machine = u16::from_le_bytes([data[0], data[1]]);
        let known = matches!(
            machine,
            0x014C | 0x8664 | 0xAA64 | 0xA641 | 0xA64E | 0x01C4 | 0x01C0 | 0x0200
        );
        if known && u16::from_le_bytes([data[16], data[17]]) == 0 {
            return Some("a COFF object");
        }
        if machine == 0 && u16::from_le_bytes([data[2], data[3]]) == 0xFFFF {
            return Some("a COFF import or big-obj object");
        }
    }
    None
}

/// The symbols `archiver` indexes for the member `data`.
pub fn member_symbols(data: &[u8], archiver: Archiver) -> Result<MemberSymbols<'_>, SymbolsError> {
    if !ElfFile::is_elf(data) {
        if archiver == Archiver::LlvmAr && turbonop_macho::is_macho(data) {
            return macho_symbols(data);
        }
        if let Some(what) = other_format(data) {
            return Err(SymbolsError::Unsupported(what));
        }
        return Ok(MemberSymbols::default());
    }
    let Ok(file) = ElfFile::parse(data) else {
        // GNU ar: not an object; llvm-ar: an error.
        return match archiver {
            Archiver::GnuAr => Ok(MemberSymbols::default()),
            Archiver::LlvmAr => Err(SymbolsError::Malformed),
        };
    };
    if archiver == Archiver::GnuAr && file.sections().any(|s| s.name.starts_with(b".gnu.lto_")) {
        return Err(SymbolsError::Unsupported("a GCC LTO object"));
    }
    let machine = file.header().machine;
    let mut out = MemberSymbols {
        object: true,
        symbols: Vec::new(),
    };
    let Some(table) = file.sections().find(|s| s.kind == SHT_SYMTAB) else {
        return Ok(out);
    };
    let symbols = file.symbols(&table);
    // A section index from SHT_SYMTAB_SHNDX is a section's, even one
    // numbered like a special index (0xFF02, 0xFFF2: an object of more than
    // 65,280 sections has them).
    let extended: Vec<u8> = file
        .sections()
        .find(|s| s.kind == SHT_SYMTAB_SHNDX && s.link as usize == table.index)
        .map(|s| file.section_data(&s).to_vec())
        .unwrap_or_default();
    let in_extended = |i: usize| {
        extended
            .get(i * 4..i * 4 + 4)
            .is_some_and(|b| b.iter().any(|&x| x != 0))
    };
    for s in symbols.iter().skip(1) {
        let listed = match archiver {
            Archiver::GnuAr => {
                let common = !in_extended(s.index)
                    && (s.shndx == SHN_COMMON
                        || (machine == EM_X86_64 && s.shndx == SHN_X86_64_LCOMMON));
                s.shndx != SHN_UNDEF
                    && (matches!(s.bind, STB_GLOBAL | STB_WEAK | STB_GNU_UNIQUE) || common)
            }
            Archiver::LlvmAr => {
                let mapping = match machine {
                    EM_AARCH64 => s.name.starts_with(b"$d") || s.name.starts_with(b"$x"),
                    EM_ARM => {
                        s.name.is_empty()
                            || s.name.starts_with(b"$d")
                            || s.name.starts_with(b"$t")
                            || s.name.starts_with(b"$a")
                    }
                    EM_CSKY => s.name.starts_with(b"$d") || s.name.starts_with(b"$t"),
                    EM_RISCV => {
                        s.name == b".L0 " || s.name.starts_with(b"$d") || s.name.starts_with(b"$x")
                    }
                    _ => false,
                };
                s.bind != STB_LOCAL
                    && s.shndx != SHN_UNDEF
                    && !matches!(s.kind, STT_FILE | STT_SECTION)
                    && !mapping
            }
        };
        if listed {
            out.symbols.push(s.name);
        }
    }
    Ok(out)
}

/// `llvm-ar`'s symbols for a thin Mach-O file.
fn macho_symbols(data: &[u8]) -> Result<MemberSymbols<'_>, SymbolsError> {
    use turbonop_macho::consts::{N_EXT, N_STAB, N_TYPE, N_UNDF};
    let file = turbonop_macho::MachO::parse(data).map_err(|_| SymbolsError::Malformed)?;
    let mut out = MemberSymbols {
        object: true,
        symbols: Vec::new(),
    };
    let strsize = file.symtab().map_or(0, |s| s.strsize);
    for s in file.symbols() {
        let listed = s.n_type & N_STAB == 0
            && s.n_type & N_EXT != 0
            && !(s.n_type & N_TYPE == N_UNDF && s.n_value == 0);
        if listed {
            if s.strx >= strsize {
                return Err(SymbolsError::Malformed);
            }
            out.symbols.push(s.name);
        }
    }
    Ok(out)
}

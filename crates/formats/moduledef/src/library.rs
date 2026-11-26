//! Writing a whole import library: the descriptor objects, the import
//! objects (and `llvm-lib`'s weak externals), and the index, as an archive.
//!
//! `lib.exe` stores the imports sorted by symbol after the descriptor
//! objects, and lists them in the second linker member (and so orders the
//! first) as the `.def` file does; an ARM64X library's ARM64EC imports come
//! before its ARM64 ones in both (measured). `llvm-lib` stores and lists
//! every member in the order it made them. In an ARM64EC or ARM64X library
//! both list the descriptor objects' symbols in `/<ECSYMBOLS>/` as well as
//! in the linker members.

use alloc::vec::Vec;

use turbonop_archive::symbols::{SymbolsError, is_import_descriptor, member_symbols};
use turbonop_archive::write::{Index, NewMember, SizeLimit, Style, Tool, WriteError};
use turbonop_pe::consts::machine;

/// Why an import library can't be written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LibraryError {
    /// A member's symbols couldn't be read (a member this crate made, so
    /// this is a bug).
    Symbols(SymbolsError),
    /// The archive couldn't be written.
    Write(WriteError),
}

impl core::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LibraryError::Symbols(e) => write!(f, "{e}"),
            LibraryError::Write(e) => write!(f, "{e}"),
        }
    }
}

impl core::error::Error for LibraryError {}

/// An import library's members, in the archive's order, with their
/// order in the index (the second linker member's list).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Members {
    /// The DLL's file name: every member's name.
    pub dll: Vec<u8>,
    /// The members' bytes, in the archive's order.
    pub data: Vec<Vec<u8>>,
    /// The members' positions in the archive, in the index's order.
    pub listed: Vec<u32>,
    /// The library's machine (ARM64EC and ARM64X libraries have a
    /// `/<ECSYMBOLS>/`).
    pub machine: u16,
}

impl Members {
    /// The archive, with `style`'s header fields, and the index `tool`
    /// computes from the members.
    pub fn write(&self, style: Style, tool: Tool) -> Result<Vec<u8>, LibraryError> {
        let ec = matches!(self.machine, machine::ARM64EC | machine::ARM64X);
        let mut symbols = Vec::with_capacity(self.listed.len());
        for &at in &self.listed {
            let data = &self.data[at as usize];
            let mut s = member_symbols(data, tool, ec).map_err(LibraryError::Symbols)?;
            if ec {
                // Both tools list the descriptor objects' symbols in
                // `/<ECSYMBOLS>/` too.
                let descriptors = s.symbols.iter().filter(|m| is_import_descriptor(&m.name));
                s.ec_symbols
                    .extend(descriptors.cloned().collect::<Vec<_>>());
            }
            symbols.push((at, s));
        }
        let index = Index::build(symbols, tool, ec);
        let fields = style.import_fields();
        let members: Vec<NewMember<'_>> = self
            .data
            .iter()
            .map(|d| NewMember {
                name: &self.dll,
                fields,
                data: d,
            })
            .collect();
        turbonop_archive::write::write(&members, &index, style, SizeLimit::FourGiB)
            .map_err(LibraryError::Write)
    }
}

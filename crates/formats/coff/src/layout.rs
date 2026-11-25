//! Where an object's parts were in the file it was read from.
//!
//! Tools differ in how they place an object's parts, and an object written
//! back unchanged must be the file it was. What the objects of the
//! turbolib corpus and of the tools here show (MSVC 14.52, LLVM 24, and
//! the assemblers):
//!
//! - `cl.exe` and `lib.exe`: the header and section table, then each
//!   section's data and its relocations, in section order, packed; then
//!   the symbol table and the string table. Sections with no data have no
//!   data pointer. `lib.exe`'s import descriptors give a section with no
//!   relocations the relocation pointer of the section before it.
//! - `ml64.exe`: as `cl.exe`, but relocations start at an even offset (the
//!   byte skipped is zero).
//! - `armasm64.exe`: each section's data starts at a multiple of 4.
//! - LLVM: as `cl.exe`, but in the order it created the sections, which is
//!   not their number order (it numbers associative sections last), and
//!   every section that isn't uninitialized has a data pointer, even an
//!   empty one.
//!
//! So the layout is recorded rather than recomputed: each section's three
//! pointers, the symbol table's, the file's length, and the bytes no part
//! claims that aren't zero. It applies as long as every part keeps its
//! size.

use alloc::borrow::Cow;
use alloc::vec::Vec;

use crate::object::Format;

/// The layout of a file that was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Layout<'data> {
    pub format: Format,
    pub optional_header: usize,
    pub sections: Vec<Placement>,
    /// `PointerToSymbolTable`.
    pub symbol_table: u32,
    /// `NumberOfSymbols`: the number of records.
    pub symbol_records: u32,
    /// The string table as it was: from its size field to the end of what
    /// it declares (or of the file, if that comes first). `None` if the
    /// file ended at the symbol table.
    pub strings: Option<Cow<'data, [u8]>>,
    /// The file's length.
    pub len: u32,
    /// Bytes outside every part that aren't all zero: padding, and
    /// anything after the string table.
    pub fill: Vec<(u32, Cow<'data, [u8]>)>,
}

/// Where one section's parts were, and their sizes then.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Placement {
    /// `PointerToRawData`.
    pub data: u32,
    /// `PointerToRelocations`.
    pub relocations: u32,
    /// `PointerToLinenumbers`.
    pub line_numbers: u32,
    /// `SizeOfRawData`, and whether the section had no data in the file.
    pub size: u32,
    pub uninitialized: bool,
    pub relocation_count: u32,
    pub line_count: u32,
    /// Whether the relocation count was in the first relocation
    /// (`IMAGE_SCN_LNK_NRELOC_OVFL`), and that record's other fields.
    pub extended: Option<(u32, u16)>,
}

impl Layout<'_> {
    pub fn into_owned(self) -> Layout<'static> {
        Layout {
            format: self.format,
            optional_header: self.optional_header,
            sections: self.sections,
            symbol_table: self.symbol_table,
            symbol_records: self.symbol_records,
            strings: self.strings.map(|s| Cow::Owned(s.into_owned())),
            len: self.len,
            fill: self
                .fill
                .into_iter()
                .map(|(at, bytes)| (at, Cow::Owned(bytes.into_owned())))
                .collect(),
        }
    }
}

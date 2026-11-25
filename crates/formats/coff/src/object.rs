//! The object model: an object's headers, sections, and symbols, as values
//! that can be changed and written back.

use alloc::borrow::Cow;
use alloc::vec::Vec;
use core::fmt;

use crate::layout::Layout;

/// Which header an object has, which also sets the size of its symbol
/// records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// `IMAGE_FILE_HEADER`: at most 65,279 sections, 18-byte symbol
    /// records with 16-bit section numbers.
    Regular,
    /// `ANON_OBJECT_HEADER_BIGOBJ` (`/bigobj`): 32-bit section counts and
    /// numbers, 20-byte symbol records.
    BigObj,
}

impl Format {
    /// The size of a symbol (or auxiliary) record.
    pub const fn record_size(self) -> usize {
        match self {
            Format::Regular => 18,
            Format::BigObj => 20,
        }
    }

    /// The size of the header, before the section table (and, in a regular
    /// object, before any optional header).
    pub const fn header_size(self) -> usize {
        match self {
            Format::Regular => 20,
            Format::BigObj => 56,
        }
    }
}

/// How a fresh layout is made: which tool's conventions the writer follows
/// when an object no longer fits the layout it was read with.
///
/// Both lay out the header and section table, then each section's data
/// followed by its relocations and line numbers, then the symbol table and
/// the string table, with no padding. They differ in:
///
/// - **Strings.** MSVC gives each use of a long name its own entry, in
///   order: symbols' names, then sections'. (A name used twice, such as
///   two sections named `.rdata$ilfixup`, is there twice.) LLVM stores
///   each name once, sorted by its reversed bytes, and a name that ends
///   another as a pointer into it.
/// - **Empty sections.** MSVC gives a section with no data no data
///   pointer; LLVM points every section that isn't
///   `IMAGE_SCN_CNT_UNINITIALIZED_DATA` at where its data would be.
///
/// LLVM also writes associative sections' data in the order it created
/// them, not the order it numbers them; a fresh layout writes sections in
/// number order in both styles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Style {
    /// `cl.exe`, `lib.exe`, `cvtres.exe`, and the MASM assemblers.
    #[default]
    Msvc,
    /// `clang-cl`, `llvm-mc`, and the rest of LLVM.
    Llvm,
}

/// The fields of a big-obj header that a regular header lacks, and that
/// describe nothing in the model (they are normally zero).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BigObjHeader {
    /// `Version`: 2.
    pub version: u16,
    /// `SizeOfData`.
    pub size_of_data: u32,
    /// `Flags`.
    pub flags: u32,
    /// `MetaDataSize`.
    pub meta_data_size: u32,
    /// `MetaDataOffset`.
    pub meta_data_offset: u32,
}

impl Default for BigObjHeader {
    fn default() -> Self {
        Self {
            version: 2,
            size_of_data: 0,
            flags: 0,
            meta_data_size: 0,
            meta_data_offset: 0,
        }
    }
}

/// A COFF object.
///
/// Sections and symbols are numbered by their positions in
/// [`sections`](Object::sections) and [`symbols`](Object::symbols):
/// [`SectionId`] and [`SymbolId`]. A symbol's auxiliary records belong to
/// it (as [`Aux`]), so a `SymbolId` counts primary records only; the
/// symbol table index the file uses is
/// [`symbol_table_index`](Object::symbol_table_index).
///
/// An object that was read remembers its layout: where each part was in
/// the file, and any bytes between the parts. Written back unchanged (or
/// with changes that keep every part the same size), it is the same file,
/// byte for byte. Otherwise it is laid out afresh, in its [`Style`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Object<'data> {
    /// Which header the object has.
    pub format: Format,
    /// `IMAGE_FILE_MACHINE_*`.
    pub machine: u16,
    /// `TimeDateStamp` (0, or a hash, under `/Brepro`).
    pub time_date_stamp: u32,
    /// A regular header's `Characteristics` (`IMAGE_FILE_*`). A big-obj
    /// header has none, so it must be 0 to write one.
    pub characteristics: u16,
    /// A regular object's optional header, as it is in the file (almost
    /// always empty). A big-obj header has none.
    pub optional_header: Cow<'data, [u8]>,
    /// The big-obj header's own fields (ignored for a regular object).
    pub bigobj: BigObjHeader,
    /// The sections, in number order (section number 1 is index 0).
    pub sections: Vec<Section<'data>>,
    /// The symbols, each with its auxiliary records.
    pub symbols: Vec<Symbol<'data>>,
    /// How to lay the object out when it no longer fits its recorded
    /// layout. Reading sets it to the style the object appears to follow.
    pub style: Style,
    /// Where the parts were in the file that was read, and its string
    /// table as it was. `None` for a new object, and after a structural
    /// edit.
    pub(crate) layout: Option<Layout<'data>>,
}

impl Default for Object<'_> {
    fn default() -> Self {
        Self::new(Format::Regular, 0)
    }
}

impl<'data> Object<'data> {
    /// An empty object for `machine`.
    pub fn new(format: Format, machine: u16) -> Self {
        Self {
            format,
            machine,
            time_date_stamp: 0,
            characteristics: 0,
            optional_header: Cow::Borrowed(&[]),
            bigobj: BigObjHeader::default(),
            sections: Vec::new(),
            symbols: Vec::new(),
            style: Style::Msvc,
            layout: None,
        }
    }

    /// The section `id`, if there is one.
    pub fn section(&self, id: SectionId) -> Option<&Section<'data>> {
        self.sections.get(id.index())
    }

    /// The section `id`, mutably.
    pub fn section_mut(&mut self, id: SectionId) -> Option<&mut Section<'data>> {
        self.sections.get_mut(id.index())
    }

    /// The symbol `id`, if there is one.
    pub fn symbol(&self, id: SymbolId) -> Option<&Symbol<'data>> {
        self.symbols.get(id.index())
    }

    /// The symbol `id`, mutably.
    pub fn symbol_mut(&mut self, id: SymbolId) -> Option<&mut Symbol<'data>> {
        self.symbols.get_mut(id.index())
    }

    /// Whether the object still has the layout it was read with (it may
    /// still not fit it, if parts have changed size).
    pub fn has_recorded_layout(&self) -> bool {
        self.layout.is_some()
    }

    /// Forgets the layout the object was read with, and its string table:
    /// the next write lays it out afresh, in its [`Style`], with a string
    /// table of exactly the names it uses.
    pub fn forget_layout(&mut self) {
        self.layout = None;
    }

    /// The object with everything it borrows copied.
    pub fn into_owned(self) -> Object<'static> {
        Object {
            format: self.format,
            machine: self.machine,
            time_date_stamp: self.time_date_stamp,
            characteristics: self.characteristics,
            optional_header: Cow::Owned(self.optional_header.into_owned()),
            bigobj: self.bigobj,
            sections: self.sections.into_iter().map(Section::into_owned).collect(),
            symbols: self.symbols.into_iter().map(Symbol::into_owned).collect(),
            style: self.style,
            layout: self.layout.map(Layout::into_owned),
        }
    }
}

/// A section's position in [`Object::sections`]: section number 1 is
/// `SectionId(0)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SectionId(pub u32);

impl SectionId {
    /// The 1-based section number the file uses.
    pub const fn number(self) -> u32 {
        self.0 + 1
    }

    /// The section with 1-based number `number`, or `None` for 0.
    pub const fn from_number(number: u32) -> Option<Self> {
        match number {
            0 => None,
            n => Some(Self(n - 1)),
        }
    }

    /// The position as an index.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A symbol's position in [`Object::symbols`], counting primary records
/// only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SymbolId(pub u32);

impl SymbolId {
    /// The position as an index.
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// A section or symbol name.
///
/// A name read from a file remembers how it was stored (inline, or which
/// string-table entry), so that a name written back unchanged is stored the
/// same way, even where a tool stored it unusually: a short name in the
/// string table, a duplicate entry, bytes after an inline name's NUL.
#[derive(Clone)]
pub struct Name<'data> {
    bytes: Cow<'data, [u8]>,
    /// The 8-byte name field as read: inline bytes, four zero bytes and a
    /// string-table offset, or (for sections) `/<offset>`.
    pub(crate) stored: Option<[u8; 8]>,
}

impl<'data> Name<'data> {
    /// A name of `bytes`, stored however the writer chooses.
    pub fn new(bytes: impl Into<Cow<'data, [u8]>>) -> Self {
        Self {
            bytes: bytes.into(),
            stored: None,
        }
    }

    pub(crate) fn stored(bytes: &'data [u8], field: [u8; 8]) -> Self {
        Self {
            bytes: Cow::Borrowed(bytes),
            stored: Some(field),
        }
    }

    /// The name's bytes (without a NUL).
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Replaces the name.
    pub fn set(&mut self, bytes: impl Into<Cow<'data, [u8]>>) {
        self.bytes = bytes.into();
        self.stored = None;
    }

    /// The name with what it borrows copied.
    pub fn into_owned(self) -> Name<'static> {
        Name {
            bytes: Cow::Owned(self.bytes.into_owned()),
            stored: self.stored,
        }
    }
}

impl PartialEq for Name<'_> {
    /// Names are equal when their bytes are, however they are stored.
    fn eq(&self, other: &Self) -> bool {
        self.bytes == other.bytes
    }
}

impl Eq for Name<'_> {}

impl PartialEq<[u8]> for Name<'_> {
    fn eq(&self, other: &[u8]) -> bool {
        *self.bytes == *other
    }
}

impl PartialEq<&[u8]> for Name<'_> {
    fn eq(&self, other: &&[u8]) -> bool {
        *self.bytes == **other
    }
}

impl<const N: usize> PartialEq<&[u8; N]> for Name<'_> {
    fn eq(&self, other: &&[u8; N]) -> bool {
        *self.bytes == other[..]
    }
}

impl fmt::Debug for Name<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{}\"", self.bytes.escape_ascii())
    }
}

impl<'data> From<&'data [u8]> for Name<'data> {
    fn from(bytes: &'data [u8]) -> Self {
        Self::new(bytes)
    }
}

impl<'data, const N: usize> From<&'data [u8; N]> for Name<'data> {
    fn from(bytes: &'data [u8; N]) -> Self {
        Self::new(&bytes[..])
    }
}

impl<'data> From<&'data str> for Name<'data> {
    fn from(text: &'data str) -> Self {
        Self::new(text.as_bytes())
    }
}

impl From<Vec<u8>> for Name<'_> {
    fn from(bytes: Vec<u8>) -> Self {
        Self::new(bytes)
    }
}

/// One section: its header's fields, its data, its relocations, and its
/// line numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section<'data> {
    /// The name (`.text$mn`, `.debug$S`, ...). Names longer than 8 bytes
    /// are stored in the string table.
    pub name: Name<'data>,
    /// `VirtualSize` (`PhysicalAddress`): 0 in objects.
    pub virtual_size: u32,
    /// `VirtualAddress`: 0 in objects.
    pub virtual_address: u32,
    /// `IMAGE_SCN_*`. The writer sets `IMAGE_SCN_LNK_NRELOC_OVFL` when the
    /// relocations need the extended count, and otherwise writes these as
    /// they are.
    pub characteristics: u32,
    /// The raw data, or the size of an uninitialized section.
    pub data: SectionData<'data>,
    /// The relocations, in order.
    pub relocations: Vec<Relocation>,
    /// The COFF line numbers (obsolete: no current tool writes them).
    pub line_numbers: Vec<LineNumber>,
}

impl<'data> Section<'data> {
    /// A section of `data`, with no relocations.
    pub fn new(
        name: impl Into<Name<'data>>,
        characteristics: u32,
        data: SectionData<'data>,
    ) -> Self {
        Self {
            name: name.into(),
            virtual_size: 0,
            virtual_address: 0,
            characteristics,
            data,
            relocations: Vec::new(),
            line_numbers: Vec::new(),
        }
    }

    /// `SizeOfRawData`: the data's length, or an uninitialized section's
    /// size.
    pub fn size(&self) -> u32 {
        self.data.size()
    }

    /// The data's bytes (empty for an uninitialized section).
    pub fn bytes(&self) -> &[u8] {
        match &self.data {
            SectionData::Bytes(b) => b,
            SectionData::Uninitialized(_) => &[],
        }
    }

    /// Whether the relocations need the extended count
    /// (`IMAGE_SCN_LNK_NRELOC_OVFL`): there are 0xFFFF or more.
    pub fn needs_extended_relocations(&self) -> bool {
        self.relocations.len() >= 0xFFFF
    }

    /// The section with what it borrows copied.
    pub fn into_owned(self) -> Section<'static> {
        Section {
            name: self.name.into_owned(),
            virtual_size: self.virtual_size,
            virtual_address: self.virtual_address,
            characteristics: self.characteristics,
            data: self.data.into_owned(),
            relocations: self.relocations,
            line_numbers: self.line_numbers,
        }
    }
}

/// A section's contents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SectionData<'data> {
    /// Raw data in the file (possibly empty).
    Bytes(Cow<'data, [u8]>),
    /// An uninitialized section (`.bss`) of this size: it has a size but
    /// no data in the file.
    Uninitialized(u32),
}

impl SectionData<'_> {
    /// `SizeOfRawData`.
    pub fn size(&self) -> u32 {
        match self {
            // The writer refuses data longer than this.
            SectionData::Bytes(b) => u32::try_from(b.len()).unwrap_or(u32::MAX),
            SectionData::Uninitialized(n) => *n,
        }
    }

    /// The data with what it borrows copied.
    pub fn into_owned(self) -> SectionData<'static> {
        match self {
            SectionData::Bytes(b) => SectionData::Bytes(Cow::Owned(b.into_owned())),
            SectionData::Uninitialized(n) => SectionData::Uninitialized(n),
        }
    }
}

/// One relocation (`IMAGE_RELOCATION`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Relocation {
    /// The offset of the location in the section (`VirtualAddress`).
    pub offset: u32,
    /// The symbol it refers to.
    pub symbol: SymbolId,
    /// `IMAGE_REL_<machine>_*`.
    pub kind: u16,
}

/// One COFF line number record (`IMAGE_LINENUMBER`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineNumber {
    /// The start of a function's line numbers (line number 0): the
    /// function's symbol.
    Function(SymbolId),
    /// A line: the address of its code (an offset in the section) and its
    /// number, relative to the function's first line. A record with number
    /// 0 whose symbol index names no symbol is read as a `Line` of number
    /// 0, which writes the same bytes.
    Line {
        /// `VirtualAddress`.
        address: u32,
        /// `Linenumber`.
        number: u16,
    },
}

/// A symbol's section number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SectionNumber {
    /// `IMAGE_SYM_UNDEFINED` (0): an external reference, or (with a
    /// nonzero value) a common symbol.
    Undefined,
    /// `IMAGE_SYM_ABSOLUTE` (-1): the value is an absolute value.
    Absolute,
    /// `IMAGE_SYM_DEBUG` (-2): debugging information (`.file`).
    Debug,
    /// A section of the object.
    Section(SectionId),
    /// Any other number: a negative one, or one past the last section.
    /// It is written as it is.
    Other(i32),
}

impl SectionNumber {
    /// The section, if the number names one.
    pub fn id(self) -> Option<SectionId> {
        match self {
            SectionNumber::Section(id) => Some(id),
            _ => None,
        }
    }

    /// The number as the file stores it.
    pub fn raw(self) -> i32 {
        match self {
            SectionNumber::Undefined => 0,
            SectionNumber::Absolute => -1,
            SectionNumber::Debug => -2,
            // Section counts are checked to fit in an i32 when written.
            SectionNumber::Section(id) => i32::try_from(id.number()).unwrap_or(i32::MAX),
            SectionNumber::Other(n) => n,
        }
    }
}

/// `IMAGE_SYM_CLASS_*` storage classes.
pub mod storage_class {
    pub const END_OF_FUNCTION: u8 = 0xFF;
    pub const NULL: u8 = 0;
    pub const AUTOMATIC: u8 = 1;
    pub const EXTERNAL: u8 = 2;
    pub const STATIC: u8 = 3;
    pub const REGISTER: u8 = 4;
    pub const EXTERNAL_DEF: u8 = 5;
    pub const LABEL: u8 = 6;
    pub const UNDEFINED_LABEL: u8 = 7;
    pub const MEMBER_OF_STRUCT: u8 = 8;
    pub const ARGUMENT: u8 = 9;
    pub const STRUCT_TAG: u8 = 10;
    pub const MEMBER_OF_UNION: u8 = 11;
    pub const UNION_TAG: u8 = 12;
    pub const TYPE_DEFINITION: u8 = 13;
    pub const UNDEFINED_STATIC: u8 = 14;
    pub const ENUM_TAG: u8 = 15;
    pub const MEMBER_OF_ENUM: u8 = 16;
    pub const REGISTER_PARAM: u8 = 17;
    pub const BIT_FIELD: u8 = 18;
    pub const BLOCK: u8 = 100;
    /// `.bf`, `.ef`, and `.lf`.
    pub const FUNCTION: u8 = 101;
    pub const END_OF_STRUCT: u8 = 102;
    /// `.file`.
    pub const FILE: u8 = 103;
    pub const SECTION: u8 = 104;
    pub const WEAK_EXTERNAL: u8 = 105;
    pub const CLR_TOKEN: u8 = 107;
}

/// The `Type` field's complex type for a function (`IMAGE_SYM_DTYPE_FUNCTION
/// << 4`), and its mask.
pub mod symbol_type {
    /// A function: `0x20`, as MSVC and LLVM write for every function.
    pub const FUNCTION: u16 = 0x20;
    /// The complex type's bits.
    pub const COMPLEX_MASK: u16 = 0x30;
}

/// `IMAGE_COMDAT_SELECT_*` values.
pub mod comdat_select {
    pub const NODUPLICATES: u8 = 1;
    pub const ANY: u8 = 2;
    pub const SAME_SIZE: u8 = 3;
    pub const EXACT_MATCH: u8 = 4;
    pub const ASSOCIATIVE: u8 = 5;
    pub const LARGEST: u8 = 6;
    pub const NEWEST: u8 = 7;
}

/// `IMAGE_WEAK_EXTERN_SEARCH_*`: how a weak external's default is used.
pub mod weak_search {
    /// No library search for the symbol: the default is used if no object
    /// defines it.
    pub const NOLIBRARY: u32 = 1;
    /// Libraries are searched for the symbol before the default is used.
    pub const LIBRARY: u32 = 2;
    /// The symbol is an alias of the default.
    pub const ALIAS: u32 = 3;
    /// An anti-dependency (ARM64EC): the default is used unless the symbol
    /// is defined, and the symbol doesn't pull a member in.
    pub const ANTI_DEPENDENCY: u32 = 4;
}

/// One symbol: a primary record and its auxiliary records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol<'data> {
    /// The name. Names longer than 8 bytes are stored in the string table.
    pub name: Name<'data>,
    /// `Value`: an offset in the section, a common symbol's size, or an
    /// absolute value.
    pub value: u32,
    /// `SectionNumber`.
    pub section: SectionNumber,
    /// `Type` (`0x20` for a function, otherwise usually 0).
    pub kind: u16,
    /// `StorageClass` (see [`storage_class`]).
    pub storage_class: u8,
    /// The auxiliary records.
    pub aux: Aux<'data>,
}

impl<'data> Symbol<'data> {
    /// A symbol with no auxiliary records.
    pub fn new(
        name: impl Into<Name<'data>>,
        value: u32,
        section: SectionNumber,
        kind: u16,
        storage_class: u8,
    ) -> Self {
        Self {
            name: name.into(),
            value,
            section,
            kind,
            storage_class,
            aux: Aux::None,
        }
    }

    /// The section definition, if this symbol has one.
    pub fn section_definition(&self) -> Option<&SectionDefinition> {
        match &self.aux {
            Aux::Section(d) => Some(d),
            _ => None,
        }
    }

    /// The section definition, mutably.
    pub fn section_definition_mut(&mut self) -> Option<&mut SectionDefinition> {
        match &mut self.aux {
            Aux::Section(d) => Some(d),
            _ => None,
        }
    }

    /// Whether this is an external symbol (defined or not).
    pub fn is_external(&self) -> bool {
        self.storage_class == storage_class::EXTERNAL
    }

    /// The symbol with what it borrows copied.
    pub fn into_owned(self) -> Symbol<'static> {
        Symbol {
            name: self.name.into_owned(),
            value: self.value,
            section: self.section,
            kind: self.kind,
            storage_class: self.storage_class,
            aux: self.aux.into_owned(),
        }
    }
}

/// A symbol's auxiliary records.
///
/// A symbol's records are read as one of the typed forms when its storage
/// class, type, and section call for it, it has the number of records that
/// form has, and the typed form writes back the same bytes (its unused
/// fields are zero, and the symbols and sections it names exist).
/// Otherwise they are kept as [`Aux::Raw`], which is written as it is: its
/// symbol indices and section numbers aren't renumbered by edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Aux<'data> {
    /// No auxiliary records.
    None,
    /// A function definition's record (an external function defined in a
    /// section).
    Function(FunctionDefinition),
    /// A `.bf` or `.ef` record (storage class `FUNCTION`).
    BeginEnd(BeginEnd),
    /// A weak external's record (storage class `WEAK_EXTERNAL`, or an
    /// undefined external of value 0).
    WeakExternal(WeakExternal),
    /// A `.file` symbol's file name (storage class `FILE`), in as many
    /// records as it needs, padded with NULs.
    File(Cow<'data, [u8]>),
    /// A section definition (a static symbol: the section's own symbol).
    Section(SectionDefinition),
    /// A CLR token's record (storage class `CLR_TOKEN`).
    ClrToken(ClrToken),
    /// Records kept as bytes: as many as their length is a multiple of the
    /// record size.
    Raw(Cow<'data, [u8]>),
}

impl Aux<'_> {
    /// The number of auxiliary records, for records of `record_size`
    /// bytes.
    pub fn count(&self, record_size: usize) -> usize {
        match self {
            Aux::None => 0,
            Aux::File(name) => name.len().div_ceil(record_size),
            Aux::Raw(bytes) => bytes.len() / record_size,
            _ => 1,
        }
    }

    /// The records with what they borrow copied.
    pub fn into_owned(self) -> Aux<'static> {
        match self {
            Aux::None => Aux::None,
            Aux::Function(f) => Aux::Function(f),
            Aux::BeginEnd(b) => Aux::BeginEnd(b),
            Aux::WeakExternal(w) => Aux::WeakExternal(w),
            Aux::File(name) => Aux::File(Cow::Owned(name.into_owned())),
            Aux::Section(s) => Aux::Section(s),
            Aux::ClrToken(c) => Aux::ClrToken(c),
            Aux::Raw(bytes) => Aux::Raw(Cow::Owned(bytes.into_owned())),
        }
    }
}

/// A function definition's auxiliary record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FunctionDefinition {
    /// `TagIndex`: the function's `.bf` symbol (0 in the file for none).
    pub tag: Option<SymbolId>,
    /// `TotalSize`: the size of the function's code.
    pub total_size: u32,
    /// `PointerToLinenumber`: the function's first line number record (a
    /// file offset in the file, 0 for none).
    pub line_numbers: Option<LineNumberRef>,
    /// `PointerToNextFunction`: the next function's symbol (0 for none).
    pub next: Option<SymbolId>,
}

/// A line number record: the section that holds it, and its position in
/// [`Section::line_numbers`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineNumberRef {
    /// The section.
    pub section: SectionId,
    /// The record's position in the section's line numbers.
    pub index: u32,
}

/// A `.bf` or `.ef` symbol's auxiliary record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BeginEnd {
    /// `Linenumber`: the function's first (`.bf`) or last (`.ef`) source
    /// line.
    pub line: u16,
    /// `PointerToNextFunction` (`.bf` only): the next function's `.bf`
    /// symbol (0 for none).
    pub next: Option<SymbolId>,
}

/// A weak external's auxiliary record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WeakExternal {
    /// `TagIndex`: the default symbol.
    pub tag: SymbolId,
    /// `Characteristics` (see [`weak_search`]).
    pub search: u32,
}

/// A section definition's auxiliary record.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SectionDefinition {
    /// `Length`: the section's size.
    pub length: u32,
    /// `NumberOfRelocations` (which tools cap at 0xFFFF).
    pub relocations: u16,
    /// `NumberOfLinenumbers`.
    pub line_numbers: u16,
    /// `CheckSum`: for a COMDAT, a CRC of its data (see
    /// [`checksum`](crate::checksum)).
    pub checksum: u32,
    /// `Number` (and, in big-obj, `HighNumber`): for an associative COMDAT,
    /// the section it is associated with. LLVM also writes each
    /// non-associative section's own number here; MSVC writes 0.
    pub number: Option<SectionId>,
    /// `Selection` (see [`comdat_select`]): for a COMDAT, how the linker
    /// chooses among copies.
    pub selection: u8,
}

/// A CLR token's auxiliary record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ClrToken {
    /// `bAuxType`: 1 (`IMAGE_AUX_SYMBOL_TYPE_TOKEN_DEF`).
    pub aux_type: u8,
    /// `SymbolTableIndex`: the symbol the token defines.
    pub symbol: SymbolId,
}

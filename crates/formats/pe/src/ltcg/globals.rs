//! `.cil$gl`, the global symbol table: a 30-byte header, then records one
//! after another, each led by a tag ([`tag`]), the linker directives last.
//! See docs/ltcg-il.md for each record's fields and the evidence for them.
//!
//! The records are walked in turn. Should one not be understood (a tag or
//! a field's value not seen before), what follows is still searched for
//! the functions' and variables' heads, as this reader did before it
//! walked them ([`Globals::walked`] says which).

use alloc::vec::Vec;
use turbonop_core::{Error, ErrorKind, Result};

use super::{Reader, Types};

/// The records' tags.
pub mod tag {
    /// A variable (or a function's local) of a scalar type.
    pub const SCALAR: u8 = 0x01;
    /// A variable of an aggregate type (an array, a structure): as
    /// [`SCALAR`], with its alignment.
    pub const AGGREGATE: u8 = 0x02;
    /// A function declared (defined elsewhere, or not at all).
    pub const FUNCTION: u8 = 0x04;
    /// A `.cil$sy` stream's end.
    pub const END: u8 = 0x06;
    /// A local static (`.cil$sy`): a function's own name for a global.
    pub const LOCAL_STATIC: u8 = 0x07;
    /// A constant (`const` or `constexpr` with a value the IL keeps).
    pub const CONSTANT: u8 = 0x08;
    /// A section (`#pragma section`, `data_seg`, `code_seg`; the compiler's
    /// own, such as `.CRT$XCU` and `.rdata$r`).
    pub const SECTION: u8 = 0x09;
    /// The linker directives, which end `.cil$gl`.
    pub const DIRECTIVES: u8 = 0x0A;
    /// A named type (a class, an enumeration, a typedef).
    pub const TYPE: u8 = 0x0B;
    /// A scope's start (`.cil$sy`).
    pub const BEGIN: u8 = 0x0D;
    /// A function with a body.
    pub const BODY: u8 = 0x0E;
    /// A vector deleting destructor (`??_E`), declared: as [`FUNCTION`],
    /// with the scalar deleting destructor (`??_G`) it calls.
    pub const VECTOR_DELETING: u8 = 0x10;
    /// A source file, with its checksum.
    pub const SOURCE_FILE: u8 = 0x12;
    /// Three compressed values (the table's first records: what they
    /// describe is not known).
    pub const PREDEFINED: u8 = 0x18;
    /// An unnamed local (`.cil$sy`): a temporary the compiler made.
    pub const UNNAMED: u8 = 0x1A;
    /// A namespace.
    pub const NAMESPACE: u8 = 0x1C;
    /// An attribute (`[[nodiscard]]`, `[[msvc::forceinline]]`, ...).
    pub const ATTRIBUTE: u8 = 0x1D;
    /// Five bytes, all zero where seen (C++20 objects only).
    pub const CXX20: u8 = 0x1E;
}

/// A source file the object was compiled from (the main file and every
/// header), with its checksum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceFile<'data> {
    pub id: u32,
    pub path: &'data [u8],
    /// CodeView's checksum kind: 1 MD5, 2 SHA-1, 3 SHA-256.
    pub checksum_kind: u8,
    pub checksum: &'data [u8],
}

/// A linker directive, from `#pragma comment(lib)` or `(linker)`, or
/// `#pragma detect_mismatch`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Directive<'data> {
    pub kind: DirectiveKind,
    pub text: &'data [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectiveKind {
    /// A library to search (`#pragma comment(lib)`: `/DEFAULTLIB`).
    Library,
    /// A linker option (`#pragma comment(linker)`, `detect_mismatch`'s
    /// `/FAILIFMISMATCH`).
    Linker,
    Other(u8),
}

impl DirectiveKind {
    fn of(b: u8) -> Self {
        match b {
            0x04 => Self::Library,
            0x0A => Self::Linker,
            other => Self::Other(other),
        }
    }
}

/// How a symbol is defined or declared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Storage {
    /// Defined here, with external linkage.
    Defined,
    /// Defined here and `__declspec(dllexport)`.
    Exported,
    /// Declared only (defined elsewhere).
    Declared,
    /// Declared `__declspec(dllimport)`.
    Imported,
    /// Defined here, internal linkage (`static`).
    Static,
    /// A C tentative definition (`int x;`): a common symbol.
    Common,
    Unknown(u8),
}

impl Storage {
    /// Whether the symbol is defined here, visible to other objects.
    pub fn is_public_definition(self) -> bool {
        matches!(self, Self::Defined | Self::Exported | Self::Common)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Defined => "defined",
            Self::Exported => "dllexport",
            Self::Declared => "declared",
            Self::Imported => "dllimport",
            Self::Static => "static",
            Self::Common => "common",
            Self::Unknown(_) => "unknown",
        }
    }

    fn of_function(b: u8) -> Self {
        match b {
            0x03 => Self::Static,
            0x04 => Self::Declared,
            0x05 => Self::Defined,
            0x09 => Self::Exported,
            0x0A => Self::Imported,
            other => Self::Unknown(other),
        }
    }

    fn of_data(b: u8) -> Self {
        match b {
            0x01 => Self::Defined,
            0x02 => Self::Declared,
            0x03 => Self::Common,
            0x04 => Self::Static,
            0x07 => Self::Exported,
            0x08 => Self::Imported,
            other => Self::Unknown(other),
        }
    }
}

/// A function's attributes ([`SymbolKind::Function`]'s `attributes`).
pub mod function {
    /// The callee pops its arguments (`__stdcall`, x86 `__thiscall`).
    pub const CALLEE_POPS: u8 = 0x10;
    pub const INLINE: u8 = 0x20;
    /// It takes variable arguments.
    pub const VARARGS: u8 = 0x40;
    pub const NORETURN: u8 = 0x80;
    /// [`super::FunctionIl::body_flags`]: the definition is emitted
    /// whether or not anything refers to it (a function not inline, an
    /// explicit instantiation).
    pub const EMITTED: u32 = 0x20;
}

/// A variable's attributes ([`Traits::attributes`] of a variable).
pub mod data {
    /// It has an initializer in `.cil$in` (and its record lists what the
    /// initializer refers to).
    pub const INITIALIZED: u8 = 0x80;
    /// `__declspec(selectany)` (or C++17's `inline`, with an
    /// initializer): any one definition is kept.
    pub const SELECTANY: u8 = 0x40;
    /// Defined in every object that needs it, kept only if something
    /// refers to it: string literals, RTTI and vftables, variable
    /// templates, local statics of inline functions.
    pub const COMDAT: u8 = 0x20;
    /// Seen on vftables only.
    pub const VFTABLE: u8 = 0x10;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Function {
        storage: Storage,
        /// [`function`]'s bits.
        attributes: u8,
        /// Whether its record has a body (tag 0x0E; 0x04 is declared,
        /// 0x10 a virtual function declared).
        body: bool,
    },
    Data {
        storage: Storage,
        writable: bool,
        /// Its size, as the IL sees it (pointers that will become RVAs
        /// counted at pointer size).
        size: u64,
    },
}

/// A list of ids (each in the IL's id encoding).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ids<'data> {
    data: &'data [u8],
    count: u32,
}

impl Ids<'_> {
    pub fn len(&self) -> usize {
        self.count as usize
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        let mut r = Reader::new(self.data, 0);
        (0..self.count).map_while(move |_| r.id())
    }
}

/// What a function's or variable's initializer refers to: each symbol's
/// id, and how.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Refs<'data> {
    data: &'data [u8],
    count: u32,
}

impl Refs<'_> {
    pub fn len(&self) -> usize {
        self.count as usize
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, RefKind)> + '_ {
        let mut r = Reader::new(self.data, 0);
        (0..self.count).map_while(move |_| {
            let id = r.id()?;
            let kind = r.u8()?;
            Some((id, RefKind::of(kind)))
        })
    }
}

/// The fields every symbol-like record has after its type's description:
/// attributes, flags, the section it goes in, its kind, and what its kind
/// brings (see docs/ltcg-il.md, "the common fields").
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traits<'data> {
    /// A function's [`function`] bits, a variable's [`data`] bits.
    pub attributes: u8,
    /// Flags: 0x80 brings [`Traits::extra`], 0x02 [`Traits::section`]; the
    /// others are not known (0x20 is set on variables whose address is
    /// taken, 0x01 on ones referred to, it seems).
    pub flags: u8,
    /// Sixteen bits more: 0x8000 on variables of class type, 0x0014 on
    /// vftables, 0x0080 on a dynamic initializer and its pointer (whose
    /// references then come without a count).
    pub extra: Option<u16>,
    /// The section record's id, if it goes in a section of its own.
    pub section: Option<u32>,
    /// Four bytes: the kind (the first: for a function, 0x01 a virtual
    /// member, 0x02 a special member or operator, 0x08 pure; for a
    /// variable, 0x02 a vftable or type descriptor), 0x80 in the second on
    /// compiler-made functions, 0x01 in the third when attributes follow
    /// the record, and 0x02 in the fourth from C++17 on.
    pub kind: [u8; 4],
    /// A virtual member function's classes: the class and the bases whose
    /// slots it overrides (CodeView type indexes, it seems), and a byte
    /// before them.
    pub virtual_classes: Option<(u8, Ids<'data>)>,
    /// A special member's or operator's two bytes (the first: 0x01 a
    /// constructor, 0x81 a copy constructor, 0x02 an assignment, 0x03 a
    /// destructor, 0x08 a comparison, 0x09 `operator new`, 0x0A `operator
    /// delete`); a vftable's or type descriptor's.
    pub special: Option<[u8; 2]>,
    /// A type descriptor's (`??_R0`) type: its CodeView type index.
    pub described_type: Option<u32>,
    /// The attribute records ([`Attribute`]) that apply to it.
    pub attribute_ids: Ids<'data>,
}

impl Traits<'_> {
    /// Whether attributes follow the record.
    fn has_attributes(&self) -> bool {
        self.kind[2] & 1 != 0
    }
}

/// What a function with a body (tag 0x0E) records of it: see the module's
/// documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionIl<'data> {
    /// Its type's index in `.cil$db` (an `LF_PROCEDURE` or `LF_MFUNCTION`).
    pub type_index: u32,
    /// Its id record's index in `.cil$db` (an `LF_FUNC_ID` or
    /// `LF_MFUNC_ID`).
    pub id_index: u32,
    /// Where its IL starts in `.cil$ex`.
    pub il_offset: u32,
    /// Where its local symbols start in `.cil$sy`.
    pub symbols_offset: u32,
    /// A measure of its body that grows with it (by about one for each
    /// operation its IL has: the inliner's measure of its size, it seems).
    pub weight: u16,
    /// Bits (the second of two 32-bit fields; the first is 1):
    /// [`function::EMITTED`]; the others are not known.
    pub body_flags: u32,
    /// How many parameters it takes (`this` counted).
    pub params: u16,
    /// What it refers to: each symbol's id, and how.
    refs: Refs<'data>,
}

/// How a function refers to a symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    /// Directly: it calls the function, or reads or writes the variable.
    Direct,
    /// It takes its address.
    Address,
    /// It calls the virtual function through a vtable.
    Virtual,
    Other(u8),
}

impl RefKind {
    fn of(b: u8) -> Self {
        match b {
            0 => Self::Virtual,
            1 => Self::Direct,
            2 => Self::Address,
            other => Self::Other(other),
        }
    }
}

impl FunctionIl<'_> {
    /// What it refers to: each symbol's id (as [`IlSymbol::id`]), and how.
    pub fn refs(&self) -> impl Iterator<Item = (u32, RefKind)> + '_ {
        self.refs.iter()
    }
}

/// A function or variable in the global symbol table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IlSymbol<'data> {
    /// The record's offset in `.cil$gl`.
    pub offset: usize,
    /// Where the record ends (its name's end, if the record was found by
    /// its head rather than walked).
    pub end: usize,
    pub id: u32,
    /// The byte before the name: 0x26 for special members, RTTI, and
    /// vftables the compiler made, 0x25 for string literals, 0x24 for
    /// variables of internal linkage, 0 for most (what its bits mean is
    /// not known).
    pub flags: u8,
    /// Its decorated name (x86 C names without the leading underscore).
    pub name: &'data [u8],
    pub kind: SymbolKind,
    /// A function with a body: its IL, type, and what it refers to.
    pub il: Option<FunctionIl<'data>>,
    /// Its value's type: the low four bits its alignment (2 one byte, 4
    /// two, 6 four, 8 eight, ..., 0xE 64), 0x80 always, 0x40 with an
    /// explicit alignment. (A function's: its return value's.)
    pub machine_type: u8,
    /// The class of its type (1 an integer, 2 unsigned or `bool`, 3 a
    /// pointer, 4 a function pointer, 5 floating point, 6 an aggregate, 7
    /// `void`: as seen).
    pub type_class: u8,
    /// Its type's index in `.cil$db` (or a CodeView primitive's).
    pub type_index: Option<u32>,
    /// A function's id record's index in `.cil$db`.
    pub id_index: Option<u32>,
    /// The common fields (none if the record was found by its head).
    pub traits: Traits<'data>,
    /// A variable's alignment (an aggregate's, tag 0x02).
    pub alignment: Option<u8>,
    /// What a variable's initializer refers to.
    pub data_refs: Refs<'data>,
    /// A vector deleting destructor's scalar deleting destructor's id.
    pub scalar_deleting: Option<u32>,
}

impl<'data> IlSymbol<'data> {
    pub fn storage(&self) -> Storage {
        match self.kind {
            SymbolKind::Function { storage, .. } | SymbolKind::Data { storage, .. } => storage,
        }
    }

    /// What it refers to (a function's body, a variable's initializer):
    /// each symbol's id, and how.
    pub fn refs(&self) -> Refs<'data> {
        self.il.as_ref().map_or(self.data_refs, |il| il.refs)
    }

    /// Whether the compiler emits it whatever refers to it (lib.exe's and
    /// the linker's roots): a function not inline (or explicitly
    /// instantiated); a `static` function not inline; a variable not
    /// COMDAT (or `selectany`), static or not; a common symbol.
    pub fn is_root(&self) -> bool {
        match self.kind {
            SymbolKind::Function {
                storage,
                attributes,
                ..
            } => match &self.il {
                Some(il) if il.body_flags & function::EMITTED != 0 => true,
                Some(_) => storage == Storage::Static && attributes & function::INLINE == 0,
                None => false,
            },
            SymbolKind::Data { storage, .. } => match storage {
                Storage::Common => true,
                Storage::Defined | Storage::Exported | Storage::Static => {
                    let a = self.traits.attributes;
                    a & data::COMDAT == 0 || a & data::SELECTANY != 0
                }
                _ => false,
            },
        }
    }
}

/// A section record: one the source names (`#pragma section`,
/// `data_seg`, `__declspec(allocate)`, `code_seg`) or the compiler does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section<'data> {
    pub offset: usize,
    pub id: u32,
    pub name: &'data [u8],
    /// Flags (0x01, 0x04, 0x07, 0x1D seen; what they mean is not known).
    pub flags: u8,
    /// Its class (`CODE`, `DATA`, `CONST`, or none).
    pub class: &'data [u8],
    /// Its COFF section characteristics, if the source gave them (0
    /// otherwise).
    pub characteristics: u32,
}

/// A namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Namespace<'data> {
    pub offset: usize,
    pub name: &'data [u8],
    pub traits: Traits<'data>,
}

/// A named type: a class, an enumeration, a typedef (its qualified name,
/// as written, not decorated).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamedType<'data> {
    pub offset: usize,
    pub id: u32,
    pub name: &'data [u8],
    /// Its index in `.cil$db` (or a CodeView primitive's).
    pub type_index: u32,
    pub traits: Traits<'data>,
}

/// A constant: a `const` or `constexpr` variable whose value the IL
/// keeps (in the record, not in `.cil$in`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constant<'data> {
    pub offset: usize,
    pub id: u32,
    pub name: &'data [u8],
    pub type_index: u32,
    pub machine_type: u8,
    pub type_class: u8,
    /// Its type's size.
    pub size: u8,
    /// Its value: little-endian, at least two bytes, sign-extended.
    pub value: &'data [u8],
}

impl Constant<'_> {
    /// Its value as an integer (sign-extended from its bytes).
    pub fn integer(&self) -> Option<i64> {
        let v = self.value;
        if v.is_empty() || v.len() > 8 {
            return None;
        }
        let mut b = if v[v.len() - 1] & 0x80 != 0 {
            [0xFF; 8]
        } else {
            [0; 8]
        };
        b[..v.len()].copy_from_slice(v);
        Some(i64::from_le_bytes(b))
    }
}

/// An attribute (`[[nodiscard]]`, `[[deprecated("why")]]`,
/// `[[msvc::forceinline]]`), which the records it applies to list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribute<'data> {
    pub offset: usize,
    pub id: u32,
    /// Its namespace (`msvc`), or none.
    pub scope: &'data [u8],
    pub name: &'data [u8],
    /// Its arguments: each a kind (0 a string) and its text.
    pub args: Vec<(u8, &'data [u8])>,
}

/// `.cil$gl`, read.
#[derive(Clone, Debug)]
pub struct Globals<'data> {
    /// The 30-byte header (what it holds is not known).
    pub header: &'data [u8],
    pub source_files: Vec<SourceFile<'data>>,
    pub symbols: Vec<IlSymbol<'data>>,
    pub directives: Vec<Directive<'data>>,
    pub sections: Vec<Section<'data>>,
    pub namespaces: Vec<Namespace<'data>>,
    pub types: Vec<NamedType<'data>>,
    pub constants: Vec<Constant<'data>>,
    pub attributes: Vec<Attribute<'data>>,
    /// The predefined records' values (tag 0x18).
    pub predefined: Vec<[u16; 3]>,
    /// Where each record starts and ends, and its tag, in order (the
    /// records walked).
    pub spans: Vec<(usize, usize, u8)>,
    /// Whether every record was walked in turn; else where the first not
    /// understood starts (what follows was searched for the functions' and
    /// variables' heads).
    pub walked: core::result::Result<(), usize>,
}

impl Globals<'_> {
    /// Each symbol's IL size (a function with a body's; `None` for the
    /// rest), in the order of [`Globals::symbols`]: from its offset in
    /// `.cil$ex` (`il_len` bytes long) to the next body's, or the end. (A
    /// function's IL unit, [`IlUnits`](super::IlUnits), is exact where
    /// this counts the IL of functions with no record that follow it.)
    pub fn il_sizes(&self, il_len: usize) -> Vec<Option<u32>> {
        let mut at: Vec<(u32, usize)> = self
            .symbols
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((s.il?.il_offset, i)))
            .collect();
        at.sort_unstable();
        let mut out = alloc::vec![None; self.symbols.len()];
        let end = u32::try_from(il_len).unwrap_or(u32::MAX);
        for (k, &(offset, i)) in at.iter().enumerate() {
            let next = at.get(k + 1).map_or(end, |n| n.0);
            out[i] = next.checked_sub(offset);
        }
        out
    }

    /// Whether lib.exe lists each symbol (in the order of
    /// [`Globals::symbols`]) in a library's index: the public definitions
    /// (and commons) that a root ([`IlSymbol::is_root`]) refers to, through
    /// any number of others, directly or by address (a virtual call needs
    /// the vftable, not the function); a vector deleting destructor refers
    /// to its scalar deleting destructor. Inline functions nothing refers
    /// to that way aren't listed. (Checked against lib.exe's index for
    /// some 440 objects: x64, x86, and ARM64.)
    pub fn lib_listed(&self) -> Vec<bool> {
        let mut by_id: Vec<(u32, usize)> = self
            .symbols
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id, i))
            .collect();
        by_id.sort_unstable();
        let find = |id: u32| -> Option<usize> {
            by_id
                .binary_search_by_key(&id, |e| e.0)
                .ok()
                .map(|k| by_id[k].1)
        };
        let mut seen = alloc::vec![false; self.symbols.len()];
        let mut work: Vec<usize> = Vec::new();
        for (i, s) in self.symbols.iter().enumerate() {
            if s.is_root() {
                seen[i] = true;
                work.push(i);
            }
        }
        while let Some(i) = work.pop() {
            let s = &self.symbols[i];
            let refs = s.refs();
            let targets = refs
                .iter()
                .filter(|&(_, how)| how != RefKind::Virtual)
                .map(|(id, _)| id)
                .chain(s.scalar_deleting);
            for id in targets {
                if let Some(j) = find(id)
                    && !seen[j]
                {
                    seen[j] = true;
                    work.push(j);
                }
            }
        }
        self.symbols
            .iter()
            .zip(seen)
            .map(|(s, reached)| reached && s.storage().is_public_definition())
            .collect()
    }

    /// The symbol whose id is `id`.
    pub fn symbol(&self, id: u32) -> Option<&IlSymbol<'_>> {
        self.symbols.iter().find(|s| s.id == id)
    }
}

/// The size of `.cil$gl`'s header.
pub(super) const GLOBALS_HEADER: usize = 30;

/// Whether `name` could be a symbol's name: of the characters decorated
/// and C names are made of, starting as they do.
fn is_name(name: &[u8]) -> bool {
    matches!(
        name.first(),
        Some(b'?' | b'_' | b'$' | b'@' | b'a'..=b'z' | b'A'..=b'Z')
    ) && name.iter().all(|&b| {
        b.is_ascii_alphanumeric() || matches!(b, b'?' | b'@' | b'$' | b'_' | b'<' | b'>' | b'-')
    })
}

/// The common fields ([`Traits`]), but for the attributes, which follow
/// the record's end. `function`: a function's record (whose kind brings
/// other fields than a variable's).
pub(super) fn traits<'d>(r: &mut Reader<'d>, function: bool) -> Option<Traits<'d>> {
    let attributes = r.u8()?;
    let flags = r.u8()?;
    let extra = if flags & 0x80 != 0 {
        Some(r.u16()?)
    } else {
        None
    };
    let section = if flags & 0x02 != 0 {
        Some(r.id()?)
    } else {
        None
    };
    // (Always 1 where seen.)
    if r.u32()? != 1 {
        return None;
    }
    let kind = r.u32()?.to_le_bytes();
    let mut t = Traits {
        attributes,
        flags,
        extra,
        section,
        kind,
        ..Traits::default()
    };
    let k = kind[0];
    if function {
        if k & 0x01 != 0 {
            let first = r.u8()?;
            let n = r.u8()?;
            let start = r.p;
            for _ in 0..n {
                r.id()?;
            }
            t.virtual_classes = Some((
                first,
                Ids {
                    data: &r.d[start..r.p],
                    count: n.into(),
                },
            ));
        }
        if k & 0x02 != 0 {
            t.special = Some([r.u8()?, r.u8()?]);
        }
        // (0x80 and 0x08 bring nothing; other bits are not known.)
        if k & 0x74 != 0 {
            return None;
        }
    } else {
        match k & 0x7F {
            0 => {}
            2 => {
                let p = r.u8()?;
                let q = r.u8()?;
                t.special = Some([p, q]);
                if p & 1 != 0 {
                    t.described_type = Some(r.leb()?);
                }
            }
            _ => return None,
        }
    }
    Some(t)
}

/// The attributes that end a record, if its kind says it has them.
pub(super) fn attribute_list<'d>(r: &mut Reader<'d>, t: &mut Traits<'d>) -> Option<()> {
    if t.has_attributes() {
        let n = r.c16()?;
        let start = r.p;
        for _ in 0..n {
            r.id()?;
        }
        t.attribute_ids = Ids {
            data: &r.d[start..r.p],
            count: n.into(),
        };
    }
    Some(())
}

/// References: a compressed count, then each id and how.
fn refs<'d>(r: &mut Reader<'d>, count: u32) -> Option<Refs<'d>> {
    let start = r.p;
    for _ in 0..count {
        r.id()?;
        r.u8()?;
    }
    Some(Refs {
        data: &r.d[start..r.p],
        count,
    })
}

/// A value's type: its machine type (and the byte an explicit alignment
/// brings) and its class.
fn value_type(r: &mut Reader<'_>) -> Option<(u8, u8)> {
    let mt = r.u8()?;
    if mt & 0x40 != 0 {
        r.u8()?;
    }
    Some((mt, r.u8()?))
}

/// A variable's record from its tag (0x01 or 0x02) on: `local`, a
/// `.cil$sy` local's (whose fields after its name are the same).
pub(super) fn data_record<'d>(r: &mut Reader<'d>, offset: usize) -> Option<IlSymbol<'d>> {
    let tag = r.u8()?;
    let _scope = r.u8()?;
    let id = r.id()?;
    let flags = r.u8()?;
    let name = r.sz()?;
    let (machine_type, type_class) = value_type(r)?;
    let _zero = r.u8()?;
    let writable = r.u8()? == 0x02;
    let storage = Storage::of_data(r.u8()?);
    let size = r.c32()?;
    let mut traits = traits(r, false)?;
    let type_index = r.c32()?;
    let alignment = if tag == tag::AGGREGATE {
        Some(r.u8()?)
    } else {
        None
    };
    // (A dynamic initializer's pointer lists the variable and the
    // initializer without a count.)
    let data_refs = if traits.extra.is_some_and(|x| x & 0x80 != 0) {
        refs(r, 2)?
    } else if traits.attributes & data::INITIALIZED != 0 {
        let n = r.c32()?;
        refs(r, n)?
    } else {
        Refs::default()
    };
    attribute_list(r, &mut traits)?;
    Some(IlSymbol {
        offset,
        end: r.p,
        id,
        flags,
        name,
        kind: SymbolKind::Data {
            storage,
            writable,
            size: size.into(),
        },
        il: None,
        machine_type,
        type_class,
        type_index: Some(type_index),
        id_index: None,
        traits,
        alignment,
        data_refs,
        scalar_deleting: None,
    })
}

fn function_record<'d>(r: &mut Reader<'d>, offset: usize) -> Option<IlSymbol<'d>> {
    let tag = r.u8()?;
    let id = r.id()?;
    let flags = r.u8()?;
    let name = r.sz()?;
    let (machine_type, type_class) = value_type(r)?;
    let storage = Storage::of_function(r.u8()?);
    let _return_size = r.u8()?;
    let mut traits = traits(r, true)?;
    let type_index = r.c32()?;
    let id_index = r.u32()?;
    let mut il = None;
    let mut scalar_deleting = None;
    if tag == tag::BODY {
        let il_offset = r.c32()?;
        let symbols_offset = r.c32()?;
        let weight = r.c16()?;
        if r.u32()? != 1 {
            return None;
        }
        let body_flags = r.u32()?;
        let params = r.c16()?;
        let n = r.c32()?;
        let refs = refs(r, n)?;
        il = Some(FunctionIl {
            type_index,
            id_index,
            il_offset,
            symbols_offset,
            weight,
            body_flags,
            params,
            refs,
        });
    } else if tag == tag::VECTOR_DELETING {
        scalar_deleting = Some(r.id()?);
    }
    attribute_list(r, &mut traits)?;
    Some(IlSymbol {
        offset,
        end: r.p,
        id,
        flags,
        name,
        kind: SymbolKind::Function {
            storage,
            attributes: traits.attributes,
            body: tag == tag::BODY,
        },
        il,
        machine_type,
        type_class,
        type_index: Some(type_index),
        id_index: Some(id_index),
        traits,
        alignment: None,
        data_refs: Refs::default(),
        scalar_deleting,
    })
}

/// The source file whose record (tag 0x12, id, path, checksum's length
/// and kind, checksum) starts at `p`, and where it ends.
fn file_at(g: &[u8], p: usize) -> Option<(SourceFile<'_>, usize)> {
    if *g.get(p)? != tag::SOURCE_FILE {
        return None;
    }
    let mut r = Reader::new(g, p + 1);
    let id = r.id()?;
    let path = r.sz()?;
    let len = usize::from(r.u8()?);
    let checksum_kind = r.u8()?;
    let plausible = path.len() >= 2
        && path.iter().all(|&b| b >= 0x20)
        && (path.contains(&b'\\') || path.contains(&b'/'))
        && matches!((checksum_kind, len), (1, 16) | (2, 20) | (3, 32) | (0, 0));
    if !plausible {
        return None;
    }
    let checksum = r.take(len)?;
    Some((
        SourceFile {
            id,
            path,
            checksum_kind,
            checksum,
        },
        r.p,
    ))
}

fn directives_at(g: &[u8], at: usize) -> Option<Vec<Directive<'_>>> {
    let mut r = Reader::new(g, at.checked_add(3)?);
    if g.get(at..at.checked_add(3)?)? != [tag::DIRECTIVES, 0, 0] {
        return None;
    }
    let count = r.u8()?;
    let mut out = Vec::with_capacity(count.into());
    for _ in 0..count {
        let kind = r.u8()?;
        let text = r.sz()?;
        if text.is_empty() || !text.iter().all(|&b| b >= 0x20) {
            return None;
        }
        out.push(Directive {
            kind: DirectiveKind::of(kind),
            text,
        });
    }
    (r.p == g.len()).then_some(out)
}

impl<'data> Globals<'data> {
    pub fn parse(g: &'data [u8]) -> Result<Self> {
        if g.len() < GLOBALS_HEADER + 4 {
            return Err(Error::new(ErrorKind::Truncated, 0, ".cil$gl header"));
        }
        let mut out = Self {
            header: &g[..GLOBALS_HEADER],
            source_files: Vec::new(),
            symbols: Vec::new(),
            directives: Vec::new(),
            sections: Vec::new(),
            namespaces: Vec::new(),
            types: Vec::new(),
            constants: Vec::new(),
            attributes: Vec::new(),
            predefined: Vec::new(),
            spans: Vec::new(),
            walked: Ok(()),
        };
        let mut p = GLOBALS_HEADER;
        loop {
            if p >= g.len() {
                // (No directives record: not one walked to the end.)
                out.walked = Err(p);
                break;
            }
            match out.record(g, p) {
                Some(None) => break,
                Some(Some(end)) => p = end,
                None => {
                    out.walked = Err(p);
                    break;
                }
            }
        }
        if let Err(at) = out.walked {
            out.scan(g, at)?;
        }
        Ok(out)
    }

    /// Reads the record at `p`: where it ends, or `None` inside for the
    /// directives (the last); `None` if it is not understood.
    fn record(&mut self, g: &'data [u8], p: usize) -> Option<Option<usize>> {
        let tag = *g.get(p)?;
        let mut r = Reader::new(g, p);
        match tag {
            tag::DIRECTIVES => {
                self.directives = directives_at(g, p)?;
                self.spans.push((p, g.len(), tag));
                return Some(None);
            }
            tag::PREDEFINED => {
                r.u8()?;
                self.predefined.push([r.c16()?, r.c16()?, r.c16()?]);
            }
            tag::SOURCE_FILE => {
                let (file, end) = file_at(g, p)?;
                self.source_files.push(file);
                r.p = end;
            }
            tag::SECTION => {
                r.u8()?;
                let id = r.id()?;
                let name = r.sz()?;
                let flags = r.u8()?;
                let class = r.sz()?;
                let characteristics = r.u32()?;
                self.sections.push(Section {
                    offset: p,
                    id,
                    name,
                    flags,
                    class,
                    characteristics,
                });
            }
            tag::NAMESPACE => {
                r.u8()?;
                let name = r.sz()?;
                let mut traits = traits(&mut r, false)?;
                attribute_list(&mut r, &mut traits)?;
                self.namespaces.push(Namespace {
                    offset: p,
                    name,
                    traits,
                });
            }
            tag::TYPE => {
                let t = named_type_at(&mut r, p)?;
                self.types.push(t);
            }
            tag::CONSTANT => {
                let c = constant_at(&mut r, p)?;
                self.constants.push(c);
            }
            tag::ATTRIBUTE => {
                let a = attribute(&mut r, p)?;
                self.attributes.push(a);
            }
            tag::CXX20 => {
                r.take(6)?;
            }
            tag::SCALAR | tag::AGGREGATE => {
                let s = data_record(&mut r, p)?;
                self.symbols.push(s);
            }
            tag::BODY | tag::FUNCTION | tag::VECTOR_DELETING => {
                let s = function_record(&mut r, p)?;
                self.symbols.push(s);
            }
            _ => return None,
        }
        self.spans.push((p, r.p, tag));
        Some(Some(r.p))
    }

    /// What follows a record not understood (at `from`): the directives,
    /// found from the end; and the functions and variables, by their
    /// heads (as this reader found them before it walked the records).
    fn scan(&mut self, g: &'data [u8], from: usize) -> Result<()> {
        let (directives, directives_at) = Self::find_directives(g, from);
        self.directives = directives;
        let end = directives_at.min(g.len());
        let mut p = from;
        while p < end {
            // (Source files come among the symbols too: those first
            // included where they are.)
            if let Some((file, e)) = file_at(&g[..end], p) {
                self.source_files.push(file);
                p = e;
                continue;
            }
            match symbol_at(&g[..end], p) {
                Some((mut s, e)) => {
                    // (A record head that starts a few bytes later and
                    // ends at the same name has the longer name: it is
                    // the record, the first a run of bytes before it,
                    // its id's, that happened to look like one.)
                    for q in p + 1..(p + 8).min(e) {
                        if let Some((t, e2)) = symbol_at(&g[..end], q)
                            && e2 == e
                            && t.name.len() > s.name.len()
                        {
                            s = t;
                        }
                    }
                    self.symbols.push(s);
                    p = e;
                }
                None => p += 1,
            }
        }
        Ok(())
    }

    /// The directives record (tag 0x0A, two zero bytes, a count, and
    /// (kind, string) pairs), which ends the table; and where it starts.
    fn find_directives(g: &'data [u8], from: usize) -> (Vec<Directive<'data>>, usize) {
        // (The last place one parses to the very end; searched for from
        // the end.)
        let mut at = g.len().saturating_sub(4);
        while at >= from {
            if let Some(d) = directives_at(g, at) {
                return (d, at);
            }
            if at == 0 {
                break;
            }
            at -= 1;
        }
        (Vec::new(), g.len())
    }
}

pub(super) fn named_type_at<'d>(r: &mut Reader<'d>, offset: usize) -> Option<NamedType<'d>> {
    r.u8()?;
    let _scope = r.u8()?;
    let id = r.id()?;
    let name = r.sz()?;
    let _zero = r.u8()?;
    let type_index = r.c32()?;
    let mut traits = traits(r, false)?;
    attribute_list(r, &mut traits)?;
    Some(NamedType {
        offset,
        id,
        name,
        type_index,
        traits,
    })
}

pub(super) fn constant_at<'d>(r: &mut Reader<'d>, offset: usize) -> Option<Constant<'d>> {
    r.u8()?;
    let _scope = r.u8()?;
    let id = r.id()?;
    let name = r.sz()?;
    let type_index = r.c32()?;
    let (machine_type, type_class) = value_type(r)?;
    let _zero = r.u8()?;
    let size = r.u8()?;
    let len = r.u8()?;
    let value = r.take(len.into())?;
    Some(Constant {
        offset,
        id,
        name,
        type_index,
        machine_type,
        type_class,
        size,
        value,
    })
}

fn attribute<'d>(r: &mut Reader<'d>, offset: usize) -> Option<Attribute<'d>> {
    r.u8()?;
    let id = r.id()?;
    let scope = r.lstr()?;
    let name = r.lstr()?;
    let n = r.u8()?;
    let mut args = Vec::with_capacity(n.into());
    for _ in 0..n {
        let kind = r.u8()?;
        args.push((kind, r.lstr()?));
    }
    Some(Attribute {
        offset,
        id,
        scope,
        name,
        args,
    })
}

/// The symbol whose record head starts at `p` (found without walking the
/// records before it), and where its name ends.
fn symbol_at(g: &[u8], p: usize) -> Option<(IlSymbol<'_>, usize)> {
    let tag = *g.get(p)?;
    let function = matches!(tag, tag::BODY | tag::FUNCTION | tag::VECTOR_DELETING);
    let data = matches!(tag, 0x01..=0x03);
    if !function && !data {
        return None;
    }
    // A record walked from here is the record.
    let mut r = Reader::new(g, p);
    let walked = if function {
        function_record(&mut r, p)
    } else {
        data_record(&mut r, p)
    };
    if let Some(s) = walked
        && is_name(s.name)
    {
        let name_end = s.end;
        return Some((s, name_end));
    }
    let mut r = Reader::new(g, p + 1);
    if data && r.u8()? != 0 {
        return None;
    }
    let id = r.id()?;
    let flags = r.u8()?;
    if flags > 0x3F {
        return None;
    }
    let name = r.sz()?;
    if !is_name(name) {
        return None;
    }
    let end = r.p;
    let a = g.get(r.p..r.p.checked_add(6)?)?;
    // (The first attribute: the value's machine type, 0x80-0x8F.)
    if a[0] & 0xF0 != 0x80 {
        return None;
    }
    let kind = if function {
        let storage = match a[2] {
            0x03 => Storage::Static,
            0x04 => Storage::Declared,
            0x05 => Storage::Defined,
            0x09 => Storage::Exported,
            0x0A => Storage::Imported,
            _ => return None,
        };
        SymbolKind::Function {
            storage,
            attributes: a[4],
            body: tag == tag::BODY,
        }
    } else {
        if a[2] != 0 {
            return None;
        }
        let writable = match a[3] {
            0x02 => true,
            0x04 | 0x08 => false,
            _ => return None,
        };
        let storage = match a[4] {
            0x01 => Storage::Defined,
            0x02 => Storage::Declared,
            0x03 => Storage::Common,
            0x04 => Storage::Static,
            0x07 => Storage::Exported,
            _ => return None,
        };
        r.p += 5;
        let size = r.c32()?;
        SymbolKind::Data {
            storage,
            writable,
            size: size.into(),
        }
    };
    let il = if tag == tag::BODY {
        function_il(g, end + 6)
    } else {
        None
    };
    Some((
        IlSymbol {
            offset: p,
            end,
            id,
            flags,
            name,
            kind,
            il,
            machine_type: a[0],
            type_class: a[1],
            type_index: il.map(|i| i.type_index),
            id_index: il.map(|i| i.id_index),
            traits: Traits::default(),
            alignment: None,
            data_refs: Refs::default(),
            scalar_deleting: None,
        },
        end,
    ))
}

/// A function body's record after its attributes (at `p`), found by its
/// type's index and its id's: a compressed type index (`0x80`, then 32
/// bits) and an id index after it, both past `.cil$db`'s first index and
/// the id past the type.
fn function_il(g: &[u8], p: usize) -> Option<FunctionIl<'_>> {
    let first = Types::FIRST_INDEX;
    let at = (p + 8..p + 24).find(|&q| {
        g.get(q) == Some(&0x80)
            && matches!(
                (super::u32_at(g, q + 1), super::u32_at(g, q + 5)),
                (Some(t), Some(i)) if t >= first && t < 0x0100_0000 && i > t && i < 0x0100_0000
            )
    })?;
    let mut r = Reader::new(g, at);
    let type_index = r.c32()?;
    let id_index = r.u32()?;
    let il_offset = r.c32()?;
    let symbols_offset = r.c32()?;
    let weight = r.c16()?;
    r.u32()?;
    let body_flags = r.u32()?;
    let params = r.c16()?;
    let n = r.c32()?;
    let refs = refs(&mut r, n)?;
    Some(FunctionIl {
        type_index,
        id_index,
        il_offset,
        symbols_offset,
        weight,
        body_flags,
        params,
        refs,
    })
}

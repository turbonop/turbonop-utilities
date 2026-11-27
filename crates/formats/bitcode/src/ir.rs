//! LLVM IR modules: the `MODULE_BLOCK` and the top-level blocks around it.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::collections::HashMap;

use turbonop_core::{Error, ErrorKind, Result};

use crate::bitstream::{
    BlockInfo, Cursor, Entry, Stream, StreamKind, TopBlock, Wrapper, idx, low32,
};
use crate::function::{self, FunctionBody};
use crate::metadata::Metadata;
use crate::symtab::Symtab;
use crate::tables::*;
use crate::types::{Name, Type, TypeTable, chars};

/// How a global is linked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Linkage {
    /// `external` (the default).
    External,
    /// `appending`.
    Appending,
    /// `internal`.
    Internal,
    /// `extern_weak`.
    ExternalWeak,
    /// `common`.
    Common,
    /// `private`.
    Private,
    /// `available_externally`.
    AvailableExternally,
    /// `weak`.
    Weak,
    /// `weak_odr`.
    WeakOdr,
    /// `linkonce`.
    LinkOnce,
    /// `linkonce_odr`.
    LinkOnceOdr,
}

impl Linkage {
    /// The linkage a record's code means (LLVM's `getDecodedLinkage`,
    /// obsolete codes upgraded).
    pub fn from_code(v: u64) -> Self {
        match v {
            2 => Self::Appending,
            3 => Self::Internal,
            7 => Self::ExternalWeak,
            8 => Self::Common,
            9 | 13 | 14 => Self::Private,
            12 => Self::AvailableExternally,
            1 | 16 => Self::Weak,
            10 | 17 => Self::WeakOdr,
            4 | 18 => Self::LinkOnce,
            11 | 19 => Self::LinkOnceOdr,
            _ => Self::External,
        }
    }

    /// Its IR keyword (empty for `external`).
    pub fn name(self) -> &'static str {
        match self {
            Self::External => "",
            Self::Appending => "appending",
            Self::Internal => "internal",
            Self::ExternalWeak => "extern_weak",
            Self::Common => "common",
            Self::Private => "private",
            Self::AvailableExternally => "available_externally",
            Self::Weak => "weak",
            Self::WeakOdr => "weak_odr",
            Self::LinkOnce => "linkonce",
            Self::LinkOnceOdr => "linkonce_odr",
        }
    }

    /// Whether the global is local to its module (`internal`, `private`).
    pub fn is_local(self) -> bool {
        matches!(self, Self::Internal | Self::Private)
    }
}

/// A global's visibility.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Visibility {
    /// `default`.
    #[default]
    Default,
    /// `hidden`.
    Hidden,
    /// `protected`.
    Protected,
}

impl Visibility {
    fn from_code(v: u64) -> Self {
        match v {
            1 => Self::Hidden,
            2 => Self::Protected,
            _ => Self::Default,
        }
    }

    /// Its IR keyword (empty for `default`).
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::Hidden => "hidden",
            Self::Protected => "protected",
        }
    }
}

/// A global's DLL storage class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DllStorage {
    /// None.
    #[default]
    Default,
    /// `dllimport`.
    Import,
    /// `dllexport`.
    Export,
}

impl DllStorage {
    fn from_code(v: u64) -> Self {
        match v {
            1 => Self::Import,
            2 => Self::Export,
            _ => Self::Default,
        }
    }
}

/// A global's thread-local mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ThreadLocal {
    /// Not thread-local.
    #[default]
    None,
    /// `thread_local` (general dynamic).
    GeneralDynamic,
    /// `thread_local(localdynamic)`.
    LocalDynamic,
    /// `thread_local(initialexec)`.
    InitialExec,
    /// `thread_local(localexec)`.
    LocalExec,
}

impl ThreadLocal {
    fn from_code(v: u64) -> Self {
        match v {
            0 => Self::None,
            2 => Self::LocalDynamic,
            3 => Self::InitialExec,
            4 => Self::LocalExec,
            _ => Self::GeneralDynamic,
        }
    }
}

/// Whether a global's address is significant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum UnnamedAddr {
    /// It is.
    #[default]
    None,
    /// `unnamed_addr`.
    Global,
    /// `local_unnamed_addr`.
    Local,
}

impl UnnamedAddr {
    fn from_code(v: u64) -> Self {
        match v {
            1 => Self::Global,
            2 => Self::Local,
            _ => Self::None,
        }
    }
}

/// An alignment exponent's alignment (`1 << (e - 1)`, none for 0).
fn alignment(e: u64) -> Option<u64> {
    (1..=64)
        .contains(&e)
        .then(|| 1u64.wrapping_shl(low32(e - 1)))
}

/// The module's producer (`IDENTIFICATION_BLOCK`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identification {
    /// Who wrote it (`LLVM24.0.0git`).
    pub producer: Vec<u8>,
    /// The bitcode epoch (0).
    pub epoch: Option<u64>,
}

/// A comdat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comdat<'d> {
    /// Its name.
    pub name: Name<'d>,
    /// Its selection kind (`COMDAT_SELECTION_KIND_*`).
    pub selection: u32,
}

impl Comdat<'_> {
    /// Its selection kind's IR keyword.
    pub fn selection_name(&self) -> &'static str {
        match self.selection {
            COMDAT_SELECTION_KIND_EXACT_MATCH => "exactmatch",
            COMDAT_SELECTION_KIND_LARGEST => "largest",
            COMDAT_SELECTION_KIND_NO_DUPLICATES => "nodeduplicate",
            COMDAT_SELECTION_KIND_SAME_SIZE => "samesize",
            _ => "any",
        }
    }
}

/// A global variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlobalVar<'d> {
    /// Its name.
    pub name: Name<'d>,
    /// Its value ID.
    pub value: u32,
    /// The type of what it holds.
    pub value_type: u32,
    /// Its address space.
    pub address_space: u32,
    /// Whether it's `constant`.
    pub constant: bool,
    /// Its initializer's value ID (none for a declaration).
    pub initializer: Option<u32>,
    /// Its linkage.
    pub linkage: Linkage,
    /// Its alignment, in bytes.
    pub alignment: Option<u64>,
    /// Its section.
    pub section: Option<Name<'d>>,
    /// Its visibility.
    pub visibility: Visibility,
    /// Its thread-local mode.
    pub thread_local: ThreadLocal,
    /// Its `unnamed_addr`.
    pub unnamed_addr: UnnamedAddr,
    /// Whether it's `externally_initialized`.
    pub externally_initialized: bool,
    /// Its DLL storage class.
    pub dll_storage: DllStorage,
    /// Its comdat (an index into [`Module::comdats`]).
    pub comdat: Option<usize>,
    /// Its attribute list (an index into [`Module::attribute_lists`], plus
    /// one; 0 for none).
    pub attributes: u64,
    /// Whether it's `dso_local`.
    pub dso_local: bool,
    /// Its partition.
    pub partition: Option<Name<'d>>,
    /// Its sanitizer metadata bits.
    pub sanitizer: u64,
    /// Its code model (0 for the default).
    pub code_model: u64,
}

impl GlobalVar<'_> {
    /// Whether it's a declaration (no initializer).
    pub fn is_declaration(&self) -> bool {
        self.initializer.is_none()
    }
}

/// A function: declared or defined.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Function<'d> {
    /// Its name.
    pub name: Name<'d>,
    /// Its value ID.
    pub value: u32,
    /// Its function type.
    pub function_type: u32,
    /// Its calling convention.
    pub calling_conv: u32,
    /// Whether it's only declared here (has no body).
    pub is_declaration: bool,
    /// Its linkage.
    pub linkage: Linkage,
    /// Its attribute list (an index into [`Module::attribute_lists`], plus
    /// one; 0 for none).
    pub attributes: u64,
    /// Its alignment, in bytes.
    pub alignment: Option<u64>,
    /// Its section.
    pub section: Option<Name<'d>>,
    /// Its visibility.
    pub visibility: Visibility,
    /// Its garbage collector.
    pub gc: Option<Name<'d>>,
    /// Its `unnamed_addr`.
    pub unnamed_addr: UnnamedAddr,
    /// Its prologue data's value ID.
    pub prologue: Option<u32>,
    /// Its DLL storage class.
    pub dll_storage: DllStorage,
    /// Its comdat (an index into [`Module::comdats`]).
    pub comdat: Option<usize>,
    /// Its prefix data's value ID.
    pub prefix: Option<u32>,
    /// Its personality function's value ID.
    pub personality: Option<u32>,
    /// Whether it's `dso_local`.
    pub dso_local: bool,
    /// Its address space.
    pub address_space: u32,
    /// Its partition.
    pub partition: Option<Name<'d>>,
    /// Its preferred alignment, in bytes.
    pub preferred_alignment: Option<u64>,
    /// Its body (an index into [`Module::bodies`]), if read.
    pub body: Option<usize>,
}

/// What an alias-like global is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AliasKind {
    /// An `alias`.
    Alias,
    /// An `ifunc`: its target is the resolver.
    IFunc,
}

/// An alias or an ifunc.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alias<'d> {
    /// Which.
    pub kind: AliasKind,
    /// Its name.
    pub name: Name<'d>,
    /// Its value ID.
    pub value: u32,
    /// Its value type.
    pub value_type: u32,
    /// Its address space.
    pub address_space: u32,
    /// The aliasee's (or resolver's) value ID.
    pub target: u32,
    /// Its linkage.
    pub linkage: Linkage,
    /// Its visibility.
    pub visibility: Visibility,
    /// Its DLL storage class.
    pub dll_storage: DllStorage,
    /// Its thread-local mode.
    pub thread_local: ThreadLocal,
    /// Its `unnamed_addr`.
    pub unnamed_addr: UnnamedAddr,
    /// Whether it's `dso_local`.
    pub dso_local: bool,
    /// Its partition.
    pub partition: Option<Name<'d>>,
}

/// What a module-level value ID names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueRef {
    /// A global variable (an index into [`Module::globals`]).
    Global(usize),
    /// A function (an index into [`Module::functions`]).
    Function(usize),
    /// An alias or ifunc (an index into [`Module::aliases`]).
    Alias(usize),
    /// A module-level constant (an index into [`Module::constants`]).
    Constant(usize),
}

/// A constant's value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConstantKind {
    /// `null`, `zeroinitializer`.
    Null,
    /// `undef` (or a constant record this reader doesn't know, which LLVM
    /// reads as `undef`).
    Undef,
    /// `poison`.
    Poison,
    /// An integer (sign-extended).
    Integer(i64),
    /// A wide integer's 64-bit words (sign-rotated), low first.
    WideInteger(Vec<u64>),
    /// A byte-type value.
    Byte(i64),
    /// A wide byte-type value's words.
    WideByte(Vec<u64>),
    /// A floating-point value's bits, in the record's words.
    Float(Vec<u64>),
    /// A structure, array, or vector of other constants' value IDs.
    Aggregate(Vec<u32>),
    /// An array of characters (`c"..."`).
    String(Vec<u8>),
    /// An array of characters with a NUL added at the end.
    CString(Vec<u8>),
    /// An array or vector of integers or floats (`ConstantData`).
    Data(Vec<u64>),
    /// A unary operation (`fneg`).
    UnaryOp {
        /// `UNOP_*`.
        op: u32,
        /// The operand's value ID.
        operand: u32,
    },
    /// A binary operation.
    BinaryOp {
        /// `BINOP_*`.
        op: u32,
        /// The left operand's value ID.
        lhs: u32,
        /// The right one's.
        rhs: u32,
        /// Its flags (`nuw`, `nsw`, `exact`).
        flags: u64,
    },
    /// A cast.
    Cast {
        /// `CAST_*`.
        op: u32,
        /// The operand's type.
        from: u32,
        /// The operand's value ID.
        operand: u32,
    },
    /// A `getelementptr`.
    GetElementPtr {
        /// The source element type (none in old bitcode).
        source_type: Option<u32>,
        /// Its flags (`GEP_*` bits).
        flags: u64,
        /// The base and indices' value IDs.
        operands: Vec<u32>,
    },
    /// A `select` (condition, true value, false value).
    Select([u32; 3]),
    /// An `extractelement` (vector, index).
    ExtractElement([u32; 2]),
    /// An `insertelement` (vector, element, index).
    InsertElement([u32; 3]),
    /// A `shufflevector` (two vectors, the mask).
    ShuffleVector([u32; 3]),
    /// An `icmp` or `fcmp`.
    Compare {
        /// The predicate.
        predicate: u64,
        /// The left operand's value ID.
        lhs: u32,
        /// The right one's.
        rhs: u32,
    },
    /// Inline assembly.
    InlineAsm {
        /// Its function type (in the record since LLVM 13).
        function_type: Option<u32>,
        /// `sideeffect`, `alignstack`, dialect, `unwind` bits.
        flags: u64,
        /// The assembly.
        asm: Vec<u8>,
        /// The constraints.
        constraints: Vec<u8>,
    },
    /// A `blockaddress`.
    BlockAddress {
        /// The function's value ID.
        function: u32,
        /// The block's index.
        block: u64,
    },
    /// A `dso_local_equivalent`.
    DsoLocalEquivalent(u32),
    /// A `no_cfi`.
    NoCfi(u32),
    /// A `ptrauth` (pointer, key, discriminator, address discriminator,
    /// and the deactivation symbol if any).
    PtrAuth(Vec<u32>),
}

/// A constant: its type and value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Constant {
    /// Its type.
    pub ty: u32,
    /// Its value.
    pub kind: ConstantKind,
}

impl Constant {
    /// The value IDs it uses.
    pub fn operands(&self) -> Vec<u32> {
        use ConstantKind as K;
        match &self.kind {
            K::Aggregate(v) | K::GetElementPtr { operands: v, .. } | K::PtrAuth(v) => v.clone(),
            K::UnaryOp { operand, .. } | K::Cast { operand, .. } => alloc::vec![*operand],
            K::BinaryOp { lhs, rhs, .. } | K::Compare { lhs, rhs, .. } => alloc::vec![*lhs, *rhs],
            K::Select(v) | K::InsertElement(v) | K::ShuffleVector(v) => v.to_vec(),
            K::ExtractElement(v) => v.to_vec(),
            K::BlockAddress { function, .. } => alloc::vec![*function],
            K::DsoLocalEquivalent(v) | K::NoCfi(v) => alloc::vec![*v],
            _ => Vec::new(),
        }
    }
}

/// One attribute.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Attribute {
    /// An attribute with no value (`nounwind`): its kind (`ATTR_KIND_*`).
    Enum(u32),
    /// An attribute with an integer (`align 8`).
    Int(u32, u64),
    /// A string attribute (`"key"="value"`).
    String(Vec<u8>, Option<Vec<u8>>),
    /// A type attribute (`byval(T)`), with its type if given.
    Type(u32, Option<u32>),
    /// A constant-range attribute (`range(i32 0, 10)`): its kind, bit
    /// width, and the record's words for the range.
    Range(u32, u64, Vec<u64>),
    /// A constant-range-list attribute: its kind, bit width, and the
    /// ranges' words.
    RangeList(u32, u64, Vec<u64>),
}

impl Attribute {
    /// Its name as IR writes it (the key, for a string attribute).
    pub fn name(&self) -> Cow<'static, str> {
        let kind = match self {
            Self::String(k, _) => return Cow::Owned(String::from_utf8_lossy(k).into_owned()),
            Self::Enum(k)
            | Self::Int(k, _)
            | Self::Type(k, _)
            | Self::Range(k, ..)
            | Self::RangeList(k, ..) => *k,
        };
        attribute_name(kind).map_or_else(
            || Cow::Owned(alloc::format!("<attribute {kind}>")),
            Cow::Borrowed,
        )
    }
}

/// An attribute group: attributes of one place (a function, its return
/// value, or a parameter).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributeGroup {
    /// Its ID.
    pub id: u64,
    /// Where it applies: `0xFFFFFFFF` the function, 0 the return value,
    /// `n` parameter `n - 1`.
    pub index: u64,
    /// The attributes.
    pub attributes: Vec<Attribute>,
}

/// An LLVM IR module.
#[derive(Clone, Debug, Default)]
pub struct Module<'d> {
    /// Its producer, from the identification block before it.
    pub identification: Option<Identification>,
    /// Where its block is in the stream, in bits (its `ENTER_SUBBLOCK`) and
    /// its length.
    pub bit_offset: u64,
    /// Its size, in bits.
    pub bits: u64,
    /// The record format's version (0: absolute value IDs; 1: relative;
    /// 2: names in the string table).
    pub version: u64,
    /// The target triple.
    pub triple: Vec<u8>,
    /// The data layout.
    pub datalayout: Vec<u8>,
    /// The source file's name.
    pub source_filename: Vec<u8>,
    /// Module-level inline assembly.
    pub asm: Vec<u8>,
    /// Section names (globals' `section` indexes them).
    pub section_names: Vec<Vec<u8>>,
    /// Garbage collector names.
    pub gc_names: Vec<Vec<u8>>,
    /// Dependent libraries (old bitcode's `deplibs`).
    pub dependent_libraries: Vec<Vec<u8>>,
    /// The module hash (`MODULE_CODE_HASH`), if written.
    pub hash: Option<[u32; 5]>,
    /// The types.
    pub types: TypeTable,
    /// Attribute groups, by ID.
    pub attribute_groups: Vec<AttributeGroup>,
    /// Attribute lists (`PARAMATTR_CODE_ENTRY`): each a list of group IDs.
    pub attribute_lists: Vec<Vec<u64>>,
    /// Comdats.
    pub comdats: Vec<Comdat<'d>>,
    /// Global variables.
    pub globals: Vec<GlobalVar<'d>>,
    /// Functions.
    pub functions: Vec<Function<'d>>,
    /// Aliases and ifuncs.
    pub aliases: Vec<Alias<'d>>,
    /// Module-level constants.
    pub constants: Vec<Constant>,
    /// What each module-level value ID names.
    pub values: Vec<ValueRef>,
    /// Each module-level value's type.
    pub value_types: Vec<u32>,
    /// Module-level metadata.
    pub metadata: Metadata<'d>,
    /// Operand bundle tags.
    pub operand_bundle_tags: Vec<Vec<u8>>,
    /// Sync scope names.
    pub sync_scope_names: Vec<Vec<u8>>,
    /// Function bodies.
    pub bodies: Vec<FunctionBody<'d>>,
    /// Whether it has a summary (ThinLTO's, or full LTO's): the block ID.
    pub summary: Option<u32>,
    /// What went wrong reading it, where reading went on past it.
    pub errors: Vec<Error>,
}

impl<'d> Module<'d> {
    /// What value ID `v` names.
    pub fn value(&self, v: u32) -> Option<ValueRef> {
        self.values.get(v as usize).copied()
    }

    /// The name of the global value ID `v` names (none for a constant).
    pub fn value_name(&self, v: u32) -> Option<&[u8]> {
        Some(match self.value(v)? {
            ValueRef::Global(i) => &self.globals.get(i)?.name,
            ValueRef::Function(i) => &self.functions.get(i)?.name,
            ValueRef::Alias(i) => &self.aliases.get(i)?.name,
            ValueRef::Constant(_) => return None,
        })
    }

    /// The name LLVM's assembly gives global value `v` (`@name`, or `@N`
    /// for an unnamed one: numbered through the global variables, then
    /// the aliases, the ifuncs, and the functions, as llvm-dis numbers
    /// them).
    pub fn ir_name(&self, v: u32) -> Option<String> {
        let name = self.value_name(v)?;
        if !name.is_empty() {
            return Some(crate::types::llvm_name('@', name));
        }
        let unnamed_globals = self
            .globals
            .iter()
            .filter(|g| g.name.is_empty())
            .map(|g| g.value);
        let unnamed_aliases = self
            .aliases
            .iter()
            .filter(|a| a.kind == AliasKind::Alias && a.name.is_empty())
            .map(|a| a.value);
        let unnamed_ifuncs = self
            .aliases
            .iter()
            .filter(|a| a.kind == AliasKind::IFunc && a.name.is_empty())
            .map(|a| a.value);
        let unnamed_functions = self
            .functions
            .iter()
            .filter(|f| f.name.is_empty())
            .map(|f| f.value);
        let n = unnamed_globals
            .chain(unnamed_aliases)
            .chain(unnamed_ifuncs)
            .chain(unnamed_functions)
            .position(|u| u == v)?;
        Some(alloc::format!("@{n}"))
    }

    /// The constant value ID `v` names.
    pub fn constant(&self, v: u32) -> Option<&Constant> {
        match self.value(v)? {
            ValueRef::Constant(i) => self.constants.get(i),
            _ => None,
        }
    }

    /// The attribute groups of attribute list `list` (as functions and
    /// calls give it: 0 for none).
    pub fn attribute_list(&self, list: u64) -> Vec<&AttributeGroup> {
        let Some(groups) = list
            .checked_sub(1)
            .and_then(|i| self.attribute_lists.get(idx(i)))
        else {
            return Vec::new();
        };
        groups
            .iter()
            .filter_map(|g| self.attribute_groups.iter().find(|a| a.id == *g))
            .collect()
    }

    /// The function a body is of.
    pub fn body_function(&self, body: &FunctionBody<'_>) -> Option<&Function<'d>> {
        self.functions.get(body.function?)
    }

    /// The defined functions with their bodies.
    pub fn defined(&self) -> impl Iterator<Item = (&Function<'d>, &FunctionBody<'d>)> {
        self.bodies
            .iter()
            .filter_map(|b| Some((self.functions.get(b.function?)?, b)))
    }

    /// The program address space the data layout gives (`P<n>`), or 0.
    pub fn program_address_space(&self) -> u32 {
        layout_address_space(&self.datalayout, b'P')
    }
}

fn layout_address_space(layout: &[u8], key: u8) -> u32 {
    layout
        .split(|&c| c == b'-')
        .find_map(|spec| {
            let rest = spec.strip_prefix(&[key])?;
            core::str::from_utf8(rest).ok()?.parse().ok()
        })
        .unwrap_or(0)
}

/// A bitcode file: its modules, and the symbol table (`irsymtab`) LTO links
/// with, if it has one.
#[derive(Clone, Debug)]
pub struct Bitcode<'d> {
    /// The input.
    pub data: &'d [u8],
    /// The bitstream (after the wrapper, if any).
    pub stream: &'d [u8],
    /// Where the stream starts in the input.
    pub stream_offset: u64,
    /// The wrapper, if it had one.
    pub wrapper: Option<Wrapper>,
    /// The top-level blocks.
    pub blocks: Vec<TopBlock>,
    /// The modules.
    pub modules: Vec<Module<'d>>,
    /// The symbol table (the first `SYMTAB` block), with the string table
    /// it refers to (the first `STRTAB`).
    pub symtab: Option<Symtab<'d>>,
}

fn malformed(bit: u64, what: &'static str) -> Error {
    Error::new(ErrorKind::Malformed, bit / 8, what)
}

/// The blob of a top-level block holding one blob record (`STRTAB`,
/// `SYMTAB`).
fn top_blob<'d>(data: &'d [u8], b: &TopBlock) -> Result<&'d [u8]> {
    let mut c = Cursor::new(data, b.header)?;
    c.enter_block(b.id)?;
    let mut ops = Vec::new();
    let mut blob = None;
    loop {
        match c.advance_skipping_blocks()? {
            Entry::EndBlock => break,
            Entry::SubBlock(_) => {}
            Entry::Record(id) => {
                ops.clear();
                let head = c.read_record(id, &mut ops)?;
                if head.code == STRTAB_BLOB && blob.is_none() {
                    blob = head.blob;
                }
            }
        }
    }
    blob.ok_or(malformed(b.start, "string or symbol table blob"))
}

fn read_identification(data: &[u8], b: &TopBlock) -> Result<Identification> {
    let mut c = Cursor::new(data, b.header)?;
    c.enter_block(b.id)?;
    let mut id = Identification::default();
    let mut ops = Vec::new();
    loop {
        match c.advance_skipping_blocks()? {
            Entry::EndBlock => return Ok(id),
            Entry::SubBlock(_) => {}
            Entry::Record(a) => {
                ops.clear();
                let head = c.read_record(a, &mut ops)?;
                match head.code {
                    IDENTIFICATION_CODE_STRING => id.producer = chars(&ops),
                    IDENTIFICATION_CODE_EPOCH => id.epoch = ops.first().copied(),
                    _ => {}
                }
            }
        }
    }
}

impl<'d> Bitcode<'d> {
    /// Reads a bitcode file (bare or wrapped): every module, with every
    /// function body (on every core).
    pub fn parse(data: &'d [u8]) -> Result<Self> {
        let stream = Stream::new(data)?;
        if stream.kind != StreamKind::LlvmIr {
            return Err(Error::new(
                ErrorKind::BadMagic,
                stream.offset,
                "bitcode magic",
            ));
        }
        let blocks = stream.top_blocks();
        if blocks.is_empty() {
            return Err(malformed(32, "bitcode (no blocks)"));
        }
        let bytes = stream.data;
        // Each module's string table is the first after it.
        let strtabs: Vec<(u64, &'d [u8])> = blocks
            .iter()
            .filter(|b| b.id == STRTAB_BLOCK_ID)
            .filter_map(|b| Some((b.start, top_blob(bytes, b).ok()?)))
            .collect();
        let symtab = blocks
            .iter()
            .find(|b| b.id == SYMTAB_BLOCK_ID)
            .and_then(|b| top_blob(bytes, b).ok())
            .map(|blob| Symtab::new(blob, strtabs.first().map_or(&[][..], |s| s.1)));
        let mut modules = Vec::new();
        let mut ident: Option<(Identification, u64)> = None;
        for b in &blocks {
            match b.id {
                IDENTIFICATION_BLOCK_ID => {
                    ident = read_identification(bytes, b).ok().map(|i| (i, b.start));
                }
                MODULE_BLOCK_ID => {
                    let strtab = strtabs
                        .iter()
                        .find(|(at, _)| *at > b.start)
                        .map_or(&[][..], |s| s.1);
                    let (identification, origin) = match ident.take() {
                        Some((i, at)) => (Some(i), at),
                        None => (None, b.start),
                    };
                    let mut m = read_module(bytes, b, strtab, origin)?;
                    m.identification = identification;
                    modules.push(m);
                }
                _ => {}
            }
        }
        Ok(Self {
            data,
            stream: bytes,
            stream_offset: stream.offset,
            wrapper: stream.wrapper,
            blocks,
            modules,
            symtab,
        })
    }
}

/// What reading a module's block collects besides the module.
struct Pending {
    /// Function blocks: where each starts (its `ENTER_SUBBLOCK`) and its
    /// header, in bits.
    bodies: Vec<(u64, u64)>,
    /// Function value IDs and their blocks' word offsets (`VST_CODE_FNENTRY`).
    fn_offsets: Vec<(u32, u64)>,
    /// Names from the value symbol table (old bitcode): value ID, name.
    names: Vec<(u32, Vec<u8>)>,
    /// The module block's abbreviation width when the bodies were seen.
    width: u32,
    info: Arc<BlockInfo>,
}

struct ModuleReader<'a, 'd> {
    data: &'d [u8],
    strtab: &'d [u8],
    m: &'a mut Module<'d>,
    pending: Pending,
    /// Whether types are typed pointers (old bitcode).
    typed_pointers: bool,
}

fn read_module<'d>(
    data: &'d [u8],
    b: &TopBlock,
    strtab: &'d [u8],
    origin: u64,
) -> Result<Module<'d>> {
    let mut m = Module {
        bit_offset: b.start,
        bits: b.end - b.start,
        ..Module::default()
    };
    let mut c = Cursor::new(data, b.header)?;
    c.enter_block(MODULE_BLOCK_ID)?;
    let mut r = ModuleReader {
        data,
        strtab,
        m: &mut m,
        pending: Pending {
            bodies: Vec::new(),
            fn_offsets: Vec::new(),
            names: Vec::new(),
            width: c.abbrev_width(),
            info: Arc::clone(c.block_info()),
        },
        typed_pointers: false,
    };
    r.read(&mut c)?;
    r.finish(origin);
    Ok(m)
}

/// Decodes a sign-rotated VBR value (`decodeSignRotatedValue`).
pub(crate) fn sign_rotated(v: u64) -> i64 {
    let mag = (v >> 1) as i64;
    if v & 1 == 0 {
        mag
    } else if v == 1 {
        i64::MIN
    } else {
        mag.wrapping_neg()
    }
}

impl<'d> ModuleReader<'_, 'd> {
    fn read(&mut self, c: &mut Cursor<'d>) -> Result<()> {
        let mut ops = Vec::new();
        loop {
            let before = c.position();
            match c.advance()? {
                Entry::EndBlock => return Ok(()),
                Entry::SubBlock(id) => self.sub_block(c, id, before)?,
                Entry::Record(a) => {
                    ops.clear();
                    let head = c.read_record(a, &mut ops)?;
                    self.record(head.code, &ops, before);
                }
            }
        }
    }

    /// Reads (or skips) a sub-block of the module. A known block that
    /// can't be read is skipped by its length and noted.
    fn sub_block(&mut self, c: &mut Cursor<'d>, id: u32, before: u64) -> Result<()> {
        if id == BLOCKINFO_BLOCK_ID {
            let info = Arc::new(c.read_block_info()?);
            c.set_block_info(info);
            return Ok(());
        }
        if id == FUNCTION_BLOCK_ID {
            self.pending.bodies.push((before, c.position()));
            self.pending.width = c.abbrev_width();
            self.pending.info = Arc::clone(c.block_info());
            return c.skip_block();
        }
        let known = matches!(
            id,
            TYPE_BLOCK_ID_NEW
                | PARAMATTR_BLOCK_ID
                | PARAMATTR_GROUP_BLOCK_ID
                | CONSTANTS_BLOCK_ID
                | METADATA_BLOCK_ID
                | METADATA_KIND_BLOCK_ID
                | VALUE_SYMTAB_BLOCK_ID
                | OPERAND_BUNDLE_TAGS_BLOCK_ID
                | SYNC_SCOPE_NAMES_BLOCK_ID
        );
        if id == GLOBALVAL_SUMMARY_BLOCK_ID || id == FULL_LTO_GLOBALVAL_SUMMARY_BLOCK_ID {
            self.m.summary = Some(id);
        }
        if !known {
            return c.skip_block();
        }
        let start = c.position();
        let mut sub = c.clone();
        match self.block(&mut sub, id) {
            Ok(()) => {
                *c = sub;
                Ok(())
            }
            Err(e) => {
                self.m.errors.push(e);
                c.seek(start)?;
                c.skip_block()
            }
        }
    }

    fn block(&mut self, c: &mut Cursor<'d>, id: u32) -> Result<()> {
        c.enter_block(id)?;
        match id {
            TYPE_BLOCK_ID_NEW => self.types(c),
            PARAMATTR_BLOCK_ID => self.attribute_lists(c),
            PARAMATTR_GROUP_BLOCK_ID => self.attribute_groups(c),
            CONSTANTS_BLOCK_ID => {
                let m = &mut *self.m;
                let i32_type = m.types.intern(Type::Integer(32));
                read_constants(c, i32_type, &mut m.value_types, &mut |k| {
                    m.values.push(ValueRef::Constant(m.constants.len()));
                    m.constants.push(k);
                })
            }
            METADATA_BLOCK_ID => self.m.metadata.read_block(c),
            METADATA_KIND_BLOCK_ID => self.m.metadata.read_kinds(c),
            VALUE_SYMTAB_BLOCK_ID => self.value_symtab(c),
            OPERAND_BUNDLE_TAGS_BLOCK_ID | SYNC_SCOPE_NAMES_BLOCK_ID => {
                let mut names = Vec::new();
                let mut ops = Vec::new();
                loop {
                    match c.advance_skipping_blocks()? {
                        Entry::EndBlock => break,
                        Entry::SubBlock(_) => {}
                        Entry::Record(a) => {
                            ops.clear();
                            c.read_record(a, &mut ops)?;
                            names.push(chars(&ops));
                        }
                    }
                }
                if id == OPERAND_BUNDLE_TAGS_BLOCK_ID {
                    self.m.operand_bundle_tags = names;
                } else {
                    self.m.sync_scope_names = names;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn types(&mut self, c: &mut Cursor<'d>) -> Result<()> {
        let mut ops = Vec::new();
        let mut name = None;
        loop {
            match c.advance_skipping_blocks()? {
                Entry::EndBlock => break,
                Entry::SubBlock(_) => {}
                Entry::Record(a) => {
                    ops.clear();
                    let head = c.read_record(a, &mut ops)?;
                    self.m.types.record(head.code, &ops, &mut name);
                }
            }
        }
        self.typed_pointers = self.m.types.types.iter().any(|t| {
            matches!(
                t,
                Type::Pointer {
                    pointee: Some(_),
                    ..
                }
            )
        });
        Ok(())
    }

    fn attribute_lists(&mut self, c: &mut Cursor<'d>) -> Result<()> {
        let mut ops = Vec::new();
        loop {
            match c.advance_skipping_blocks()? {
                Entry::EndBlock => return Ok(()),
                Entry::SubBlock(_) => {}
                Entry::Record(a) => {
                    ops.clear();
                    let head = c.read_record(a, &mut ops)?;
                    if matches!(head.code, PARAMATTR_CODE_ENTRY | PARAMATTR_CODE_ENTRY_OLD) {
                        self.m.attribute_lists.push(ops.clone());
                    }
                }
            }
        }
    }

    fn attribute_groups(&mut self, c: &mut Cursor<'d>) -> Result<()> {
        let mut ops = Vec::new();
        loop {
            match c.advance_skipping_blocks()? {
                Entry::EndBlock => return Ok(()),
                Entry::SubBlock(_) => {}
                Entry::Record(a) => {
                    ops.clear();
                    let head = c.read_record(a, &mut ops)?;
                    if head.code == PARAMATTR_GRP_CODE_ENTRY && ops.len() >= 2 {
                        self.m.attribute_groups.push(AttributeGroup {
                            id: ops[0],
                            index: ops[1],
                            attributes: attributes(&ops[2..]),
                        });
                    }
                }
            }
        }
    }

    fn value_symtab(&mut self, c: &mut Cursor<'d>) -> Result<()> {
        let mut ops = Vec::new();
        loop {
            match c.advance_skipping_blocks()? {
                Entry::EndBlock => return Ok(()),
                Entry::SubBlock(_) => {}
                Entry::Record(a) => {
                    ops.clear();
                    let head = c.read_record(a, &mut ops)?;
                    match head.code {
                        VST_CODE_ENTRY if !ops.is_empty() => {
                            self.pending.names.push((low32(ops[0]), chars(&ops[1..])));
                        }
                        VST_CODE_FNENTRY if ops.len() >= 2 => {
                            self.pending.fn_offsets.push((low32(ops[0]), ops[1]));
                            if ops.len() > 2 {
                                self.pending.names.push((low32(ops[0]), chars(&ops[2..])));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// A name from the string table (`[offset, size, ...]`), and the rest
    /// of the record; or, before version 2, no name and the record.
    fn strtab_name<'o>(&self, ops: &'o [u64]) -> (Name<'d>, &'o [u64]) {
        if self.m.version < 2 {
            return (Cow::Borrowed(&[][..]), ops);
        }
        let [off, size, ..] = *ops else {
            return (Cow::Borrowed(&[][..]), &[]);
        };
        let len = self.strtab.len() as u64;
        if off > len || size > len - off {
            return (Cow::Borrowed(&[][..]), &[]);
        }
        (
            Cow::Borrowed(&self.strtab[idx(off)..idx(off + size)]),
            &ops[2..],
        )
    }

    fn strtab_slice(&self, off: u64, size: u64) -> Option<Name<'d>> {
        let len = self.strtab.len() as u64;
        (off <= len && size <= len - off)
            .then(|| Cow::Borrowed(&self.strtab[idx(off)..idx(off + size)]))
    }

    fn section(&self, v: u64) -> Option<Name<'d>> {
        let i = v.checked_sub(1)?;
        self.m
            .section_names
            .get(idx(i))
            .map(|s| Cow::Owned(s.clone()))
    }

    fn pointer_to(&mut self, pointee: u32, address_space: u32) -> u32 {
        let t = Type::Pointer {
            pointee: self.typed_pointers.then_some(pointee),
            address_space,
        };
        self.m.types.intern(t)
    }

    fn record(&mut self, code: u32, ops: &[u64], at: u64) {
        let op = |i: usize| ops.get(i).copied().unwrap_or(0);
        match code {
            MODULE_CODE_VERSION => self.m.version = op(0),
            MODULE_CODE_TRIPLE => self.m.triple = chars(ops),
            MODULE_CODE_DATALAYOUT => self.m.datalayout = chars(ops),
            MODULE_CODE_ASM => {
                if !self.m.asm.is_empty() {
                    self.m.asm.push(b'\n');
                }
                self.m.asm.extend(chars(ops));
            }
            MODULE_CODE_SECTIONNAME => self.m.section_names.push(chars(ops)),
            MODULE_CODE_DEPLIB => self.m.dependent_libraries.push(chars(ops)),
            MODULE_CODE_GCNAME => self.m.gc_names.push(chars(ops)),
            MODULE_CODE_SOURCE_FILENAME => self.m.source_filename = chars(ops),
            MODULE_CODE_HASH if ops.len() == 5 => {
                self.m.hash = Some([
                    low32(ops[0]),
                    low32(ops[1]),
                    low32(ops[2]),
                    low32(ops[3]),
                    low32(ops[4]),
                ]);
            }
            MODULE_CODE_COMDAT => self.comdat(ops),
            MODULE_CODE_GLOBALVAR => self.global_var(ops, at),
            MODULE_CODE_FUNCTION => self.function(ops, at),
            MODULE_CODE_ALIAS | MODULE_CODE_ALIAS_OLD | MODULE_CODE_IFUNC => {
                self.alias(code, ops, at);
            }
            _ => {}
        }
    }

    fn comdat(&mut self, ops: &[u64]) {
        let (name, rest) = self.strtab_name(ops);
        let Some(&selection) = rest.first() else {
            return;
        };
        let name = if self.m.version < 2 {
            // v1: [selection_kind, name size, name chars...]
            let n = idx(rest.get(1).copied().unwrap_or(0)).min(rest.len().saturating_sub(2));
            Cow::Owned(chars(&rest[2.min(rest.len())..2 + n]))
        } else {
            name
        };
        self.m.comdats.push(Comdat {
            name,
            selection: low32(selection),
        });
    }

    fn comdat_index(&self, v: u64) -> Option<usize> {
        v.checked_sub(1)
            .map(idx)
            .filter(|&i| i < self.m.comdats.len())
    }

    fn global_var(&mut self, ops: &[u64], at: u64) {
        let (name, r) = self.strtab_name(ops);
        if r.len() < 6 {
            self.m.errors.push(malformed(at, "global variable record"));
            return;
        }
        let op = |i: usize| r.get(i).copied().unwrap_or(0);
        let mut ty = low32(r[0]);
        let explicit = r[1] & 2 != 0;
        let (value_type, address_space, ptr) = if explicit {
            let address_space = low32(r[1] >> 2);
            (ty, address_space, self.pointer_to(ty, address_space))
        } else {
            let address_space = self.m.types.address_space(ty).unwrap_or(0);
            let ptr = ty;
            ty = self.m.types.contained(ty, 0).unwrap_or(ty);
            (ty, address_space, ptr)
        };
        let linkage = Linkage::from_code(r[3]);
        let value = low32(self.m.values.len() as u64);
        let g = GlobalVar {
            name,
            value,
            value_type,
            address_space,
            constant: r[1] & 1 != 0,
            initializer: r[2].checked_sub(1).map(low32),
            linkage,
            alignment: alignment(r[4]),
            section: self.section(r[5]),
            visibility: if linkage.is_local() {
                Visibility::Default
            } else {
                Visibility::from_code(op(6))
            },
            thread_local: ThreadLocal::from_code(op(7)),
            unnamed_addr: UnnamedAddr::from_code(op(8)),
            externally_initialized: op(9) != 0,
            dll_storage: if linkage.is_local() {
                DllStorage::Default
            } else {
                DllStorage::from_code(op(10))
            },
            comdat: self.comdat_index(op(11)),
            attributes: op(12),
            dso_local: op(13) != 0,
            partition: if r.len() > 15 {
                self.strtab_slice(r[14], r[15])
            } else {
                None
            },
            sanitizer: op(16),
            code_model: op(17),
        };
        self.m.values.push(ValueRef::Global(self.m.globals.len()));
        self.m.value_types.push(ptr);
        self.m.globals.push(g);
    }

    fn function(&mut self, ops: &[u64], at: u64) {
        let (name, r) = self.strtab_name(ops);
        if r.len() < 8 {
            self.m.errors.push(malformed(at, "function record"));
            return;
        }
        let op = |i: usize| r.get(i).copied().unwrap_or(0);
        let mut fty = low32(r[0]);
        if let Some(Type::Pointer {
            pointee: Some(p), ..
        }) = self.m.types.get(fty)
        {
            fty = *p;
        }
        let address_space = if r.len() > 16 {
            low32(r[16])
        } else {
            self.m.program_address_space()
        };
        let linkage = Linkage::from_code(r[3]);
        let value = low32(self.m.values.len() as u64);
        let ptr = self.pointer_to(fty, address_space);
        let f = Function {
            name,
            value,
            function_type: fty,
            calling_conv: low32(r[1]),
            is_declaration: r[2] != 0,
            linkage,
            attributes: r[4],
            alignment: alignment(r[5]),
            section: self.section(r[6]),
            visibility: if linkage.is_local() {
                Visibility::Default
            } else {
                Visibility::from_code(r[7])
            },
            gc: op(8)
                .checked_sub(1)
                .and_then(|i| self.m.gc_names.get(idx(i)))
                .map(|s| Cow::Owned(s.clone())),
            unnamed_addr: UnnamedAddr::from_code(op(9)),
            prologue: op(10).checked_sub(1).map(low32),
            dll_storage: if linkage.is_local() {
                DllStorage::Default
            } else {
                DllStorage::from_code(op(11))
            },
            comdat: self.comdat_index(op(12)),
            prefix: op(13).checked_sub(1).map(low32),
            personality: op(14).checked_sub(1).map(low32),
            dso_local: op(15) != 0,
            address_space,
            partition: if r.len() > 18 {
                self.strtab_slice(r[17], r[18])
            } else {
                None
            },
            preferred_alignment: if r.len() > 19 { alignment(r[19]) } else { None },
            body: None,
        };
        self.m
            .values
            .push(ValueRef::Function(self.m.functions.len()));
        self.m.value_types.push(ptr);
        self.m.functions.push(f);
    }

    fn alias(&mut self, code: u32, ops: &[u64], at: u64) {
        let (name, r) = self.strtab_name(ops);
        let new = code != MODULE_CODE_ALIAS_OLD;
        if r.len() < 3 + usize::from(new) {
            self.m.errors.push(malformed(at, "alias record"));
            return;
        }
        let mut i = 0;
        let mut next = || {
            let v = r.get(i).copied();
            i += 1;
            v
        };
        let mut ty = low32(next().unwrap_or(0));
        let (address_space, ptr) = if new {
            let address_space = low32(next().unwrap_or(0));
            (address_space, None)
        } else {
            let address_space = self.m.types.address_space(ty).unwrap_or(0);
            let ptr = ty;
            ty = self.m.types.contained(ty, 0).unwrap_or(ty);
            (address_space, Some(ptr))
        };
        let target = low32(next().unwrap_or(0));
        let linkage = Linkage::from_code(next().unwrap_or(0));
        let mut a = Alias {
            kind: if code == MODULE_CODE_IFUNC {
                AliasKind::IFunc
            } else {
                AliasKind::Alias
            },
            name,
            value: low32(self.m.values.len() as u64),
            value_type: ty,
            address_space,
            target,
            linkage,
            visibility: Visibility::Default,
            dll_storage: DllStorage::Default,
            thread_local: ThreadLocal::None,
            unnamed_addr: UnnamedAddr::None,
            dso_local: false,
            partition: None,
        };
        if let Some(v) = next()
            && !linkage.is_local()
        {
            a.visibility = Visibility::from_code(v);
        }
        if a.kind == AliasKind::Alias {
            if let Some(v) = next()
                && !linkage.is_local()
            {
                a.dll_storage = DllStorage::from_code(v);
            }
            if let Some(v) = next() {
                a.thread_local = ThreadLocal::from_code(v);
            }
            if let Some(v) = next() {
                a.unnamed_addr = UnnamedAddr::from_code(v);
            }
        }
        if let Some(v) = next() {
            a.dso_local = v != 0;
        }
        if let (Some(off), Some(size)) = (next(), next()) {
            a.partition = self.strtab_slice(off, size);
        }
        let ptr = ptr.unwrap_or_else(|| self.pointer_to(ty, address_space));
        self.m.values.push(ValueRef::Alias(self.m.aliases.len()));
        self.m.value_types.push(ptr);
        self.m.aliases.push(a);
    }

    /// After the block: old bitcode's names, and the function bodies.
    fn finish(&mut self, origin: u64) {
        let names = core::mem::take(&mut self.pending.names);
        for (v, name) in names {
            let name = Cow::Owned(name);
            match self.m.value(v) {
                Some(ValueRef::Global(i)) => self.m.globals[i].name = name,
                Some(ValueRef::Function(i)) => self.m.functions[i].name = name,
                Some(ValueRef::Alias(i)) => self.m.aliases[i].name = name,
                _ => {}
            }
        }
        // Bodies go to the defined functions in order, unless the value
        // symbol table's offsets say otherwise (and agree with the blocks).
        let defined: Vec<usize> = (0..self.m.functions.len())
            .filter(|&i| !self.m.functions[i].is_declaration)
            .collect();
        let mut owner: Vec<Option<usize>> = (0..self.pending.bodies.len())
            .map(|i| defined.get(i).copied())
            .collect();
        if !self.pending.fn_offsets.is_empty() {
            let mut by_offset = alloc::vec![None; self.pending.bodies.len()];
            let mut all = true;
            for &(v, off) in &self.pending.fn_offsets {
                let Some(ValueRef::Function(f)) = self.m.value(v) else {
                    continue;
                };
                let bit = origin.wrapping_add(off.wrapping_sub(1).wrapping_mul(32));
                match self.pending.bodies.iter().position(|b| b.0 == bit) {
                    Some(i) => by_offset[i] = Some(f),
                    None => all = false,
                }
            }
            if all && by_offset.iter().all(Option::is_some) {
                owner = by_offset;
            }
        }
        let ctx = function::Context::new(self.m);
        let jobs: Vec<(usize, Option<usize>, u64, u64)> = self
            .pending
            .bodies
            .iter()
            .zip(&owner)
            .enumerate()
            .map(|(i, (&(start, header), &f))| (i, f, start, header))
            .collect();
        let data = self.data;
        let info = Arc::clone(&self.pending.info);
        let bodies = turbonop_par::map(&jobs, 8, |&(_, f, start, header)| {
            function::read_body(&ctx, data, &info, f, start, header)
        });
        for (i, b) in bodies.into_iter().enumerate() {
            if let Some(f) = b.function_index()
                && let Some(func) = self.m.functions.get_mut(f)
            {
                func.body = Some(i);
            }
            self.m.bodies.push(b);
        }
    }
}

/// Reads the attributes of a group record (after its ID and index).
fn attributes(ops: &[u64]) -> Vec<Attribute> {
    let mut out = Vec::new();
    let mut i = 0;
    let get = |i: usize| ops.get(i).copied();
    while i < ops.len() {
        let kind = ops[i];
        i += 1;
        match kind {
            0 => {
                let Some(k) = get(i) else { break };
                out.push(Attribute::Enum(low32(k)));
                i += 1;
            }
            1 => {
                let (Some(k), Some(v)) = (get(i), get(i + 1)) else {
                    break;
                };
                out.push(Attribute::Int(low32(k), v));
                i += 2;
            }
            3 | 4 => {
                let end = ops[i..]
                    .iter()
                    .position(|&c| c == 0)
                    .map_or(ops.len(), |p| i + p);
                let key = chars(&ops[i..end]);
                i = end + 1;
                let value = if kind == 4 {
                    let end = ops
                        .get(i..)
                        .and_then(|r| r.iter().position(|&c| c == 0))
                        .map_or(ops.len(), |p| i + p);
                    let v = chars(ops.get(i..end).unwrap_or(&[]));
                    i = end + 1;
                    Some(v)
                } else {
                    None
                };
                out.push(Attribute::String(key, value));
            }
            5 | 6 => {
                let Some(k) = get(i) else { break };
                i += 1;
                let ty = if kind == 6 {
                    let t = get(i).map(low32);
                    i += 1;
                    t
                } else {
                    None
                };
                out.push(Attribute::Type(low32(k), ty));
            }
            7 => {
                let (Some(k), Some(width)) = (get(i), get(i + 1)) else {
                    break;
                };
                i += 2;
                let n = range_words(ops, i, width);
                let words = ops.get(i..i + n).unwrap_or(&[]).to_vec();
                i += n;
                out.push(Attribute::Range(low32(k), width, words));
            }
            8 => {
                let (Some(k), Some(count), Some(width)) = (get(i), get(i + 1), get(i + 2)) else {
                    break;
                };
                i += 3;
                let start = i;
                for _ in 0..count.min(ops.len() as u64) {
                    i += range_words(ops, i, width);
                }
                out.push(Attribute::RangeList(
                    low32(k),
                    width,
                    ops.get(start..i).unwrap_or(&[]).to_vec(),
                ));
            }
            _ => break,
        }
    }
    out
}

/// How many record words a constant range of `width` bits at `i` takes.
fn range_words(ops: &[u64], i: usize, width: u64) -> usize {
    if width > 64 {
        let Some(&w) = ops.get(i) else { return 0 };
        let lower = idx(w & 0xFFFF_FFFF);
        let upper = idx(w >> 32);
        (1 + lower.saturating_add(upper)).min(ops.len() - i)
    } else {
        2.min(ops.len().saturating_sub(i))
    }
}

/// Reads a `CONSTANTS_BLOCK` (entered): each constant, with its type
/// pushed on `value_types`. Every record but `SETTYPE` makes a value (an
/// unknown one is `undef`, as LLVM reads it).
pub(crate) fn read_constants(
    c: &mut Cursor<'_>,
    i32_type: u32,
    value_types: &mut Vec<u32>,
    push: &mut dyn FnMut(Constant),
) -> Result<()> {
    use ConstantKind as K;
    let mut ops = Vec::new();
    // The current type starts as i32.
    let mut ty = i32_type;
    loop {
        let a = match c.advance_skipping_blocks()? {
            Entry::EndBlock => return Ok(()),
            Entry::SubBlock(_) => continue,
            Entry::Record(a) => a,
        };
        ops.clear();
        let head = c.read_record(a, &mut ops)?;
        let op = |i: usize| ops.get(i).copied().unwrap_or(0);
        let v = |i: usize| low32(op(i));
        let kind = match head.code {
            CST_CODE_SETTYPE => {
                ty = v(0);
                continue;
            }
            CST_CODE_NULL => K::Null,
            CST_CODE_POISON => K::Poison,
            CST_CODE_INTEGER => K::Integer(sign_rotated(op(0))),
            CST_CODE_WIDE_INTEGER => K::WideInteger(ops.clone()),
            CST_CODE_BYTE => K::Byte(sign_rotated(op(0))),
            CST_CODE_WIDE_BYTE => K::WideByte(ops.clone()),
            CST_CODE_FLOAT => K::Float(ops.clone()),
            CST_CODE_AGGREGATE => K::Aggregate(ops.iter().map(|&x| low32(x)).collect()),
            CST_CODE_STRING => K::String(chars(&ops)),
            CST_CODE_CSTRING => K::CString(chars(&ops)),
            CST_CODE_DATA => K::Data(ops.clone()),
            CST_CODE_CE_UNOP => K::UnaryOp {
                op: v(0),
                operand: v(1),
            },
            CST_CODE_CE_BINOP => K::BinaryOp {
                op: v(0),
                lhs: v(1),
                rhs: v(2),
                flags: op(3),
            },
            CST_CODE_CE_CAST => K::Cast {
                op: v(0),
                from: v(1),
                operand: v(2),
            },
            CST_CODE_CE_INBOUNDS_GEP
            | CST_CODE_CE_GEP_OLD
            | CST_CODE_CE_GEP_WITH_INRANGE_INDEX_OLD
            | CST_CODE_CE_GEP
            | CST_CODE_CE_GEP_WITH_INRANGE => {
                let code = head.code;
                let mut i = 0;
                let mut source_type = None;
                if code == CST_CODE_CE_GEP_WITH_INRANGE_INDEX_OLD
                    || code == CST_CODE_CE_GEP_WITH_INRANGE
                    || code == CST_CODE_CE_GEP
                    || ops.len() % 2 == 1
                {
                    source_type = Some(v(0));
                    i = 1;
                }
                let mut flags = 0;
                if code == CST_CODE_CE_GEP_WITH_INRANGE_INDEX_OLD {
                    flags = op(i) & 1;
                    i += 1;
                } else if code == CST_CODE_CE_GEP_WITH_INRANGE {
                    flags = op(i);
                    let width = op(i + 1);
                    i += 2;
                    i += range_words(&ops, i, width);
                } else if code == CST_CODE_CE_GEP {
                    flags = op(i);
                    i += 1;
                } else if code == CST_CODE_CE_INBOUNDS_GEP {
                    flags = 1 << GEP_INBOUNDS;
                }
                let operands = ops
                    .get(i..)
                    .unwrap_or(&[])
                    .chunks(2)
                    .filter(|p| p.len() == 2)
                    .map(|p| low32(p[1]))
                    .collect();
                K::GetElementPtr {
                    source_type,
                    flags,
                    operands,
                }
            }
            CST_CODE_CE_SELECT => K::Select([v(0), v(1), v(2)]),
            CST_CODE_CE_EXTRACTELT => {
                K::ExtractElement([v(1), if ops.len() == 4 { v(3) } else { v(2) }])
            }
            CST_CODE_CE_INSERTELT => {
                K::InsertElement([v(0), v(1), if ops.len() == 4 { v(3) } else { v(2) }])
            }
            CST_CODE_CE_SHUFFLEVEC => K::ShuffleVector([v(0), v(1), v(2)]),
            CST_CODE_CE_SHUFVEC_EX => K::ShuffleVector([v(1), v(2), v(3)]),
            CST_CODE_CE_CMP => K::Compare {
                predicate: op(3),
                lhs: v(1),
                rhs: v(2),
            },
            CST_CODE_INLINEASM_OLD
            | CST_CODE_INLINEASM_OLD2
            | CST_CODE_INLINEASM_OLD3
            | CST_CODE_INLINEASM => {
                let (function_type, rest) = if head.code == CST_CODE_INLINEASM {
                    (Some(v(0)), ops.get(1..).unwrap_or(&[]))
                } else {
                    (None, &ops[..])
                };
                let flags = rest.first().copied().unwrap_or(0);
                let n = idx(rest.get(1).copied().unwrap_or(0)).min(rest.len().saturating_sub(2));
                let asm = chars(rest.get(2..2 + n).unwrap_or(&[]));
                let m = idx(rest.get(2 + n).copied().unwrap_or(0))
                    .min(rest.len().saturating_sub(3 + n));
                let constraints = chars(rest.get(3 + n..3 + n + m).unwrap_or(&[]));
                K::InlineAsm {
                    function_type,
                    flags,
                    asm,
                    constraints,
                }
            }
            CST_CODE_BLOCKADDRESS => K::BlockAddress {
                function: v(1),
                block: op(2),
            },
            CST_CODE_DSO_LOCAL_EQUIVALENT => K::DsoLocalEquivalent(v(1)),
            CST_CODE_NO_CFI_VALUE => K::NoCfi(v(1)),
            CST_CODE_PTRAUTH | CST_CODE_PTRAUTH2 => {
                K::PtrAuth(ops.iter().map(|&x| low32(x)).collect())
            }
            _ => K::Undef,
        };
        value_types.push(ty);
        push(Constant { ty, kind });
    }
}

/// A map from types to their IDs, for interning on many threads.
pub(crate) fn type_index(types: &TypeTable) -> HashMap<Type, u32> {
    let mut m = HashMap::with_capacity(types.len());
    for (i, t) in types.types.iter().enumerate() {
        m.entry(t.clone()).or_insert(low32(i as u64));
    }
    m
}

//! The metadata tables (II.22, II.24.2.6): the `#~` (compressed) or `#-`
//! (uncompressed, with the `Ptr` tables of edit-and-continue) stream, every
//! table's schema, and typed rows.
//!
//! A column's width depends on the file: heap indexes are 2 or 4 bytes by
//! the stream's heap-size flags, a table index is 2 bytes while the table
//! has fewer than 2^16 rows, and a coded index (II.24.2.6) is 2 bytes while
//! every table it can point to has fewer than 2^(16 - tag bits) rows. The
//! portable PDB tables (0x30 to 0x37) count the type system's tables from
//! the `#Pdb` stream, which holds their row counts.
//!
//! Rows are numbered from 1 (a row id, *rid*); rid 0 is null.

use std::fmt;

use turbonop_core::{Error, ErrorKind, Result};

use crate::heaps::{BlobIndex, GuidIndex, StringIndex};
use crate::reader::Cursor;

/// The most columns a table has (Assembly and AssemblyRef have 9).
pub const MAX_COLUMNS: usize = 9;

/// A metadata token: a table (the high byte) and a row id (the low 24
/// bits). User strings (`0x70`) put a `#US` offset where the row id is.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Token(pub u32);

impl Token {
    /// The token of row `rid` of `table`.
    pub const fn new(table: TableId, rid: u32) -> Self {
        Self(((table as u32) << 24) | (rid & 0x00FF_FFFF))
    }

    /// The table byte.
    pub const fn table_byte(self) -> u8 {
        (self.0 >> 24) as u8
    }

    /// The table, if the high byte is one.
    pub const fn table(self) -> Option<TableId> {
        TableId::from_u8(self.table_byte())
    }

    /// The row id (or `#US` offset).
    pub const fn rid(self) -> u32 {
        self.0 & 0x00FF_FFFF
    }

    /// Whether the row id is 0.
    pub const fn is_null(self) -> bool {
        self.rid() == 0
    }

    /// Whether this is a user string's (`0x70`).
    pub const fn is_user_string(self) -> bool {
        self.table_byte() == 0x70
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010x}", self.0)
    }
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010x}", self.0)
    }
}

/// The kinds of coded index (II.24.2.6, and the portable PDB's
/// HasCustomDebugInformation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CodedIndexKind {
    /// TypeDef, TypeRef, TypeSpec.
    TypeDefOrRef,
    /// Field, Param, Property.
    HasConstant,
    /// Anything that can carry a custom attribute (22 tables).
    HasCustomAttribute,
    /// Field, Param.
    HasFieldMarshal,
    /// TypeDef, MethodDef, Assembly.
    HasDeclSecurity,
    /// TypeDef, TypeRef, ModuleRef, MethodDef, TypeSpec.
    MemberRefParent,
    /// Event, Property.
    HasSemantics,
    /// MethodDef, MemberRef.
    MethodDefOrRef,
    /// Field, MethodDef.
    MemberForwarded,
    /// File, AssemblyRef, ExportedType.
    Implementation,
    /// (unused), (unused), MethodDef, MemberRef, (unused).
    CustomAttributeType,
    /// Module, ModuleRef, AssemblyRef, TypeRef.
    ResolutionScope,
    /// TypeDef, MethodDef.
    TypeOrMethodDef,
    /// Anything that can carry custom debug information (27 tables).
    HasCustomDebugInformation,
}

use TableId as T;

const HAS_CUSTOM_ATTRIBUTE: &[Option<TableId>] = &[
    Some(T::MethodDef),
    Some(T::Field),
    Some(T::TypeRef),
    Some(T::TypeDef),
    Some(T::Param),
    Some(T::InterfaceImpl),
    Some(T::MemberRef),
    Some(T::Module),
    Some(T::DeclSecurity),
    Some(T::Property),
    Some(T::Event),
    Some(T::StandAloneSig),
    Some(T::ModuleRef),
    Some(T::TypeSpec),
    Some(T::Assembly),
    Some(T::AssemblyRef),
    Some(T::File),
    Some(T::ExportedType),
    Some(T::ManifestResource),
    Some(T::GenericParam),
    Some(T::GenericParamConstraint),
    Some(T::MethodSpec),
];

const HAS_CUSTOM_DEBUG_INFORMATION: &[Option<TableId>] = &[
    Some(T::MethodDef),
    Some(T::Field),
    Some(T::TypeRef),
    Some(T::TypeDef),
    Some(T::Param),
    Some(T::InterfaceImpl),
    Some(T::MemberRef),
    Some(T::Module),
    Some(T::DeclSecurity),
    Some(T::Property),
    Some(T::Event),
    Some(T::StandAloneSig),
    Some(T::ModuleRef),
    Some(T::TypeSpec),
    Some(T::Assembly),
    Some(T::AssemblyRef),
    Some(T::File),
    Some(T::ExportedType),
    Some(T::ManifestResource),
    Some(T::GenericParam),
    Some(T::GenericParamConstraint),
    Some(T::MethodSpec),
    Some(T::Document),
    Some(T::LocalScope),
    Some(T::LocalVariable),
    Some(T::LocalConstant),
    Some(T::ImportScope),
];

impl CodedIndexKind {
    /// Every kind.
    pub const ALL: [CodedIndexKind; 14] = [
        Self::TypeDefOrRef,
        Self::HasConstant,
        Self::HasCustomAttribute,
        Self::HasFieldMarshal,
        Self::HasDeclSecurity,
        Self::MemberRefParent,
        Self::HasSemantics,
        Self::MethodDefOrRef,
        Self::MemberForwarded,
        Self::Implementation,
        Self::CustomAttributeType,
        Self::ResolutionScope,
        Self::TypeOrMethodDef,
        Self::HasCustomDebugInformation,
    ];

    /// The tables its tags name, in tag order (`None` for an unused tag).
    pub fn tables(self) -> &'static [Option<TableId>] {
        match self {
            Self::TypeDefOrRef => &[Some(T::TypeDef), Some(T::TypeRef), Some(T::TypeSpec)],
            Self::HasConstant => &[Some(T::Field), Some(T::Param), Some(T::Property)],
            Self::HasCustomAttribute => HAS_CUSTOM_ATTRIBUTE,
            Self::HasFieldMarshal => &[Some(T::Field), Some(T::Param)],
            Self::HasDeclSecurity => &[Some(T::TypeDef), Some(T::MethodDef), Some(T::Assembly)],
            Self::MemberRefParent => &[
                Some(T::TypeDef),
                Some(T::TypeRef),
                Some(T::ModuleRef),
                Some(T::MethodDef),
                Some(T::TypeSpec),
            ],
            Self::HasSemantics => &[Some(T::Event), Some(T::Property)],
            Self::MethodDefOrRef => &[Some(T::MethodDef), Some(T::MemberRef)],
            Self::MemberForwarded => &[Some(T::Field), Some(T::MethodDef)],
            Self::Implementation => &[Some(T::File), Some(T::AssemblyRef), Some(T::ExportedType)],
            Self::CustomAttributeType => {
                &[None, None, Some(T::MethodDef), Some(T::MemberRef), None]
            }
            Self::ResolutionScope => &[
                Some(T::Module),
                Some(T::ModuleRef),
                Some(T::AssemblyRef),
                Some(T::TypeRef),
            ],
            Self::TypeOrMethodDef => &[Some(T::TypeDef), Some(T::MethodDef)],
            Self::HasCustomDebugInformation => HAS_CUSTOM_DEBUG_INFORMATION,
        }
    }

    /// How many low bits the tag takes.
    #[allow(clippy::cast_possible_truncation)]
    pub fn tag_bits(self) -> u32 {
        let n = self.tables().len() as u32;
        u32::BITS - (n - 1).leading_zeros()
    }

    /// The coded value for `token`, if this kind can point to its table.
    #[allow(clippy::cast_possible_truncation)]
    pub fn encode(self, token: Token) -> Option<u32> {
        let table = token.table()?;
        let tag = self.tables().iter().position(|&t| t == Some(table))? as u32;
        token
            .rid()
            .checked_shl(self.tag_bits())
            .filter(|v| v >> self.tag_bits() == token.rid())
            .map(|v| v | tag)
    }
}

/// A coded index's value, as it is in its column: decode it with
/// [`CodedIndex::token`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct CodedIndex {
    /// Its kind.
    pub kind: CodedIndexKind,
    /// The column's value.
    pub raw: u32,
}

impl CodedIndex {
    /// `raw`, of `kind`.
    pub const fn new(kind: CodedIndexKind, raw: u32) -> Self {
        Self { kind, raw }
    }

    /// The tag.
    pub fn tag(self) -> u32 {
        self.raw & ((1 << self.kind.tag_bits()) - 1)
    }

    /// The table the tag names, if any.
    pub fn table(self) -> Option<TableId> {
        self.kind
            .tables()
            .get(self.tag() as usize)
            .copied()
            .flatten()
    }

    /// The row id.
    pub fn rid(self) -> u32 {
        self.raw >> self.kind.tag_bits()
    }

    /// Whether the row id is 0 (whatever the tag).
    pub fn is_null(self) -> bool {
        self.rid() == 0
    }

    /// The token, if the tag names a table (a null row gives a null token
    /// of that table).
    pub fn token(self) -> Option<Token> {
        Some(Token::new(self.table()?, self.rid()))
    }

    /// The token, or the null token (0) if the tag names no table.
    pub fn token_or_null(self) -> Token {
        self.token().unwrap_or_default()
    }
}

/// What a column holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColumnKind {
    /// A byte and a byte of padding (Constant's Type).
    U8,
    /// A 2-byte constant.
    U16,
    /// A 4-byte constant.
    U32,
    /// A `#Strings` index.
    Str,
    /// A `#GUID` index.
    Guid,
    /// A `#Blob` index.
    Blob,
    /// A row of a table.
    Index(TableId),
    /// The first of a run of rows of a table (TypeDef's FieldList, ...),
    /// through its `Ptr` table if there is one.
    List(TableId),
    /// A coded index.
    Coded(CodedIndexKind),
}

/// A column of a table's schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnDef {
    /// Its name (ECMA-335's).
    pub name: &'static str,
    /// What it holds.
    pub kind: ColumnKind,
}

/// A table's typed row.
pub trait Row: Sized + Copy {
    /// Its table.
    const TABLE: TableId;
    /// The row from its columns' values, in schema order.
    fn from_columns(c: &[u32; MAX_COLUMNS]) -> Self;
}

macro_rules! column_type {
    (U8) => {
        u8
    };
    (U16) => {
        u16
    };
    (U32) => {
        u32
    };
    (Str) => {
        StringIndex
    };
    (Guid) => {
        GuidIndex
    };
    (Blob) => {
        BlobIndex
    };
    (Index) => {
        u32
    };
    (List) => {
        u32
    };
    (Coded) => {
        CodedIndex
    };
}

macro_rules! column_kind {
    (U8) => {
        ColumnKind::U8
    };
    (U16) => {
        ColumnKind::U16
    };
    (U32) => {
        ColumnKind::U32
    };
    (Str) => {
        ColumnKind::Str
    };
    (Guid) => {
        ColumnKind::Guid
    };
    (Blob) => {
        ColumnKind::Blob
    };
    (Index($t:ident)) => {
        ColumnKind::Index(TableId::$t)
    };
    (List($t:ident)) => {
        ColumnKind::List(TableId::$t)
    };
    (Coded($k:ident)) => {
        ColumnKind::Coded(CodedIndexKind::$k)
    };
}

macro_rules! column_value {
    (U8, $v:expr) => {
        $v as u8
    };
    (U16, $v:expr) => {
        $v as u16
    };
    (U32, $v:expr) => {
        $v
    };
    (Str, $v:expr) => {
        StringIndex($v)
    };
    (Guid, $v:expr) => {
        GuidIndex($v)
    };
    (Blob, $v:expr) => {
        BlobIndex($v)
    };
    (Index($t:ident), $v:expr) => {
        $v
    };
    (List($t:ident), $v:expr) => {
        $v
    };
    (Coded($k:ident), $v:expr) => {
        CodedIndex::new(CodedIndexKind::$k, $v)
    };
}

macro_rules! tables {
    ($(
        $(#[doc = $doc:literal])*
        $id:ident = $num:literal, $row:ident {
            $( $(#[doc = $fdoc:literal])* $field:ident : $kind:ident $(($arg:ident))? ),* $(,)?
        }
    )*) => {
        /// A metadata table.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[repr(u8)]
        pub enum TableId {
            $( $(#[doc = $doc])* $id = $num, )*
        }

        impl TableId {
            /// Every table, in id order.
            pub const ALL: &'static [TableId] = &[ $(TableId::$id),* ];

            /// The table numbered `n`.
            pub const fn from_u8(n: u8) -> Option<Self> {
                match n {
                    $( $num => Some(TableId::$id), )*
                    _ => None,
                }
            }

            /// Its name (ECMA-335's, or the portable PDB specification's).
            pub const fn name(self) -> &'static str {
                match self {
                    $( TableId::$id => stringify!($id), )*
                }
            }

            /// Its columns.
            pub const fn columns(self) -> &'static [ColumnDef] {
                match self {
                    $( TableId::$id => &[ $( ColumnDef {
                        name: stringify!($field),
                        kind: column_kind!($kind $(($arg))?),
                    } ),* ], )*
                }
            }
        }

        $(
            $(#[doc = $doc])*
            #[derive(Clone, Copy, Debug, PartialEq, Eq)]
            pub struct $row {
                $( $(#[doc = $fdoc])* #[doc = concat!("(The `", stringify!($field), "` column.)")] pub $field: column_type!($kind), )*
            }

            impl Row for $row {
                const TABLE: TableId = TableId::$id;
                #[allow(clippy::cast_possible_truncation, unused_assignments)]
                fn from_columns(c: &[u32; MAX_COLUMNS]) -> Self {
                    let mut i = 0usize;
                    $( let $field = column_value!($kind $(($arg))?, c[i]); i += 1; )*
                    Self { $($field),* }
                }
            }
        )*
    };
}

tables! {
    /// The module (one row).
    Module = 0x00, ModuleRow {
        /// Reserved (0), or the edit-and-continue generation.
        generation: U16,
        name: Str,
        /// The module version ID.
        mvid: Guid,
        enc_id: Guid,
        enc_base_id: Guid,
    }
    /// References to types of other modules and assemblies.
    TypeRef = 0x01, TypeRefRow {
        /// The module, module reference, assembly reference, or (for a
        /// nested type) type reference that holds it.
        resolution_scope: Coded(ResolutionScope),
        type_name: Str,
        type_namespace: Str,
    }
    /// The types this module defines (row 1 is `<Module>`).
    TypeDef = 0x02, TypeDefRow {
        /// `TypeAttributes`.
        flags: U32,
        type_name: Str,
        type_namespace: Str,
        extends: Coded(TypeDefOrRef),
        /// Its first field (they run to the next type's first).
        field_list: List(Field),
        /// Its first method.
        method_list: List(MethodDef),
    }
    /// Field order indirection (uncompressed streams).
    FieldPtr = 0x03, FieldPtrRow { field: Index(Field) }
    /// Fields.
    Field = 0x04, FieldRow {
        /// `FieldAttributes`.
        flags: U16,
        name: Str,
        signature: Blob,
    }
    /// Method order indirection (uncompressed streams).
    MethodPtr = 0x05, MethodPtrRow { method: Index(MethodDef) }
    /// Methods.
    MethodDef = 0x06, MethodDefRow {
        /// Where its body is (0 if it has none).
        rva: U32,
        /// `MethodImplAttributes`.
        impl_flags: U16,
        /// `MethodAttributes`.
        flags: U16,
        name: Str,
        signature: Blob,
        /// Its first parameter.
        param_list: List(Param),
    }
    /// Parameter order indirection (uncompressed streams).
    ParamPtr = 0x07, ParamPtrRow { param: Index(Param) }
    /// Parameters (sequence 0 is the return value).
    Param = 0x08, ParamRow {
        /// `ParamAttributes`.
        flags: U16,
        sequence: U16,
        name: Str,
    }
    /// Interfaces a type implements.
    InterfaceImpl = 0x09, InterfaceImplRow {
        class: Index(TypeDef),
        interface: Coded(TypeDefOrRef),
    }
    /// References to methods and fields (of other modules, or of
    /// instantiated types, or vararg call sites).
    MemberRef = 0x0A, MemberRefRow {
        class: Coded(MemberRefParent),
        name: Str,
        signature: Blob,
    }
    /// Constant values of fields, parameters, and properties.
    Constant = 0x0B, ConstantRow {
        /// An element type (`ELEMENT_TYPE_*`).
        ty: U8,
        parent: Coded(HasConstant),
        value: Blob,
    }
    /// Custom attributes.
    CustomAttribute = 0x0C, CustomAttributeRow {
        parent: Coded(HasCustomAttribute),
        ty: Coded(CustomAttributeType),
        value: Blob,
    }
    /// Marshalling descriptors of fields and parameters.
    FieldMarshal = 0x0D, FieldMarshalRow {
        parent: Coded(HasFieldMarshal),
        native_type: Blob,
    }
    /// Declarative security (permission sets).
    DeclSecurity = 0x0E, DeclSecurityRow {
        /// A `SecurityAction`.
        action: U16,
        parent: Coded(HasDeclSecurity),
        permission_set: Blob,
    }
    /// Explicit layouts of types.
    ClassLayout = 0x0F, ClassLayoutRow {
        packing_size: U16,
        class_size: U32,
        parent: Index(TypeDef),
    }
    /// Explicit offsets of fields.
    FieldLayout = 0x10, FieldLayoutRow {
        offset: U32,
        field: Index(Field),
    }
    /// Signatures that stand alone: locals, and `calli`'s.
    StandAloneSig = 0x11, StandAloneSigRow { signature: Blob }
    /// Which types have events.
    EventMap = 0x12, EventMapRow {
        parent: Index(TypeDef),
        event_list: List(Event),
    }
    /// Event order indirection (uncompressed streams).
    EventPtr = 0x13, EventPtrRow { event: Index(Event) }
    /// Events.
    Event = 0x14, EventRow {
        /// `EventAttributes`.
        event_flags: U16,
        name: Str,
        event_type: Coded(TypeDefOrRef),
    }
    /// Which types have properties.
    PropertyMap = 0x15, PropertyMapRow {
        parent: Index(TypeDef),
        property_list: List(Property),
    }
    /// Property order indirection (uncompressed streams).
    PropertyPtr = 0x16, PropertyPtrRow { property: Index(Property) }
    /// Properties.
    Property = 0x17, PropertyRow {
        /// `PropertyAttributes`.
        flags: U16,
        name: Str,
        ty: Blob,
    }
    /// The methods of events and properties (getters, setters, adders, ...).
    MethodSemantics = 0x18, MethodSemanticsRow {
        /// `MethodSemanticsAttributes`.
        semantics: U16,
        method: Index(MethodDef),
        association: Coded(HasSemantics),
    }
    /// Explicit overrides (a method body that implements a declaration).
    MethodImpl = 0x19, MethodImplRow {
        class: Index(TypeDef),
        method_body: Coded(MethodDefOrRef),
        method_declaration: Coded(MethodDefOrRef),
    }
    /// References to modules (native DLLs for P/Invoke, too).
    ModuleRef = 0x1A, ModuleRefRow { name: Str }
    /// Type specifications (instantiations, arrays, pointers, ...).
    TypeSpec = 0x1B, TypeSpecRow { signature: Blob }
    /// P/Invoke: the native function a method is.
    ImplMap = 0x1C, ImplMapRow {
        /// `PInvokeAttributes`.
        mapping_flags: U16,
        member_forwarded: Coded(MemberForwarded),
        import_name: Str,
        import_scope: Index(ModuleRef),
    }
    /// Fields with initial data in the image.
    FieldRVA = 0x1D, FieldRVARow {
        rva: U32,
        field: Index(Field),
    }
    /// Edit-and-continue log.
    ENCLog = 0x1E, ENCLogRow {
        token: U32,
        func_code: U32,
    }
    /// Edit-and-continue token map.
    ENCMap = 0x1F, ENCMapRow { token: U32 }
    /// The assembly (one row, in its manifest module).
    Assembly = 0x20, AssemblyRow {
        /// `AssemblyHashAlgorithm`.
        hash_alg_id: U32,
        major_version: U16,
        minor_version: U16,
        build_number: U16,
        revision_number: U16,
        /// `AssemblyFlags`.
        flags: U32,
        public_key: Blob,
        name: Str,
        culture: Str,
    }
    /// (Unused.)
    AssemblyProcessor = 0x21, AssemblyProcessorRow { processor: U32 }
    /// (Unused.)
    AssemblyOS = 0x22, AssemblyOSRow {
        os_platform_id: U32,
        os_major_version: U32,
        os_minor_version: U32,
    }
    /// References to other assemblies.
    AssemblyRef = 0x23, AssemblyRefRow {
        major_version: U16,
        minor_version: U16,
        build_number: U16,
        revision_number: U16,
        /// `AssemblyFlags`.
        flags: U32,
        public_key_or_token: Blob,
        name: Str,
        culture: Str,
        hash_value: Blob,
    }
    /// (Unused.)
    AssemblyRefProcessor = 0x24, AssemblyRefProcessorRow {
        processor: U32,
        assembly_ref: Index(AssemblyRef),
    }
    /// (Unused.)
    AssemblyRefOS = 0x25, AssemblyRefOSRow {
        os_platform_id: U32,
        os_major_version: U32,
        os_minor_version: U32,
        assembly_ref: Index(AssemblyRef),
    }
    /// The assembly's other files (modules, linked resources).
    File = 0x26, FileRow {
        /// `FileAttributes` (1: no metadata).
        flags: U32,
        name: Str,
        hash_value: Blob,
    }
    /// Types the assembly exports from its other modules, and forwards to
    /// other assemblies.
    ExportedType = 0x27, ExportedTypeRow {
        /// `TypeAttributes` (0x00200000: a forwarder).
        flags: U32,
        /// The TypeDef's token in its module (a hint).
        type_def_id: U32,
        type_name: Str,
        type_namespace: Str,
        implementation: Coded(Implementation),
    }
    /// Manifest resources: embedded (at Offset in the CLI header's
    /// resources), in another file, or in another assembly.
    ManifestResource = 0x28, ManifestResourceRow {
        offset: U32,
        /// `ManifestResourceAttributes` (1 public, 2 private).
        flags: U32,
        name: Str,
        /// Null for a resource in this file.
        implementation: Coded(Implementation),
    }
    /// Which types are nested in which.
    NestedClass = 0x29, NestedClassRow {
        nested_class: Index(TypeDef),
        enclosing_class: Index(TypeDef),
    }
    /// Generic parameters of types and methods.
    GenericParam = 0x2A, GenericParamRow {
        number: U16,
        /// `GenericParameterAttributes`.
        flags: U16,
        owner: Coded(TypeOrMethodDef),
        name: Str,
    }
    /// Instantiations of generic methods.
    MethodSpec = 0x2B, MethodSpecRow {
        method: Coded(MethodDefOrRef),
        instantiation: Blob,
    }
    /// Constraints on generic parameters.
    GenericParamConstraint = 0x2C, GenericParamConstraintRow {
        owner: Index(GenericParam),
        constraint: Coded(TypeDefOrRef),
    }
    /// Portable PDB: source documents.
    Document = 0x30, DocumentRow {
        /// A document name blob (a separator and parts).
        name: Blob,
        hash_algorithm: Guid,
        hash: Blob,
        language: Guid,
    }
    /// Portable PDB: each method's sequence points.
    MethodDebugInformation = 0x31, MethodDebugInformationRow {
        document: Index(Document),
        sequence_points: Blob,
    }
    /// Portable PDB: lexical scopes.
    LocalScope = 0x32, LocalScopeRow {
        method: Index(MethodDef),
        import_scope: Index(ImportScope),
        variable_list: List(LocalVariable),
        constant_list: List(LocalConstant),
        start_offset: U32,
        length: U32,
    }
    /// Portable PDB: local variables.
    LocalVariable = 0x33, LocalVariableRow {
        attributes: U16,
        index: U16,
        name: Str,
    }
    /// Portable PDB: local constants.
    LocalConstant = 0x34, LocalConstantRow {
        name: Str,
        signature: Blob,
    }
    /// Portable PDB: namespace imports.
    ImportScope = 0x35, ImportScopeRow {
        parent: Index(ImportScope),
        imports: Blob,
    }
    /// Portable PDB: state machines' MoveNext and the methods that start
    /// them.
    StateMachineMethod = 0x36, StateMachineMethodRow {
        move_next_method: Index(MethodDef),
        kickoff_method: Index(MethodDef),
    }
    /// Portable PDB: custom debug information.
    CustomDebugInformation = 0x37, CustomDebugInformationRow {
        parent: Coded(HasCustomDebugInformation),
        kind: Guid,
        value: Blob,
    }
}

impl TableId {
    /// The `Ptr` table that orders this one, in uncompressed streams.
    pub const fn ptr_table(self) -> Option<TableId> {
        match self {
            TableId::Field => Some(TableId::FieldPtr),
            TableId::MethodDef => Some(TableId::MethodPtr),
            TableId::Param => Some(TableId::ParamPtr),
            TableId::Event => Some(TableId::EventPtr),
            TableId::Property => Some(TableId::PropertyPtr),
            _ => None,
        }
    }

    /// Whether this is one of the portable PDB's tables.
    pub const fn is_debug(self) -> bool {
        self as u8 >= 0x30
    }
}

impl fmt::Display for TableId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A table's place in the stream and its columns' layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableLayout {
    /// How many rows.
    pub rows: u32,
    /// Each row's size.
    pub row_size: u32,
    /// Where its rows start, from the start of the tables stream.
    pub offset: u32,
    /// Each column's offset in a row and width (2 or 4; 1 for none).
    pub columns: [(u8, u8); MAX_COLUMNS],
}

/// `HeapSizes` bits.
pub mod heap_sizes {
    /// `#Strings` indexes are 4 bytes.
    pub const STRINGS: u8 = 0x01;
    /// `#GUID` indexes are 4 bytes.
    pub const GUID: u8 = 0x02;
    /// `#Blob` indexes are 4 bytes.
    pub const BLOB: u8 = 0x04;
    /// (Uncompressed streams of edit-and-continue deltas.) Padding.
    pub const PADDING: u8 = 0x08;
    /// Delta-only data (edit and continue).
    pub const DELTA_ONLY: u8 = 0x20;
    /// Four extra bytes follow the row counts.
    pub const EXTRA_DATA: u8 = 0x40;
    /// Rows may be marked deleted (edit and continue).
    pub const HAS_DELETE: u8 = 0x80;
}

/// The tables stream (`#~` or `#-`).
#[derive(Clone, Debug)]
pub struct Tables<'d> {
    data: &'d [u8],
    /// Whether it was `#-` (uncompressed).
    pub uncompressed: bool,
    /// Reserved (0).
    pub reserved: u32,
    /// Its major version (2).
    pub major_version: u8,
    /// Its minor version (0).
    pub minor_version: u8,
    /// The `HeapSizes` flags ([`heap_sizes`]).
    pub heap_sizes: u8,
    /// Reserved (1; once the row-id width).
    pub reserved2: u8,
    /// Which tables are present (a bit for each).
    pub valid: u64,
    /// Which are sorted.
    pub sorted: u64,
    /// The extra data, with [`heap_sizes::EXTRA_DATA`].
    pub extra_data: Option<u32>,
    /// Each table's layout (row count 0 for one that's absent).
    layouts: Box<[TableLayout; 64]>,
    /// The width of `#Strings`, `#GUID`, and `#Blob` indexes.
    pub string_width: u8,
    /// See `string_width`.
    pub guid_width: u8,
    /// See `string_width`.
    pub blob_width: u8,
}

impl<'d> Tables<'d> {
    /// Parses a tables stream. `external_rows` holds the row counts of
    /// tables that another file has (a portable PDB's type-system tables,
    /// from its `#Pdb` stream), for the widths of indexes into them;
    /// `large_heaps` makes every heap index 4 bytes (a `#JTD` stream's
    /// minimal delta).
    pub fn parse(
        data: &'d [u8],
        uncompressed: bool,
        external_rows: Option<&[u32; 64]>,
        large_heaps: bool,
    ) -> Result<Self> {
        let mut c = Cursor::new(data);
        let reserved = c.u32("tables stream header")?;
        let major_version = c.u8("tables stream header")?;
        let minor_version = c.u8("tables stream header")?;
        let heap_sizes = c.u8("tables stream header")?;
        let reserved2 = c.u8("tables stream header")?;
        let valid = c.u64("tables stream header")?;
        let sorted = c.u64("tables stream header")?;
        let mut rows = [0u32; 64];
        for (i, r) in rows.iter_mut().enumerate() {
            if valid & (1 << i) == 0 {
                continue;
            }
            #[allow(clippy::cast_possible_truncation)]
            if TableId::from_u8(i as u8).is_none() {
                return Err(c.error(ErrorKind::Unsupported, "an unknown metadata table"));
            }
            *r = c.u32("table row counts")?;
            if *r > 0x00FF_FFFF {
                return Err(c.error(ErrorKind::Malformed, "table row count"));
            }
        }
        let extra_data = if heap_sizes & heap_sizes::EXTRA_DATA != 0 {
            Some(c.u32("tables extra data")?)
        } else {
            None
        };
        let big = |bit: u8| {
            if large_heaps || heap_sizes & bit != 0 {
                4
            } else {
                2
            }
        };
        let string_width = big(heap_sizes::STRINGS);
        let guid_width = big(heap_sizes::GUID);
        let blob_width = big(heap_sizes::BLOB);

        // Row counts for widths: this stream's, else the other file's.
        let mut counted = rows;
        if let Some(ext) = external_rows {
            for (i, r) in counted.iter_mut().enumerate() {
                if valid & (1 << i) == 0 {
                    *r = ext[i];
                }
            }
        }
        let index_width = |t: TableId| {
            if counted[t as usize] < 0x1_0000 {
                2u8
            } else {
                4
            }
        };
        let width = |kind: ColumnKind| -> u8 {
            match kind {
                ColumnKind::U8 | ColumnKind::U16 => 2,
                ColumnKind::U32 => 4,
                ColumnKind::Str => string_width,
                ColumnKind::Guid => guid_width,
                ColumnKind::Blob => blob_width,
                ColumnKind::Index(t) => index_width(t),
                ColumnKind::List(t) => match t.ptr_table() {
                    Some(p) => index_width(t).max(index_width(p)),
                    None => index_width(t),
                },
                ColumnKind::Coded(k) => {
                    let limit = 1u32 << (16 - k.tag_bits());
                    let max = k
                        .tables()
                        .iter()
                        .flatten()
                        .map(|&t| counted[t as usize])
                        .max()
                        .unwrap_or(0);
                    if max < limit { 2 } else { 4 }
                }
            }
        };

        let mut layouts = Box::new([TableLayout::default(); 64]);
        let mut at = c.pos() as u64;
        for &t in TableId::ALL {
            let l = &mut layouts[t as usize];
            let mut offset = 0u8;
            for (i, col) in t.columns().iter().enumerate() {
                let w = width(col.kind);
                l.columns[i] = (offset, w);
                offset += w;
            }
            l.row_size = u32::from(offset);
            l.rows = rows[t as usize];
            if l.rows == 0 {
                continue;
            }
            let size = u64::from(l.rows) * u64::from(l.row_size);
            if at + size > data.len() as u64 {
                return Err(Error::new(ErrorKind::Truncated, at, "metadata table rows"));
            }
            l.offset = u32::try_from(at)
                .map_err(|_| Error::new(ErrorKind::Overflow, at, "metadata tables"))?;
            at += size;
        }
        Ok(Self {
            data,
            uncompressed,
            reserved,
            major_version,
            minor_version,
            heap_sizes,
            reserved2,
            valid,
            sorted,
            extra_data,
            layouts,
            string_width,
            guid_width,
            blob_width,
        })
    }

    /// The stream's bytes.
    pub fn data(&self) -> &'d [u8] {
        self.data
    }

    /// Where the rows end (any bytes after are padding).
    pub fn end(&self) -> u32 {
        self.layouts
            .iter()
            .filter(|l| l.rows > 0)
            .map(|l| l.offset + l.rows * l.row_size)
            .max()
            .unwrap_or(0)
    }

    /// `table`'s layout.
    pub fn layout(&self, table: TableId) -> &TableLayout {
        &self.layouts[table as usize]
    }

    /// How many rows `table` has.
    pub fn rows(&self, table: TableId) -> u32 {
        self.layouts[table as usize].rows
    }

    /// Whether `table` is marked sorted.
    pub fn is_sorted(&self, table: TableId) -> bool {
        self.sorted & (1 << table as u32) != 0
    }

    /// Row `rid`'s bytes.
    pub fn raw_row(&self, table: TableId, rid: u32) -> Option<&'d [u8]> {
        let l = &self.layouts[table as usize];
        if rid == 0 || rid > l.rows {
            return None;
        }
        let at = l.offset as usize + (rid as usize - 1) * l.row_size as usize;
        self.data.get(at..at + l.row_size as usize)
    }

    /// Column `column` of row `rid`.
    pub fn column(&self, table: TableId, rid: u32, column: usize) -> Option<u32> {
        let row = self.raw_row(table, rid)?;
        let (offset, width) = *self.layouts[table as usize].columns.get(column)?;
        if column >= table.columns().len() {
            return None;
        }
        let b = row.get(offset as usize..offset as usize + width as usize)?;
        Some(
            read_le(b)
                & if table.columns()[column].kind == ColumnKind::U8 {
                    0xFF
                } else {
                    u32::MAX
                },
        )
    }

    /// Every column of row `rid`, in schema order.
    pub fn columns(&self, table: TableId, rid: u32) -> Option<[u32; MAX_COLUMNS]> {
        let row = self.raw_row(table, rid)?;
        let l = &self.layouts[table as usize];
        let mut out = [0u32; MAX_COLUMNS];
        for (i, col) in table.columns().iter().enumerate() {
            let (offset, width) = l.columns[i];
            let v = read_le(&row[offset as usize..offset as usize + width as usize]);
            out[i] = if col.kind == ColumnKind::U8 {
                v & 0xFF
            } else {
                v
            };
        }
        Some(out)
    }

    /// Row `rid` of `R`'s table.
    pub fn get<R: Row>(&self, rid: u32) -> Option<R> {
        self.columns(R::TABLE, rid).map(|c| R::from_columns(&c))
    }

    /// Every row of `R`'s table, with its rid.
    pub fn iter<R: Row>(&self) -> impl ExactSizeIterator<Item = (u32, R)> + '_ {
        (1..self.rows(R::TABLE) + 1).map(move |rid| {
            let c = self.columns(R::TABLE, rid).unwrap_or([0; MAX_COLUMNS]);
            (rid, R::from_columns(&c))
        })
    }

    /// The row a list column's position names: through the `Ptr` table if
    /// there is one.
    pub fn resolve_list(&self, table: TableId, position: u32) -> u32 {
        if let Some(p) = table.ptr_table()
            && self.rows(p) > 0
        {
            return self.column(p, position, 0).unwrap_or(0);
        }
        position
    }

    /// The positions a list column of `owner`'s row `rid` (column
    /// `column`, into `target`) spans: from its value to the next row's
    /// (or the end), as positions in the `Ptr` table if there is one.
    pub fn list_positions(
        &self,
        owner: TableId,
        rid: u32,
        column: usize,
        target: TableId,
    ) -> core::ops::Range<u32> {
        let count = match target.ptr_table() {
            Some(p) if self.rows(p) > 0 => self.rows(p),
            _ => self.rows(target),
        };
        let Some(start) = self.column(owner, rid, column) else {
            return 0..0;
        };
        let end = if rid < self.rows(owner) {
            self.column(owner, rid + 1, column).unwrap_or(count + 1)
        } else {
            count + 1
        };
        let start = start.clamp(1, count + 1);
        let end = end.clamp(start, count + 1);
        start..end
    }

    /// The rows of `target` that `owner`'s row `rid` lists (column
    /// `column`).
    pub fn list(
        &self,
        owner: TableId,
        rid: u32,
        column: usize,
        target: TableId,
    ) -> impl Iterator<Item = u32> + '_ {
        self.list_positions(owner, rid, column, target)
            .map(move |p| self.resolve_list(target, p))
    }
}

fn read_le(b: &[u8]) -> u32 {
    b.iter().rev().fold(0u32, |v, &x| (v << 8) | u32::from(x))
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn schema_is_whole() {
        // Every table of II.22 and of the portable PDB, at its number.
        assert_eq!(TableId::ALL.len(), 45 + 8);
        for &t in TableId::ALL {
            assert_eq!(TableId::from_u8(t as u8), Some(t));
            assert!(!t.columns().is_empty() && t.columns().len() <= MAX_COLUMNS);
        }
        assert_eq!(TableId::from_u8(0x2D), None);
        assert_eq!(TableId::from_u8(0x38), None);
        assert_eq!(TableId::Assembly.columns().len(), 9);
        assert_eq!(TableId::FieldRVA.name(), "FieldRVA");
    }

    #[test]
    fn coded_index_tags() {
        let bits: Vec<u32> = CodedIndexKind::ALL.iter().map(|k| k.tag_bits()).collect();
        assert_eq!(bits, [2, 2, 5, 1, 2, 3, 1, 1, 1, 2, 3, 2, 1, 5]);
        let c = CodedIndex::new(CodedIndexKind::TypeDefOrRef, (5 << 2) | 1);
        assert_eq!(c.token(), Some(Token::new(TableId::TypeRef, 5)));
        assert_eq!(
            CodedIndexKind::TypeDefOrRef.encode(Token::new(TableId::TypeRef, 5)),
            Some(c.raw)
        );
        assert_eq!(
            CodedIndexKind::TypeDefOrRef.encode(Token::new(TableId::Field, 5)),
            None
        );
        // An unused tag.
        let c = CodedIndex::new(CodedIndexKind::CustomAttributeType, (3 << 3) | 1);
        assert_eq!((c.table(), c.token_or_null()), (None, Token(0)));
        // A tag past the table list.
        let c = CodedIndex::new(CodedIndexKind::TypeDefOrRef, 3);
        assert_eq!(c.table(), None);
        let c = CodedIndex::new(CodedIndexKind::HasCustomAttribute, 31);
        assert_eq!(c.table(), None);
        assert_eq!(
            CodedIndexKind::HasCustomDebugInformation.encode(Token::new(TableId::ImportScope, 1)),
            Some(32 | 26)
        );
    }

    fn stream(heap: u8, rows: &[(TableId, u32)], body: usize) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0, 2, 0, heap, 1];
        let valid = rows.iter().fold(0u64, |m, (t, _)| m | 1 << *t as u32);
        v.extend_from_slice(&valid.to_le_bytes());
        v.extend_from_slice(&0u64.to_le_bytes());
        for (_, n) in rows {
            v.extend_from_slice(&n.to_le_bytes());
        }
        v.resize(v.len() + body, 0);
        v
    }

    #[test]
    fn widths_follow_row_counts_and_heap_flags() {
        let data = stream(0, &[(TableId::Module, 1), (TableId::TypeDef, 2)], 10 + 28);
        let t = Tables::parse(&data, false, None, false).unwrap();
        assert_eq!(t.layout(TableId::Module).row_size, 10);
        assert_eq!(t.layout(TableId::TypeDef).row_size, 14);
        assert_eq!(t.layout(TableId::TypeDef).offset, 24 + 8 + 10);
        // Wide heaps.
        let data = stream(7, &[(TableId::Module, 1)], 20);
        let t = Tables::parse(&data, false, None, false).unwrap();
        assert_eq!(t.layout(TableId::Module).row_size, 2 + 4 + 4 * 3);
        let data = stream(0, &[(TableId::Module, 1)], 18);
        let t = Tables::parse(&data, false, None, true).unwrap();
        assert_eq!(t.layout(TableId::Module).row_size, 18);
        // 2^14 TypeRefs make TypeDefOrRef (2 tag bits) 4 bytes; 2^14 - 1
        // don't.
        for (n, w) in [(0x3FFF, 2), (0x4000, 4)] {
            let data = stream(0, &[(TableId::TypeRef, n), (TableId::InterfaceImpl, 1)], 0);
            let t = Tables::parse(&data, false, None, false);
            assert!(t.is_err());
            // (TypeRef's own ResolutionScope widens with it.)
            let size = n as usize * (4 + w) + 2 + w;
            let data = stream(
                0,
                &[(TableId::TypeRef, n), (TableId::InterfaceImpl, 1)],
                size,
            );
            let t = Tables::parse(&data, false, None, false).unwrap();
            assert_eq!(t.layout(TableId::InterfaceImpl).columns[1].1 as usize, w);
        }
        // HasCustomAttribute (5 bits): 2^11 rows of any of its tables.
        let data = stream(
            0,
            &[(TableId::Param, 0x800), (TableId::CustomAttribute, 1)],
            0x800 * 6 + 10,
        );
        let t = Tables::parse(&data, false, None, false).unwrap();
        assert_eq!(t.layout(TableId::CustomAttribute).row_size, 4 + 2 + 2);
        // A PDB's counts of the type system's tables, from #Pdb.
        let mut ext = [0u32; 64];
        ext[TableId::MethodDef as usize] = 0x1_0000;
        let data = stream(0, &[(TableId::StateMachineMethod, 1)], 8);
        let t = Tables::parse(&data, false, Some(&ext), false).unwrap();
        assert_eq!(t.layout(TableId::StateMachineMethod).row_size, 8);
    }

    #[test]
    fn extra_data_and_damage() {
        let mut data = stream(heap_sizes::EXTRA_DATA, &[(TableId::ModuleRef, 1)], 4 + 2);
        data[28..32].copy_from_slice(&0x1234u32.to_le_bytes());
        let t = Tables::parse(&data, true, None, false).unwrap();
        assert_eq!(t.extra_data, Some(0x1234));
        assert_eq!(t.layout(TableId::ModuleRef).offset, 32);
        // An unknown table.
        let mut data = stream(0, &[], 8);
        data[8] = 0;
        data[13] = 0x20; // bit 0x2D
        assert_eq!(
            Tables::parse(&data, false, None, false).unwrap_err().kind,
            ErrorKind::Unsupported
        );
        // A row count past 2^24.
        let data = stream(0, &[(TableId::ModuleRef, 0x0100_0000)], 0);
        assert!(Tables::parse(&data, false, None, false).is_err());
        for n in 0..24 {
            assert!(Tables::parse(&data[..n], false, None, false).is_err());
        }
    }

    #[test]
    fn rows_and_lists() {
        // Three types; fields 1-2 belong to the first, none to the second,
        // 3 to the third.
        let mut data = stream(0, &[(TableId::TypeDef, 3), (TableId::Field, 3)], 0);
        for (flags, fields) in [(1u32, 1u16), (2, 3), (3, 3)] {
            data.extend_from_slice(&flags.to_le_bytes());
            data.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
            data.extend_from_slice(&fields.to_le_bytes());
            data.extend_from_slice(&1u16.to_le_bytes());
        }
        data.extend_from_slice(&[0; 18]);
        let t = Tables::parse(&data, false, None, false).unwrap();
        let r: TypeDefRow = t.get(2).unwrap();
        assert_eq!((r.flags, r.field_list), (2, 3));
        assert!(t.get::<TypeDefRow>(0).is_none() && t.get::<TypeDefRow>(4).is_none());
        let lists: Vec<Vec<u32>> = (1..=3)
            .map(|rid| t.list(TableId::TypeDef, rid, 4, TableId::Field).collect())
            .collect();
        assert_eq!(lists, [vec![1, 2], vec![], vec![3]]);
        assert_eq!(t.iter::<TypeDefRow>().len(), 3);
        assert_eq!(t.column(TableId::TypeDef, 1, 9), None);
    }

    #[test]
    fn lists_through_ptr_tables() {
        // FieldPtr reverses the fields.
        let mut data = stream(
            0,
            &[
                (TableId::TypeDef, 1),
                (TableId::FieldPtr, 2),
                (TableId::Field, 2),
            ],
            0,
        );
        data.extend_from_slice(&[0; 10]);
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&2u16.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        data.extend_from_slice(&[0; 12]);
        let t = Tables::parse(&data, true, None, false).unwrap();
        let fields: Vec<u32> = t.list(TableId::TypeDef, 1, 4, TableId::Field).collect();
        assert_eq!(fields, [2, 1]);
        // A list start past the end, and one going backwards: clamped.
        let mut data = stream(0, &[(TableId::TypeDef, 2), (TableId::Field, 1)], 0);
        for fields in [9u16, 1] {
            data.extend_from_slice(&[0; 10]);
            data.extend_from_slice(&fields.to_le_bytes());
            data.extend_from_slice(&[0; 2]);
        }
        data.extend_from_slice(&[0; 6]);
        let t = Tables::parse(&data, false, None, false).unwrap();
        assert_eq!(t.list(TableId::TypeDef, 1, 4, TableId::Field).count(), 0);
        assert_eq!(
            t.list(TableId::TypeDef, 2, 4, TableId::Field)
                .collect::<Vec<_>>(),
            [1]
        );
    }
}

//! The metadata root (II.24.2.1): `BSJB`, the runtime version string, and
//! the stream headers; and [`Metadata`], the streams parsed, with the
//! lookups between tables that readers need (a type's fields and methods,
//! a method's type, a type's enclosing type, a token's custom attributes,
//! ...).
//!
//! The same root starts an assembly's metadata (in its PE image, at the CLI
//! header's metadata directory) and a portable PDB file.

use std::borrow::Cow;
use std::sync::OnceLock;

use turbonop_core::{Error, ErrorKind, Result};

use crate::heaps::{BlobHeap, BlobIndex, GuidHeap, StringIndex, StringsHeap, UserStringsHeap};
use crate::pdb::PdbStream;
use crate::reader::Cursor;
use crate::tables::{
    AssemblyRow, CodedIndexKind, MAX_COLUMNS, Row, TableId, Tables, Token, TypeDefRow,
};

/// The metadata root's signature, `BSJB`.
pub const METADATA_SIGNATURE: u32 = 0x424A_5342;

/// A stream: its header's name and extent, and its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stream<'d> {
    /// Its name (`#~`, `#Strings`, ...).
    pub name: &'d [u8],
    /// Where it starts, from the metadata root.
    pub offset: u32,
    /// Its size.
    pub size: u32,
    /// Its bytes.
    pub data: &'d [u8],
}

/// The metadata root's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetadataHeader<'d> {
    /// Major version (1).
    pub major_version: u16,
    /// Minor version (1).
    pub minor_version: u16,
    /// Reserved (0).
    pub reserved: u32,
    /// The version string as stored, padding and all.
    pub version_raw: &'d [u8],
    /// The version string, to its first NUL (`v4.0.30319`, or `PDB v1.0`).
    pub version: &'d [u8],
    /// Flags (0).
    pub flags: u16,
}

/// Lookups built on first use: which type owns each field and method,
/// and each keyed table's rows sorted by key.
#[derive(Debug, Default)]
struct Derived {
    field_owner: OnceLock<Vec<u32>>,
    method_owner: OnceLock<Vec<u32>>,
    keyed: [OnceLock<Vec<(u32, u32)>>; KEYS.len()],
}

/// The keyed columns: (table, column).
const KEYS: [(TableId, usize); 21] = [
    (TableId::CustomAttribute, 0),
    (TableId::Constant, 1),
    (TableId::FieldMarshal, 0),
    (TableId::DeclSecurity, 1),
    (TableId::ClassLayout, 2),
    (TableId::FieldLayout, 1),
    (TableId::EventMap, 0),
    (TableId::PropertyMap, 0),
    (TableId::MethodSemantics, 2),
    (TableId::MethodImpl, 0),
    (TableId::ImplMap, 1),
    (TableId::FieldRVA, 1),
    (TableId::NestedClass, 0),
    (TableId::NestedClass, 1),
    (TableId::GenericParam, 2),
    (TableId::GenericParamConstraint, 0),
    (TableId::InterfaceImpl, 0),
    (TableId::CustomDebugInformation, 0),
    (TableId::LocalScope, 0),
    (TableId::StateMachineMethod, 0),
    (TableId::MethodSemantics, 1),
];

#[derive(Clone, Copy)]
enum Key {
    CustomAttribute,
    Constant,
    FieldMarshal,
    DeclSecurity,
    ClassLayout,
    FieldLayout,
    EventMap,
    PropertyMap,
    MethodSemantics,
    MethodImpl,
    ImplMap,
    FieldRva,
    NestedClass,
    EnclosingClass,
    GenericParam,
    GenericParamConstraint,
    InterfaceImpl,
    CustomDebugInformation,
    LocalScope,
    StateMachineMethod,
    SemanticsMethod,
}

/// An assembly's version (or a reference's).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// Major.
    pub major: u16,
    /// Minor.
    pub minor: u16,
    /// Build.
    pub build: u16,
    /// Revision.
    pub revision: u16,
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}",
            self.major, self.minor, self.build, self.revision
        )
    }
}

/// Parsed metadata: the root, its streams, the heaps, and the tables.
#[derive(Debug)]
pub struct Metadata<'d> {
    data: &'d [u8],
    header: MetadataHeader<'d>,
    streams: Vec<Stream<'d>>,
    strings: StringsHeap<'d>,
    user_strings: UserStringsHeap<'d>,
    guids: GuidHeap<'d>,
    blobs: BlobHeap<'d>,
    tables: Tables<'d>,
    tables_stream: Option<usize>,
    pdb: Option<PdbStream<'d>>,
    derived: Derived,
}

static NO_TABLES: [u8; 24] = [0; 24];

impl<'d> Metadata<'d> {
    /// Parses metadata that starts at `data` (the metadata root).
    pub fn parse(data: &'d [u8]) -> Result<Self> {
        let mut c = Cursor::new(data);
        if c.u32("metadata signature")? != METADATA_SIGNATURE {
            return Err(Error::new(ErrorKind::BadMagic, 0, "metadata signature"));
        }
        let major_version = c.u16("metadata root")?;
        let minor_version = c.u16("metadata root")?;
        let reserved = c.u32("metadata root")?;
        let len = c.u32("metadata version length")? as usize;
        let version_raw = c.bytes(len, "metadata version string")?;
        let version = &version_raw[..version_raw.iter().position(|&b| b == 0).unwrap_or(len)];
        // (The length should be a multiple of four; whether or not, the
        // stream headers follow the string as its length says.)
        let flags = c.u16("metadata root")?;
        let count = c.u16("metadata stream count")?;
        let mut streams = Vec::new();
        for _ in 0..count {
            let offset = c.u32("stream header")?;
            let size = c.u32("stream header")?;
            let rest = &data[c.pos()..];
            let n = rest
                .iter()
                .take(32)
                .position(|&b| b == 0)
                .ok_or_else(|| c.error(ErrorKind::Malformed, "stream name"))?;
            let name = &rest[..n];
            c.seek(c.pos() + n + 1, "stream name")?;
            c.align(4, "stream name")?;
            let body = (offset as usize)
                .checked_add(size as usize)
                .and_then(|end| data.get(offset as usize..end))
                .ok_or(Error::new(
                    ErrorKind::Truncated,
                    u64::from(offset),
                    "metadata stream",
                ))?;
            streams.push(Stream {
                name,
                offset,
                size,
                data: body,
            });
        }
        // The first stream of each name counts.
        let find = |name: &[u8]| streams.iter().position(|s| s.name == name);
        let heap = |name: &[u8]| find(name).map_or(&[][..], |i| streams[i].data);
        let pdb = find(b"#Pdb")
            .map(|i| PdbStream::parse(streams[i].data, u64::from(streams[i].offset)))
            .transpose()?;
        let compressed = find(b"#~");
        let uncompressed = find(b"#-");
        let tables_stream = compressed.or(uncompressed);
        let large = find(b"#JTD").is_some();
        let tables = match tables_stream {
            Some(i) => Tables::parse(
                streams[i].data,
                compressed.is_none(),
                pdb.as_ref().map(|p| &p.type_system_rows),
                large,
            )
            .map_err(|e| Error::new(e.kind, e.offset + u64::from(streams[i].offset), e.what))?,
            None => Tables::parse(&NO_TABLES, false, None, false)?,
        };
        Ok(Self {
            data,
            header: MetadataHeader {
                major_version,
                minor_version,
                reserved,
                version_raw,
                version,
                flags,
            },
            strings: StringsHeap::new(heap(b"#Strings")),
            user_strings: UserStringsHeap::new(heap(b"#US")),
            guids: GuidHeap::new(heap(b"#GUID")),
            blobs: BlobHeap::new(heap(b"#Blob")),
            streams,
            tables,
            tables_stream,
            pdb,
            derived: Derived::default(),
        })
    }

    /// The metadata's bytes (from the root).
    pub fn data(&self) -> &'d [u8] {
        self.data
    }

    /// The root's header.
    pub fn header(&self) -> &MetadataHeader<'d> {
        &self.header
    }

    /// The runtime version string (`v4.0.30319`).
    pub fn version(&self) -> Cow<'d, str> {
        String::from_utf8_lossy(self.header.version)
    }

    /// The streams, in header order.
    pub fn streams(&self) -> &[Stream<'d>] {
        &self.streams
    }

    /// The first stream named `name`.
    pub fn stream(&self, name: &[u8]) -> Option<&Stream<'d>> {
        self.streams.iter().find(|s| s.name == name)
    }

    /// The tables stream's header, if there is one.
    pub fn tables_stream(&self) -> Option<&Stream<'d>> {
        self.tables_stream.map(|i| &self.streams[i])
    }

    /// `#Strings`.
    pub fn strings(&self) -> &StringsHeap<'d> {
        &self.strings
    }

    /// `#US`.
    pub fn user_strings(&self) -> &UserStringsHeap<'d> {
        &self.user_strings
    }

    /// `#GUID`.
    pub fn guids(&self) -> &GuidHeap<'d> {
        &self.guids
    }

    /// `#Blob`.
    pub fn blobs(&self) -> &BlobHeap<'d> {
        &self.blobs
    }

    /// The tables.
    pub fn tables(&self) -> &Tables<'d> {
        &self.tables
    }

    /// The portable PDB's `#Pdb` stream, if this is a PDB.
    pub fn pdb(&self) -> Option<&PdbStream<'d>> {
        self.pdb.as_ref()
    }

    /// The string at `index`, as text (`""` if the index is bad).
    pub fn string(&self, index: StringIndex) -> Cow<'d, str> {
        self.strings.text(index)
    }

    /// The blob at `index`.
    pub fn blob(&self, index: BlobIndex) -> Result<&'d [u8]> {
        self.blobs.get(index)
    }

    /// Row `rid` of `R`'s table.
    pub fn row<R: Row>(&self, rid: u32) -> Option<R> {
        self.tables.get(rid)
    }

    /// Whether `token` names a row that exists.
    pub fn has_row(&self, token: Token) -> bool {
        token
            .table()
            .is_some_and(|t| token.rid() >= 1 && token.rid() <= self.tables.rows(t))
    }

    // Ownership: lists.

    /// The fields of type `typedef`, in order.
    pub fn type_fields(&self, typedef: u32) -> impl Iterator<Item = u32> + '_ {
        self.tables
            .list(TableId::TypeDef, typedef, 4, TableId::Field)
    }

    /// The methods of type `typedef`, in order.
    pub fn type_methods(&self, typedef: u32) -> impl Iterator<Item = u32> + '_ {
        self.tables
            .list(TableId::TypeDef, typedef, 5, TableId::MethodDef)
    }

    /// The parameters of method `method` (the return value's, sequence 0,
    /// first if it has one).
    pub fn method_params(&self, method: u32) -> impl Iterator<Item = u32> + '_ {
        self.tables
            .list(TableId::MethodDef, method, 5, TableId::Param)
    }

    /// The events of type `typedef` (by its EventMap row).
    pub fn type_events(&self, typedef: u32) -> Vec<u32> {
        match self.lookup(Key::EventMap, typedef).first() {
            Some(&(_, map)) => self
                .tables
                .list(TableId::EventMap, map, 1, TableId::Event)
                .collect(),
            None => Vec::new(),
        }
    }

    /// The properties of type `typedef` (by its PropertyMap row).
    pub fn type_properties(&self, typedef: u32) -> Vec<u32> {
        match self.lookup(Key::PropertyMap, typedef).first() {
            Some(&(_, map)) => self
                .tables
                .list(TableId::PropertyMap, map, 1, TableId::Property)
                .collect(),
            None => Vec::new(),
        }
    }

    /// The local variables of portable PDB scope `scope`.
    pub fn scope_variables(&self, scope: u32) -> impl Iterator<Item = u32> + '_ {
        self.tables
            .list(TableId::LocalScope, scope, 2, TableId::LocalVariable)
    }

    /// The local constants of portable PDB scope `scope`.
    pub fn scope_constants(&self, scope: u32) -> impl Iterator<Item = u32> + '_ {
        self.tables
            .list(TableId::LocalScope, scope, 3, TableId::LocalConstant)
    }

    fn owners(&self, column: usize, target: TableId) -> Vec<u32> {
        let positions = match target.ptr_table() {
            Some(p) if self.tables.rows(p) > 0 => self.tables.rows(p),
            _ => self.tables.rows(target),
        };
        let mut owner = vec![0u32; self.tables.rows(target) as usize + 1];
        for t in 1..=self.tables.rows(TableId::TypeDef) {
            for p in self
                .tables
                .list_positions(TableId::TypeDef, t, column, target)
            {
                let rid = self.tables.resolve_list(target, p.min(positions));
                if let Some(o) = owner.get_mut(rid as usize) {
                    *o = t;
                }
            }
        }
        owner
    }

    /// The type that declares field `field`.
    pub fn field_owner(&self, field: u32) -> Option<u32> {
        let o = self
            .derived
            .field_owner
            .get_or_init(|| self.owners(4, TableId::Field));
        o.get(field as usize).copied().filter(|&t| t != 0)
    }

    /// The type that declares method `method`.
    pub fn method_owner(&self, method: u32) -> Option<u32> {
        let o = self
            .derived
            .method_owner
            .get_or_init(|| self.owners(5, TableId::MethodDef));
        o.get(method as usize).copied().filter(|&t| t != 0)
    }

    // Keyed tables.

    fn index(&self, key: Key) -> &[(u32, u32)] {
        let (table, column) = KEYS[key as usize];
        self.derived.keyed[key as usize].get_or_init(|| {
            let mut v: Vec<(u32, u32)> = (1..=self.tables.rows(table))
                .map(|rid| (self.tables.column(table, rid, column).unwrap_or(0), rid))
                .collect();
            if !v.is_sorted() {
                v.sort_unstable();
            }
            v
        })
    }

    fn lookup(&self, key: Key, value: u32) -> &[(u32, u32)] {
        let v = self.index(key);
        let start = v.partition_point(|&(k, _)| k < value);
        let end = start + v[start..].partition_point(|&(k, _)| k == value);
        &v[start..end]
    }

    fn lookup_coded(&self, key: Key, kind: CodedIndexKind, token: Token) -> Vec<u32> {
        match kind.encode(token) {
            Some(code) if !token.is_null() => {
                self.lookup(key, code).iter().map(|&(_, r)| r).collect()
            }
            _ => Vec::new(),
        }
    }

    fn lookup_rows(&self, key: Key, value: u32) -> Vec<u32> {
        if value == 0 {
            return Vec::new();
        }
        self.lookup(key, value).iter().map(|&(_, r)| r).collect()
    }

    /// The type that type `typedef` is nested in.
    pub fn enclosing_type(&self, typedef: u32) -> Option<u32> {
        let &(_, row) = self.lookup(Key::NestedClass, typedef).first()?;
        self.tables
            .column(TableId::NestedClass, row, 1)
            .filter(|&t| t != 0)
    }

    /// The types nested in type `typedef`, in NestedClass order.
    pub fn nested_types(&self, typedef: u32) -> Vec<u32> {
        self.lookup_rows(Key::EnclosingClass, typedef)
            .into_iter()
            .filter_map(|row| self.tables.column(TableId::NestedClass, row, 0))
            .collect()
    }

    /// The CustomAttribute rows of `token`.
    pub fn custom_attributes(&self, token: Token) -> Vec<u32> {
        self.lookup_coded(
            Key::CustomAttribute,
            CodedIndexKind::HasCustomAttribute,
            token,
        )
    }

    /// The Constant row of `token` (a field, parameter, or property).
    pub fn constant(&self, token: Token) -> Option<u32> {
        self.lookup_coded(Key::Constant, CodedIndexKind::HasConstant, token)
            .first()
            .copied()
    }

    /// The FieldMarshal row of `token` (a field or parameter).
    pub fn field_marshal(&self, token: Token) -> Option<u32> {
        self.lookup_coded(Key::FieldMarshal, CodedIndexKind::HasFieldMarshal, token)
            .first()
            .copied()
    }

    /// The DeclSecurity rows of `token` (a type, method, or the assembly).
    pub fn decl_security(&self, token: Token) -> Vec<u32> {
        self.lookup_coded(Key::DeclSecurity, CodedIndexKind::HasDeclSecurity, token)
    }

    /// The ClassLayout row of type `typedef`.
    pub fn class_layout(&self, typedef: u32) -> Option<u32> {
        self.lookup_rows(Key::ClassLayout, typedef).first().copied()
    }

    /// The FieldLayout row of field `field`.
    pub fn field_layout(&self, field: u32) -> Option<u32> {
        self.lookup_rows(Key::FieldLayout, field).first().copied()
    }

    /// The FieldRVA row of field `field`.
    pub fn field_rva(&self, field: u32) -> Option<u32> {
        self.lookup_rows(Key::FieldRva, field).first().copied()
    }

    /// The ImplMap row of `token` (a method, or a field).
    pub fn impl_map(&self, token: Token) -> Option<u32> {
        self.lookup_coded(Key::ImplMap, CodedIndexKind::MemberForwarded, token)
            .first()
            .copied()
    }

    /// The MethodSemantics rows of `token` (an event or property).
    pub fn method_semantics(&self, token: Token) -> Vec<u32> {
        self.lookup_coded(Key::MethodSemantics, CodedIndexKind::HasSemantics, token)
    }

    /// The MethodSemantics rows that name method `method`.
    pub fn semantics_of_method(&self, method: u32) -> Vec<u32> {
        self.lookup_rows(Key::SemanticsMethod, method)
    }

    /// The MethodImpl rows of type `typedef`.
    pub fn method_impls(&self, typedef: u32) -> Vec<u32> {
        self.lookup_rows(Key::MethodImpl, typedef)
    }

    /// The InterfaceImpl rows of type `typedef`.
    pub fn interface_impls(&self, typedef: u32) -> Vec<u32> {
        self.lookup_rows(Key::InterfaceImpl, typedef)
    }

    /// The GenericParam rows of `token` (a type or method).
    pub fn generic_params(&self, token: Token) -> Vec<u32> {
        self.lookup_coded(Key::GenericParam, CodedIndexKind::TypeOrMethodDef, token)
    }

    /// The GenericParamConstraint rows of generic parameter `param`.
    pub fn generic_param_constraints(&self, param: u32) -> Vec<u32> {
        self.lookup_rows(Key::GenericParamConstraint, param)
    }

    /// The CustomDebugInformation rows of `token` (portable PDBs).
    pub fn custom_debug_information(&self, token: Token) -> Vec<u32> {
        self.lookup_coded(
            Key::CustomDebugInformation,
            CodedIndexKind::HasCustomDebugInformation,
            token,
        )
    }

    /// The LocalScope rows of method `method` (portable PDBs).
    pub fn local_scopes(&self, method: u32) -> Vec<u32> {
        self.lookup_rows(Key::LocalScope, method)
    }

    /// The kickoff method of state machine method `move_next` (portable
    /// PDBs).
    pub fn kickoff_method(&self, move_next: u32) -> Option<u32> {
        let row = *self
            .lookup_rows(Key::StateMachineMethod, move_next)
            .first()?;
        self.tables
            .column(TableId::StateMachineMethod, row, 1)
            .filter(|&m| m != 0)
    }

    // The assembly.

    /// The Assembly row, if this module is an assembly's manifest.
    pub fn assembly(&self) -> Option<AssemblyRow> {
        self.row(1)
    }

    /// The assembly's name.
    pub fn assembly_name(&self) -> Option<Cow<'d, str>> {
        self.assembly().map(|a| self.string(a.name))
    }

    /// The assembly's version.
    pub fn assembly_version(&self) -> Option<Version> {
        self.assembly().map(|a| Version {
            major: a.major_version,
            minor: a.minor_version,
            build: a.build_number,
            revision: a.revision_number,
        })
    }

    /// The module's name.
    pub fn module_name(&self) -> Option<Cow<'d, str>> {
        self.row::<crate::tables::ModuleRow>(1)
            .map(|m| self.string(m.name))
    }

    /// The TypeDef row whose namespace and name are `namespace` and `name`,
    /// and that isn't nested.
    pub fn find_type(&self, namespace: &str, name: &str) -> Option<u32> {
        self.tables
            .iter::<TypeDefRow>()
            .find(|(rid, t)| {
                self.string(t.type_name) == name
                    && self.string(t.type_namespace) == namespace
                    && self.enclosing_type(*rid).is_none()
            })
            .map(|(rid, _)| rid)
    }

    /// How many rows each present table has, in table order.
    pub fn row_counts(&self) -> Vec<(TableId, u32)> {
        TableId::ALL
            .iter()
            .map(|&t| (t, self.tables.rows(t)))
            .filter(|&(_, n)| n > 0)
            .collect()
    }
}

/// Each column of a row, for generic dumps.
pub fn row_columns(md: &Metadata<'_>, table: TableId, rid: u32) -> Option<[u32; MAX_COLUMNS]> {
    md.tables().columns(table, rid)
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;
    use crate::testutil::MetadataBuilder;

    #[test]
    fn root_and_streams() {
        let mut b = MetadataBuilder::new();
        b.string("Hello");
        let data = b.build();
        let md = Metadata::parse(&data).unwrap();
        assert_eq!(md.version(), "v4.0.30319");
        let names: Vec<&[u8]> = md.streams().iter().map(|s| s.name).collect();
        assert_eq!(names, [&b"#~"[..], b"#Strings", b"#US", b"#GUID", b"#Blob"]);
        assert_eq!(md.strings().get(StringIndex(1)).unwrap(), b"Hello");
        assert!(md.tables_stream().is_some());
    }

    #[test]
    fn damaged_roots() {
        let data = MetadataBuilder::new().build();
        assert_eq!(
            Metadata::parse(b"BSJA").unwrap_err().kind,
            ErrorKind::BadMagic
        );
        for n in 0..data.len() {
            // Every truncation that cuts a stream short fails cleanly.
            let _ = Metadata::parse(&data[..n]);
        }
        // A stream past the end.
        let mut d = data.clone();
        let at = 16 + 12 + 4 + 4; // the first stream header's size
        d[at..at + 4].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        assert!(Metadata::parse(&d).is_err());
        // A stream name with no NUL in 32 bytes.
        let mut d = data.clone();
        let name = 16 + 12 + 4 + 8;
        for b in &mut d[name..name + 40] {
            *b = b'x';
        }
        assert!(Metadata::parse(&d).is_err());
        // No streams at all: empty heaps and tables.
        let mut d = data[..16 + 12].to_vec();
        d.extend_from_slice(&[0, 0, 0, 0]);
        let md = Metadata::parse(&d).unwrap();
        assert_eq!(md.tables().rows(TableId::TypeDef), 0);
        assert_eq!(md.string(StringIndex(0)), "");
    }

    #[test]
    fn lookups() {
        let mut b = MetadataBuilder::new();
        let outer = b.string("Outer");
        let inner = b.string("Inner");
        // Three types: <Module>, Outer (fields 1-2, methods 1), Inner
        // (field 3, methods 2-3) nested in Outer.
        b.row(TableId::TypeDef, &[0, 0, 0, 0, 1, 1]);
        b.row(TableId::TypeDef, &[0, outer, 0, 0, 1, 1]);
        b.row(TableId::TypeDef, &[0, inner, 0, 0, 3, 2]);
        for _ in 0..3 {
            b.row(TableId::Field, &[0, 0, 0]);
            b.row(TableId::MethodDef, &[0, 0, 0, 0, 0, 1]);
        }
        b.row(TableId::NestedClass, &[3, 2]);
        // Custom attributes on Inner, Outer, Inner (unsorted).
        let inner_code = CodedIndexKind::HasCustomAttribute
            .encode(Token::new(TableId::TypeDef, 3))
            .unwrap();
        let outer_code = CodedIndexKind::HasCustomAttribute
            .encode(Token::new(TableId::TypeDef, 2))
            .unwrap();
        b.row(TableId::CustomAttribute, &[inner_code, 0, 0]);
        b.row(TableId::CustomAttribute, &[outer_code, 0, 0]);
        b.row(TableId::CustomAttribute, &[inner_code, 0, 0]);
        let data = b.build();
        let md = Metadata::parse(&data).unwrap();
        assert_eq!(md.type_fields(2).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(md.type_fields(1).count(), 0);
        assert_eq!(md.type_methods(3).collect::<Vec<_>>(), [2, 3]);
        assert_eq!(md.field_owner(3), Some(3));
        assert_eq!(md.method_owner(1), Some(2));
        assert_eq!(md.method_owner(4), None);
        assert_eq!(md.enclosing_type(3), Some(2));
        assert_eq!(md.enclosing_type(2), None);
        assert_eq!(md.nested_types(2), [3]);
        assert_eq!(
            md.custom_attributes(Token::new(TableId::TypeDef, 3)),
            [1, 3]
        );
        assert_eq!(md.custom_attributes(Token::new(TableId::TypeDef, 2)), [2]);
        assert!(
            md.custom_attributes(Token::new(TableId::Field, 0))
                .is_empty()
        );
        assert_eq!(md.find_type("", "Outer"), Some(2));
        assert_eq!(md.find_type("", "Inner"), None);
        assert!(md.has_row(Token::new(TableId::Field, 3)));
        assert!(!md.has_row(Token::new(TableId::Field, 4)));
        assert_eq!(md.row_counts().len(), 5);
    }
}

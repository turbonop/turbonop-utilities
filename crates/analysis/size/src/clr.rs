//! .NET images: a size model and symbols from the CLI metadata (as
//! [`crate::map`] makes them from a linker map), for images that have no
//! PDB to say who contributed what.
//!
//! Each type is a compiland (`Ns.Outer/Inner`), in a library that is its
//! namespace (`[global namespace]` for none), and owns:
//!
//! - its methods' IL bodies (a symbol each, `Ns.Type::Name(int32, string)`)
//!   and its fields' initial data (FieldRVA);
//! - its metadata rows: its TypeDef row, its fields', methods', and
//!   parameters' rows, its properties, events, interfaces, overrides,
//!   generic parameters, layouts, constants, marshalling, P/Invoke, and
//!   the custom attributes on any of them; and its methods' local
//!   signatures (StandAloneSig);
//! - the heap entries (strings, blobs, user strings) that only it refers
//!   to. An entry two owners (or any row no type owns) refer to is the
//!   heap's.
//!
//! What a member owns is a symbol named `[metadata] <member>` (kind
//! [`SymbolKind::ClrMetadata`]), charged to the member's label, as a
//! function's unwind data is. What no type owns is named for what it is:
//! a table's rows (`[CLR metadata: TypeRef]`), a heap's shared entries
//! (`[CLR heap: #Strings]`), the metadata's headers, the CLI header,
//! each embedded resource (`[CLR resource: Name]`), the strong-name
//! signature, v-table fixups, and a ReadyToRun image's header and sections.
//!
//! A ReadyToRun image's native code isn't charged to its methods yet: its
//! sections are labeled as wholes.

use turbonop_hash::Map as HashMap;

use turbonop_clr::tables::{ColumnKind, FieldRVARow, FieldRow, MethodDefRow, TableId};
use turbonop_clr::{CliImage, CodedIndex, Metadata, Token, il, signature};
use turbonop_core::{Error, ErrorKind, Result};
use turbonop_debuginfo::{Compiland, Contribution, Group as DebugGroup};
use turbonop_image::Image;
use turbonop_pe::PeFile;

use crate::pe::{PeInputs, extract_image};
use crate::symbols::{Symbol, SymbolKind};
use crate::{Model, Size};

/// The library of types in no namespace.
pub const GLOBAL_NAMESPACE: &str = "[global namespace]";
/// The library of what no type owns.
pub const CLR_LIBRARY: &str = "[CLR]";

/// Whether `data` is a managed PE image (one with CLI metadata that reads).
pub fn is_managed(data: &[u8]) -> bool {
    PeFile::parse_image(data)
        .ok()
        .is_some_and(|pe| matches!(CliImage::parse(&pe), Ok(Some(_))))
}

/// Who owns a range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Owner {
    Type(u32),
    Method(u32),
    Field(u32),
    /// More than one owner, or none.
    Shared,
}

impl Owner {
    fn merge(self, other: Owner) -> Owner {
        if self == other { self } else { Owner::Shared }
    }
}

/// What a range is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum What {
    /// A method's IL body.
    Code(u32),
    /// A field's initial data.
    Data(u32),
    /// An owner's metadata.
    Meta(Owner),
    /// A structure no type owns: its index in the structure names, and
    /// its kind.
    Structure(u32, SymbolKind),
}

struct Piece {
    rva: u64,
    len: u64,
    what: What,
}

/// Which members belong to which types, from the metadata.
struct Owners<'m, 'd> {
    md: &'m Metadata<'d>,
    param_method: Vec<u32>,
    property_type: Vec<u32>,
    event_type: Vec<u32>,
    local_sig_method: HashMap<u32, Owner>,
}

impl<'m, 'd> Owners<'m, 'd> {
    fn new(md: &'m Metadata<'d>) -> Self {
        let rows = |t| md.tables().rows(t) as usize + 1;
        let mut param_method = vec![0; rows(TableId::Param)];
        for m in 1..=md.tables().rows(TableId::MethodDef) {
            for p in md.method_params(m) {
                if let Some(slot) = param_method.get_mut(p as usize) {
                    *slot = m;
                }
            }
        }
        let mut property_type = vec![0; rows(TableId::Property)];
        let mut event_type = vec![0; rows(TableId::Event)];
        for t in 1..=md.tables().rows(TableId::TypeDef) {
            for p in md.type_properties(t) {
                if let Some(slot) = property_type.get_mut(p as usize) {
                    *slot = t;
                }
            }
            for e in md.type_events(t) {
                if let Some(slot) = event_type.get_mut(e as usize) {
                    *slot = t;
                }
            }
        }
        Self {
            md,
            param_method,
            property_type,
            event_type,
            local_sig_method: HashMap::default(),
        }
    }

    fn of_list(list: &[u32], rid: u32, wrap: fn(u32) -> Owner) -> Owner {
        match list.get(rid as usize) {
            Some(&o) if o != 0 => wrap(o),
            _ => Owner::Shared,
        }
    }

    /// Who owns what `token` names.
    fn of_token(&self, token: Token, depth: u32) -> Owner {
        let md = self.md;
        if !md.has_row(token) || depth > 4 {
            return Owner::Shared;
        }
        let rid = token.rid();
        match token.table() {
            Some(TableId::TypeDef) => Owner::Type(rid),
            Some(TableId::MethodDef) => Owner::Method(rid),
            Some(TableId::Field) => Owner::Field(rid),
            Some(TableId::Param) => Self::of_list(&self.param_method, rid, Owner::Method),
            Some(TableId::Property) => Self::of_list(&self.property_type, rid, Owner::Type),
            Some(TableId::Event) => Self::of_list(&self.event_type, rid, Owner::Type),
            Some(TableId::InterfaceImpl) => md
                .tables()
                .column(TableId::InterfaceImpl, rid, 0)
                .map_or(Owner::Shared, |c| {
                    self.of_token(Token::new(TableId::TypeDef, c), depth + 1)
                }),
            Some(TableId::GenericParam) => md
                .tables()
                .column(TableId::GenericParam, rid, 2)
                .map_or(Owner::Shared, |c| {
                    let owner = CodedIndex::new(turbonop_clr::CodedIndexKind::TypeOrMethodDef, c);
                    self.of_token(owner.token_or_null(), depth + 1)
                }),
            Some(TableId::GenericParamConstraint) => md
                .tables()
                .column(TableId::GenericParamConstraint, rid, 0)
                .map_or(Owner::Shared, |g| {
                    self.of_token(Token::new(TableId::GenericParam, g), depth + 1)
                }),
            _ => Owner::Shared,
        }
    }

    fn of_coded(&self, table: TableId, rid: u32, column: usize) -> Owner {
        let Some(raw) = self.md.tables().column(table, rid, column) else {
            return Owner::Shared;
        };
        let ColumnKind::Coded(kind) = table.columns()[column].kind else {
            return Owner::Shared;
        };
        let c = CodedIndex::new(kind, raw);
        if c.is_null() {
            return Owner::Shared;
        }
        self.of_token(c.token_or_null(), 0)
    }

    fn of_column(&self, table: TableId, rid: u32, column: usize, target: TableId) -> Owner {
        self.md
            .tables()
            .column(table, rid, column)
            .map_or(Owner::Shared, |r| self.of_token(Token::new(target, r), 0))
    }

    /// Who owns row `rid` of `table`.
    fn of_row(&self, table: TableId, rid: u32) -> Owner {
        use TableId as T;
        match table {
            T::TypeDef
            | T::MethodDef
            | T::Field
            | T::Param
            | T::Property
            | T::Event
            | T::InterfaceImpl
            | T::GenericParam
            | T::GenericParamConstraint => self.of_token(Token::new(table, rid), 0),
            T::Constant => self.of_coded(table, rid, 1),
            T::CustomAttribute | T::FieldMarshal | T::ImplMap => {
                self.of_coded(table, rid, if table == T::ImplMap { 1 } else { 0 })
            }
            T::DeclSecurity => self.of_coded(table, rid, 1),
            T::ClassLayout => self.of_column(table, rid, 2, T::TypeDef),
            T::FieldLayout | T::FieldRVA => self.of_column(table, rid, 1, T::Field),
            T::EventMap | T::PropertyMap | T::MethodImpl | T::NestedClass => {
                self.of_column(table, rid, 0, T::TypeDef)
            }
            T::MethodSemantics => self.of_column(table, rid, 1, T::MethodDef),
            T::StandAloneSig => self
                .local_sig_method
                .get(&rid)
                .copied()
                .unwrap_or(Owner::Shared),
            _ => Owner::Shared,
        }
    }
}

/// The names of types, methods, and fields as the symbols give them.
struct Names {
    types: Vec<String>,
    namespaces: Vec<String>,
    methods: Vec<String>,
    fields: Vec<String>,
}

fn names(md: &Metadata<'_>) -> Names {
    let n = |t| md.tables().rows(t) as usize;
    let types: Vec<String> = turbonop_par::map_range(n(TableId::TypeDef), 1024, |i| {
        let rid = i as u32 + 1;
        md.type_def_name(rid)
            .unwrap_or_else(|_| format!("[TypeDef {rid}]"))
    });
    let namespaces: Vec<String> = turbonop_par::map_range(n(TableId::TypeDef), 1024, |i| {
        let mut t = i as u32 + 1;
        for _ in 0..64 {
            match md.enclosing_type(t) {
                Some(o) => t = o,
                None => break,
            }
        }
        let ns = md
            .row::<turbonop_clr::tables::TypeDefRow>(t)
            .map(|r| md.string(r.type_namespace).into_owned())
            .unwrap_or_default();
        if ns.is_empty() {
            GLOBAL_NAMESPACE.into()
        } else {
            ns
        }
    });
    let type_of = |owner: Option<u32>| match owner.and_then(|t| types.get(t as usize - 1)) {
        Some(t) => t.clone(),
        None => "[no type]".into(),
    };
    let methods = turbonop_par::map_range(n(TableId::MethodDef), 512, |i| {
        let rid = i as u32 + 1;
        let Some(m) = md.row::<MethodDefRow>(rid) else {
            return String::new();
        };
        let generics: Vec<String> = md
            .generic_params(Token::new(TableId::MethodDef, rid))
            .into_iter()
            .filter_map(|g| md.row::<turbonop_clr::tables::GenericParamRow>(g))
            .map(|g| md.string(g.name).into_owned())
            .collect();
        let generics = if generics.is_empty() {
            String::new()
        } else {
            format!("<{}>", generics.join(", "))
        };
        let params = md
            .blob(m.signature)
            .and_then(signature::parse_method_sig)
            .map_or_else(
                |_| "?".into(),
                |sig| {
                    let mut list: Vec<String> = sig
                        .params
                        .iter()
                        .map(|p| md.type_sig_name(p).unwrap_or_else(|_| "?".into()))
                        .collect();
                    if let Some(s) = sig.sentinel
                        && s <= list.len()
                    {
                        list.insert(s, "...".into());
                    }
                    list.join(", ")
                },
            );
        format!(
            "{}::{}{generics}({params})",
            type_of(md.method_owner(rid)),
            md.string(m.name)
        )
    });
    let fields = turbonop_par::map_range(n(TableId::Field), 1024, |i| {
        let rid = i as u32 + 1;
        let name = md
            .row::<FieldRow>(rid)
            .map(|f| md.string(f.name).into_owned())
            .unwrap_or_default();
        format!("{}::{name}", type_of(md.field_owner(rid)))
    });
    Names {
        types,
        namespaces,
        methods,
        fields,
    }
}

/// The size of a field's initial data, from its type: a primitive's, or
/// a value type's explicit size (ClassLayout).
fn field_data_size(md: &Metadata<'_>, field: u32) -> Option<u64> {
    let f: FieldRow = md.row(field)?;
    let t = signature::parse_field_sig(md.blob(f.signature).ok()?).ok()?;
    match t {
        signature::TypeSig::Primitive(p) => p.size().map(u64::from),
        signature::TypeSig::ValueType(tok) if tok.table() == Some(TableId::TypeDef) => {
            let l = md.class_layout(tok.rid())?;
            let l: turbonop_clr::tables::ClassLayoutRow = md.row(l)?;
            (l.class_size > 0).then_some(u64::from(l.class_size))
        }
        _ => None,
    }
}

/// The NUL-terminated runs of `#Strings`: their starts, sorted.
fn string_runs(heap: &[u8]) -> Vec<u32> {
    let mut starts = Vec::new();
    let mut at = 0usize;
    while at < heap.len() {
        starts.push(at as u32);
        at += heap[at..]
            .iter()
            .position(|&b| b == 0)
            .map_or(heap.len() - at, |n| n + 1);
    }
    starts
}

fn blob_extent(heap: &[u8], at: u32) -> Option<u64> {
    let rest = heap.get(at as usize..)?;
    let (len, n) = turbonop_clr::reader::decode_compressed_u32(rest)?;
    let end = n as u64 + u64::from(len);
    (end <= rest.len() as u64).then_some(end)
}

struct Builder {
    pieces: Vec<Piece>,
    structures: Vec<String>,
    structure_index: HashMap<String, u32>,
}

impl Builder {
    fn structure(&mut self, name: &str) -> u32 {
        if let Some(&i) = self.structure_index.get(name) {
            return i;
        }
        let i = self.structures.len() as u32;
        self.structures.push(name.to_string());
        self.structure_index.insert(name.to_string(), i);
        i
    }

    fn add(&mut self, rva: u64, len: u64, what: What) {
        if len > 0 {
            self.pieces.push(Piece { rva, len, what });
        }
    }

    fn add_structure(&mut self, rva: u64, len: u64, name: &str, kind: SymbolKind) {
        let s = self.structure(name);
        self.add(rva, len, What::Structure(s, kind));
    }

    /// Labels what isn't yet labeled in `[rva, rva + len)` as `name`.
    fn fill(&mut self, rva: u64, len: u64, name: &str, kind: SymbolKind) {
        let end = rva.saturating_add(len);
        let mut covered: Vec<(u64, u64)> = self
            .pieces
            .iter()
            .filter(|p| p.rva < end && p.rva + p.len > rva)
            .map(|p| (p.rva, p.rva + p.len))
            .collect();
        covered.sort_unstable();
        let mut at = rva;
        let mut gaps = Vec::new();
        for (s, e) in covered {
            if s > at {
                gaps.push((at, s));
            }
            at = at.max(e);
        }
        if at < end {
            gaps.push((at, end));
        }
        for (s, e) in gaps {
            self.add_structure(s, e - s, name, kind);
        }
    }
}

/// The metadata's pieces: every table's rows, and every heap's entries,
/// by owner.
fn metadata_pieces(
    cli: &CliImage<'_>,
    owners: &Owners<'_, '_>,
    user_strings: &HashMap<u32, Owner>,
    b: &mut Builder,
) {
    let md = cli.metadata();
    let base = u64::from(cli.header().metadata.rva);
    let tables = md.tables();
    // Heap entries' owners, from every row that refers to them.
    let strings = md.strings().data();
    let runs = string_runs(strings);
    let mut string_owner: HashMap<u32, Owner> = HashMap::default();
    let mut blob_owner: HashMap<u32, Owner> = HashMap::default();
    if let Some(ts) = md.tables_stream() {
        let ts_at = base + u64::from(ts.offset);
        let first = TableId::ALL
            .iter()
            .map(|&t| tables.layout(t))
            .filter(|l| l.rows > 0)
            .map(|l| l.offset)
            .min()
            .unwrap_or(tables.end());
        b.add_structure(
            ts_at,
            u64::from(first),
            "[CLR metadata: tables header]",
            SymbolKind::ClrMetadata,
        );
        for &t in TableId::ALL {
            let l = *tables.layout(t);
            if l.rows == 0 {
                continue;
            }
            let shared = b.structure(&format!("[CLR metadata: {t}]"));
            let cols = t.columns();
            for rid in 1..=l.rows {
                let owner = owners.of_row(t, rid);
                let at = ts_at + u64::from(l.offset) + u64::from(rid - 1) * u64::from(l.row_size);
                let what = match owner {
                    Owner::Shared => What::Structure(shared, SymbolKind::ClrMetadata),
                    o => What::Meta(o),
                };
                b.add(at, u64::from(l.row_size), what);
                let Some(values) = tables.columns(t, rid) else {
                    continue;
                };
                for (c, col) in cols.iter().enumerate() {
                    let v = values[c];
                    match col.kind {
                        ColumnKind::Str if v != 0 => {
                            let i = runs.partition_point(|&s| s <= v);
                            if let Some(&start) = i.checked_sub(1).and_then(|i| runs.get(i)) {
                                let e = string_owner.entry(start).or_insert(owner);
                                *e = e.merge(owner);
                            }
                        }
                        ColumnKind::Blob if v != 0 => {
                            let e = blob_owner.entry(v).or_insert(owner);
                            *e = e.merge(owner);
                        }
                        _ => {}
                    }
                }
            }
        }
        b.fill(
            ts_at,
            u64::from(ts.size),
            "[CLR metadata: tables padding]",
            SymbolKind::ClrMetadata,
        );
    }
    let heap_at = |name: &[u8]| {
        md.stream(name)
            .map(|s| (base + u64::from(s.offset), u64::from(s.size)))
    };
    if let Some((at, size)) = heap_at(b"#Strings") {
        for (i, &start) in runs.iter().enumerate() {
            if let Some(&o) = string_owner.get(&start)
                && o != Owner::Shared
            {
                let end = runs
                    .get(i + 1)
                    .map_or(strings.len() as u64, |&n| u64::from(n));
                b.add(at + u64::from(start), end - u64::from(start), What::Meta(o));
            }
        }
        b.fill(at, size, "[CLR heap: #Strings]", SymbolKind::ClrMetadata);
    }
    if let Some((at, size)) = heap_at(b"#Blob") {
        let heap = md.blobs().data();
        for (&offset, &o) in &blob_owner {
            if o != Owner::Shared
                && let Some(len) = blob_extent(heap, offset)
            {
                b.add(at + u64::from(offset), len, What::Meta(o));
            }
        }
        b.fill(at, size, "[CLR heap: #Blob]", SymbolKind::ClrMetadata);
    }
    if let Some((at, size)) = heap_at(b"#US") {
        let heap = md.user_strings().data();
        for (&offset, &o) in user_strings {
            if o != Owner::Shared
                && let Some(len) = blob_extent(heap, offset)
            {
                b.add(at + u64::from(offset), len, What::Meta(o));
            }
        }
        b.fill(at, size, "[CLR heap: #US]", SymbolKind::ClrMetadata);
    }
    for s in md.streams() {
        let name = String::from_utf8_lossy(s.name);
        if !matches!(&*name, "#~" | "#-" | "#Strings" | "#Blob" | "#US") {
            b.fill(
                base + u64::from(s.offset),
                u64::from(s.size),
                &format!("[CLR heap: {name}]"),
                SymbolKind::ClrMetadata,
            );
        }
    }
    let size = u64::from(cli.header().metadata.size);
    b.fill(
        base,
        size,
        "[CLR metadata headers]",
        SymbolKind::ClrMetadata,
    );
}

/// The inputs to [`crate::pe::build`] from a managed image's metadata, and
/// its symbols.
pub fn inputs(image: &Image<'_>) -> Result<(PeInputs, Vec<Symbol>)> {
    let pe = image
        .pe()
        .ok_or(Error::new(ErrorKind::Unsupported, 0, "not a PE image"))?;
    let cli = CliImage::parse(pe)?.ok_or(Error::new(
        ErrorKind::Unsupported,
        0,
        "not a managed image (no CLI header)",
    ))?;
    let md = cli.metadata();
    let mut inputs = extract_image(image)?;
    let mut owners = Owners::new(md);
    let names = names(md);
    let mut b = Builder {
        pieces: Vec::new(),
        structures: Vec::new(),
        structure_index: HashMap::default(),
    };

    // IL bodies (on every core), and what they claim: their local
    // signatures and the user strings they load.
    let bodies = cli.method_bodies();
    let claims: Vec<(u32, u32, Vec<u32>)> = turbonop_par::map(&bodies, 256, |(rid, body)| {
        let Ok(body) = body else {
            return (*rid, 0, Vec::new());
        };
        let strings: Vec<u32> = il::decode(body.code)
            .flatten()
            .filter_map(|i| match i.operand {
                il::Operand::Token(t) if t.is_user_string() => Some(t.rid()),
                _ => None,
            })
            .collect();
        let sig = if body.local_var_sig.table() == Some(TableId::StandAloneSig) {
            body.local_var_sig.rid()
        } else {
            0
        };
        (*rid, sig, strings)
    });
    let mut user_strings: HashMap<u32, Owner> = HashMap::default();
    for (rid, sig, strings) in &claims {
        let o = Owner::Method(*rid);
        if *sig != 0 {
            let e = owners.local_sig_method.entry(*sig).or_insert(o);
            *e = e.merge(o);
        }
        for &s in strings {
            let e = user_strings.entry(s).or_insert(o);
            *e = e.merge(o);
        }
    }
    let mut seen_rva = turbonop_hash::Set::default();
    for (rid, body) in &bodies {
        let (Ok(body), Some(m)) = (body, md.row::<MethodDefRow>(*rid)) else {
            continue;
        };
        if seen_rva.insert(m.rva) {
            b.add(u64::from(m.rva), u64::from(body.size), What::Code(*rid));
        }
    }
    // Fields' initial data.
    for (_, r) in md.tables().iter::<FieldRVARow>() {
        if r.rva != 0
            && let Some(size) = field_data_size(md, r.field)
        {
            b.add(u64::from(r.rva), size, What::Data(r.field));
        }
    }
    metadata_pieces(&cli, &owners, &user_strings, &mut b);

    // The CLI header's other parts.
    let h = *cli.header();
    let dir = |d: turbonop_clr::Directory| (u64::from(d.rva), u64::from(d.size));
    let header_dir = pe
        .data_directory(turbonop_pe::consts::directory::COM_DESCRIPTOR)
        .map(|d| (u64::from(d.virtual_address.get()), u64::from(d.size.get())));
    if let Some((at, size)) = header_dir {
        b.add_structure(at, size, "[CLR header]", SymbolKind::PeDirectory);
    }
    for r in cli.embedded_resources().into_iter().flatten() {
        let name = md
            .row::<turbonop_clr::tables::ManifestResourceRow>(r.rid)
            .map(|m| md.string(m.name).into_owned())
            .unwrap_or_default();
        let at = u64::from(h.resources.rva) + u64::from(r.offset);
        b.add_structure(
            at,
            4 + r.data.len() as u64,
            &format!("[CLR resource: {name}]"),
            SymbolKind::ClrResource,
        );
    }
    if !h.resources.is_empty() {
        let (at, size) = dir(h.resources);
        b.fill(at, size, "[CLR resources]", SymbolKind::ClrResource);
    }
    for (d, name) in [
        (h.strong_name_signature, "[CLR strong-name signature]"),
        (h.vtable_fixups, "[CLR v-table fixups]"),
        (
            h.export_address_table_jumps,
            "[CLR export address table jumps]",
        ),
    ] {
        if !d.is_empty() {
            let (at, size) = dir(d);
            b.add_structure(at, size, name, SymbolKind::PeDirectory);
        }
    }
    if let Ok(Some(r2r)) = cli.ready_to_run() {
        let (at, _) = dir(h.managed_native_header);
        b.add_structure(
            at,
            16 + 12 * r2r.sections.len() as u64,
            "[ReadyToRun header]",
            SymbolKind::PeDirectory,
        );
        for s in &r2r.sections {
            let name = turbonop_clr::r2r::section_name(s.ty).map_or_else(
                || format!("[ReadyToRun section {}]", s.ty),
                |n| format!("[ReadyToRun {n}]"),
            );
            b.fill(
                u64::from(s.rva),
                u64::from(s.size),
                &name,
                SymbolKind::PeDirectory,
            );
        }
    } else if !h.managed_native_header.is_empty() {
        let (at, size) = dir(h.managed_native_header);
        b.add_structure(
            at,
            size,
            "[CLR managed native header]",
            SymbolKind::PeDirectory,
        );
    }

    // Groups: the CLI header's directories.
    let mut groups: Vec<DebugGroup> = Vec::new();
    let sections: Vec<(u64, u64, u32)> = inputs
        .sections
        .iter()
        .map(|s| {
            (
                s.addr,
                s.addr + s.vm_size.max(s.raw_size),
                s.characteristics,
            )
        })
        .collect();
    let section_of = |rva: u64| {
        sections
            .iter()
            .find(|s| rva >= s.0 && rva < s.1)
            .map(|s| s.2)
    };
    let mut add_group = |name: &str, (at, size): (u64, u64)| {
        if at == 0 || size == 0 {
            return;
        }
        let Some(characteristics) = section_of(at) else {
            return;
        };
        if groups
            .iter()
            .any(|g| at < g.addr + g.size && g.addr < at + size)
        {
            return;
        }
        groups.push(DebugGroup {
            name: name.into(),
            addr: at,
            size,
            characteristics,
        });
    };
    if let Some(d) = header_dir {
        add_group("[CLR header]", d);
    }
    add_group("[CLR metadata]", dir(h.metadata));
    add_group("[CLR resources]", dir(h.resources));
    add_group("[CLR strong-name signature]", dir(h.strong_name_signature));
    add_group("[CLR v-table fixups]", dir(h.vtable_fixups));
    add_group("[ReadyToRun header]", dir(h.managed_native_header));

    // Compilands: each type, then each structure.
    for (t, name) in names.types.iter().enumerate() {
        inputs.compilands.push(Compiland {
            name: name.clone(),
            library: names.namespaces[t].clone(),
        });
    }
    let types = names.types.len() as u32;
    for s in &b.structures {
        inputs.compilands.push(Compiland {
            name: s.clone(),
            library: CLR_LIBRARY.into(),
        });
    }
    // The pieces, in address order, not overlapping (the first of two
    // that do keeps the bytes), adjacent ones of one owner merged.
    let mut pieces = std::mem::take(&mut b.pieces);
    pieces.sort_by_key(|p| (p.rva, std::cmp::Reverse(p.len)));
    let mut merged: Vec<Piece> = Vec::with_capacity(pieces.len());
    for mut p in pieces {
        if let Some(last) = merged.last_mut() {
            let end = last.rva + last.len;
            if p.rva < end {
                let new_start = end.min(p.rva + p.len);
                p.len = (p.rva + p.len).saturating_sub(new_start);
                p.rva = new_start;
                if p.len == 0 {
                    continue;
                }
            }
            if p.rva == last.rva + last.len && p.what == last.what {
                last.len += p.len;
                continue;
            }
        }
        merged.push(p);
    }
    // What no directory's group covers (IL bodies, field data, ReadyToRun
    // sections): a group of each run of it (gaps of alignment padding
    // inside), so that it's attributed.
    let in_group = |at: u64, groups: &[DebugGroup]| {
        groups.iter().any(|g| at >= g.addr && at < g.addr + g.size)
    };
    let mut runs: Vec<(u64, u64)> = Vec::new();
    for p in &merged {
        if in_group(p.rva, &groups) {
            continue;
        }
        match runs.last_mut() {
            Some(r)
                if p.rva <= r.1 + 16 && !groups.iter().any(|g| g.addr >= r.1 && g.addr < p.rva) =>
            {
                r.1 = p.rva + p.len;
            }
            _ => runs.push((p.rva, p.rva + p.len)),
        }
    }
    for (start, end) in runs {
        if let Some(characteristics) = section_of(start) {
            groups.push(DebugGroup {
                name: "[CLR IL and data]".into(),
                addr: start,
                size: end - start,
                characteristics,
            });
        }
    }
    groups.sort_by_key(|g| g.addr);
    inputs.groups.extend(groups);
    let method_type = |m: u32| md.method_owner(m);
    let field_type = |f: u32| md.field_owner(f);
    let type_compiland = |t: Option<u32>| t.filter(|&t| t >= 1 && t <= types).map(|t| t - 1);
    let mut symbols = Vec::with_capacity(merged.len());
    for p in merged {
        let (compiland, kind, name) = match p.what {
            What::Code(m) => (
                type_compiland(method_type(m)),
                SymbolKind::Function,
                names.methods[m as usize - 1].clone(),
            ),
            What::Data(f) => (
                type_compiland(field_type(f)),
                SymbolKind::Data,
                names.fields[f as usize - 1].clone(),
            ),
            What::Meta(o) => {
                let (t, name) = match o {
                    Owner::Type(t) => (Some(t), names.types[t as usize - 1].clone()),
                    Owner::Method(m) => (method_type(m), names.methods[m as usize - 1].clone()),
                    Owner::Field(f) => (field_type(f), names.fields[f as usize - 1].clone()),
                    Owner::Shared => (None, String::new()),
                };
                (
                    type_compiland(t),
                    SymbolKind::ClrMetadata,
                    format!("[metadata] {name}"),
                )
            }
            What::Structure(s, kind) => (Some(types + s), kind, b.structures[s as usize].clone()),
        };
        let Some(compiland) = compiland else { continue };
        let characteristics = section_of(p.rva).unwrap_or(0);
        inputs.contributions.push(Contribution {
            addr: p.rva,
            size: p.len,
            compiland,
            characteristics,
        });
        symbols.push(Symbol {
            addr: p.rva,
            size: Size::new(p.len, p.len),
            kind,
            folded: false,
            name,
            compiland: Some(compiland),
            file_static: false,
        });
    }
    Ok((inputs, symbols))
}

/// The size model of a managed image from its metadata, and its symbols.
pub fn model(image: &Image<'_>) -> Result<(Model, Vec<Symbol>)> {
    let (inputs, symbols) = inputs(image)?;
    Ok((crate::pe::build(&inputs), symbols))
}

/// The namespace each type compiland's symbols are in (`crates` for a
/// managed image): by compiland index.
pub fn namespace_of(model: &Model, compiland: u32) -> Option<&str> {
    let c = model.compilands.get(compiland as usize)?;
    let lib = &model.libraries.get(c.library as usize)?.name;
    (lib != CLR_LIBRARY).then_some(lib.as_str())
}

/// `-disassemble` for a managed image: the method named `function` (as the
/// symbols name it, `Ns.Type::Name(int32)`; or without its parameters, or
/// as `Type::Name` or `Name`, if just one method is), its IL listed: its
/// header, its exception clauses, and each instruction with tokens named.
pub fn listing(data: &[u8], function: &str) -> std::result::Result<String, String> {
    use std::fmt::Write as _;
    let pe = PeFile::parse_image(data).map_err(|e| e.to_string())?;
    let cli = CliImage::parse(&pe)
        .map_err(|e| e.to_string())?
        .ok_or("not a managed image")?;
    let md = cli.metadata();
    let names = names(md);
    let bare = |n: &str| n.split('(').next().unwrap_or(n).to_string();
    let exact: Vec<u32> = (1..=names.methods.len() as u32)
        .filter(|&m| names.methods[m as usize - 1] == function)
        .collect();
    let found: Vec<u32> = if exact.is_empty() {
        (1..=names.methods.len() as u32)
            .filter(|&m| {
                let b = bare(&names.methods[m as usize - 1]);
                b == function
                    || b.ends_with(&format!(".{function}"))
                    || b.rsplit("::").next() == Some(function)
            })
            .collect()
    } else {
        exact
    };
    let m = match found[..] {
        [m] => m,
        [] => return Err(format!("no method named {function}")),
        _ => {
            let list: Vec<&str> = found
                .iter()
                .take(10)
                .map(|&m| names.methods[m as usize - 1].as_str())
                .collect();
            return Err(format!(
                "{} methods are named {function}: {}{}",
                found.len(),
                list.join("; "),
                if found.len() > 10 { "; ..." } else { "" }
            ));
        }
    };
    let row: MethodDefRow = md.row(m).ok_or("no such method")?;
    let mut out = format!("{}:\n", names.methods[m as usize - 1]);
    let Some(body) = cli.method_body(m).map_err(|e| e.to_string())? else {
        let _ = writeln!(
            out,
            "  // no body (abstract, extern, or runtime-implemented)"
        );
        return Ok(out);
    };
    let _ = writeln!(
        out,
        "  // RVA {:#x}: {} bytes ({} header, {} code), {} header, max stack {}{}",
        row.rva,
        body.size,
        body.header_size,
        body.code.len(),
        if body.fat { "fat" } else { "tiny" },
        body.max_stack,
        if body.init_locals() {
            ", locals zeroed"
        } else {
            ""
        }
    );
    if body.local_var_sig.table() == Some(TableId::StandAloneSig)
        && let Some(s) = md.row::<turbonop_clr::tables::StandAloneSigRow>(body.local_var_sig.rid())
        && let Ok(locals) = md.blob(s.signature).and_then(signature::parse_local_sig)
    {
        let list: Vec<String> = locals
            .iter()
            .map(|t| md.type_sig_name(t).unwrap_or_else(|_| "?".into()))
            .collect();
        let _ = writeln!(out, "  // locals ({})", list.join(", "));
    }
    for c in &body.clauses {
        let what = match c.kind {
            turbonop_clr::body::ClauseKind::Catch(t) => format!("catch {}", md.type_token_name(t)),
            turbonop_clr::body::ClauseKind::Filter(f) => format!("filter IL_{f:04x}"),
            turbonop_clr::body::ClauseKind::Finally => "finally".into(),
            turbonop_clr::body::ClauseKind::Fault => "fault".into(),
            turbonop_clr::body::ClauseKind::Unknown(f) => format!("flags {f:#x}"),
        };
        let _ = writeln!(
            out,
            "  .try IL_{:04x} to IL_{:04x}, handler IL_{:04x} to IL_{:04x}: {what}",
            c.try_offset,
            c.try_offset.saturating_add(c.try_length),
            c.handler_offset,
            c.handler_offset.saturating_add(c.handler_length)
        );
    }
    for i in il::decode(body.code) {
        match i {
            Ok(i) => {
                let _ = writeln!(out, "  IL_{:04x}: {}", i.offset, md.instruction_text(&i));
            }
            Err(il::IlError::Unknown { offset, value }) => {
                let _ = writeln!(out, "  IL_{offset:04x}: (unknown opcode {value:#x})");
            }
            Err(il::IlError::Truncated { offset, .. }) => {
                let _ = writeln!(out, "  IL_{offset:04x}: (truncated)");
            }
        }
    }
    Ok(out)
}

/// The CustomAttribute rows' owners (for tests).
#[cfg(test)]
fn attribute_owners(md: &Metadata<'_>) -> Vec<Owner> {
    let owners = Owners::new(md);
    md.tables()
        .iter::<turbonop_clr::tables::CustomAttributeRow>()
        .map(|(rid, _)| owners.of_row(TableId::CustomAttribute, rid))
        .collect()
}

#[cfg(test)]
#[path = "clr_tests.rs"]
mod tests;

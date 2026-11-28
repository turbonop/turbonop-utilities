//! A file's metadata in tools/clr/ClrDump.cs's text form, from
//! turbonop-clr: every row of every table, method bodies and their IL,
//! signatures, custom attributes, resources, and a portable PDB's tables.
//! (ClrDump.cs says what each line holds; this follows it line for line.)

use std::fmt::Write;

use turbonop_clr::attributes::EnumTypes;
use turbonop_clr::body::ClauseKind;
use turbonop_clr::heaps::{BlobIndex, GuidIndex};
use turbonop_clr::il::{self, IlError};
use turbonop_clr::names::quote;
use turbonop_clr::pdb;
use turbonop_clr::resources;
use turbonop_clr::signature::{self, StandAloneSig};
use turbonop_clr::tables::*;
use turbonop_clr::{CliImage, CodedIndex, Metadata, TableId, Token};

fn tok(t: Token) -> String {
    format!("0x{:08x}", if t.is_null() { 0 } else { t.0 })
}

fn coded(c: CodedIndex) -> String {
    if c.is_null() {
        return tok(Token(0));
    }
    tok(c.token_or_null())
}

fn rid_tok(table: TableId, rid: u32) -> String {
    tok(Token::new(table, rid))
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        let _ = write!(s, "{x:02x}");
    }
    s
}

fn blob_hex(md: &Metadata<'_>, b: BlobIndex) -> String {
    md.blob(b).map(hex).unwrap_or_default()
}

fn guid(md: &Metadata<'_>, g: GuidIndex) -> String {
    match md.guids().get(g) {
        Ok(Some(g)) => g.to_string(),
        _ => "null".into(),
    }
}

fn ranges(rids: impl IntoIterator<Item = u32>) -> String {
    let mut parts = Vec::new();
    let mut run: Option<(u32, u32)> = None;
    for r in rids {
        match run {
            Some((s, l)) if r == l + 1 => run = Some((s, r)),
            Some((s, l)) => {
                parts.push(if s == l {
                    format!("{s}")
                } else {
                    format!("{s}-{l}")
                });
                run = Some((r, r));
            }
            None => run = Some((r, r)),
        }
    }
    if let Some((s, l)) = run {
        parts.push(if s == l {
            format!("{s}")
        } else {
            format!("{s}-{l}")
        });
    }
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(",")
    }
}

fn try_or(r: turbonop_core::Result<String>) -> String {
    r.unwrap_or_else(|_| "undecodable".into())
}

fn bool_text(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

/// SRM's TableIndex names.
fn table_name(t: TableId) -> &'static str {
    match t {
        TableId::FieldRVA => "FieldRva",
        TableId::ENCLog => "EncLog",
        TableId::ENCMap => "EncMap",
        t => t.name(),
    }
}

fn method_sig(md: &Metadata<'_>, b: BlobIndex) -> String {
    try_or(
        md.blob(b)
            .and_then(signature::parse_method_sig)
            .and_then(|m| md.method_sig_name(&m)),
    )
}

fn generic_params(md: &Metadata<'_>, owner: Token, out: &mut String) {
    for g in md.generic_params(owner) {
        let r: GenericParamRow = md.row(g).unwrap();
        let _ = writeln!(
            out,
            "  GenericParam[{g}] index={} flags=0x{:04x} name={} owner={}",
            r.number,
            r.flags,
            md.string(r.name),
            coded(r.owner)
        );
        for c in md.generic_param_constraints(g) {
            let r: GenericParamConstraintRow = md.row(c).unwrap();
            let _ = writeln!(
                out,
                "    GenericParamConstraint[{c}] {}",
                md.type_token_name(r.constraint.token_or_null())
            );
        }
    }
}

fn constant(md: &Metadata<'_>, parent: Token, out: &mut String) {
    if let Some(c) = md.constant(parent) {
        let r: ConstantRow = md.row(c).unwrap();
        let _ = writeln!(
            out,
            "  const Constant[{c}] type=0x{:02x} value={}",
            r.ty,
            blob_hex(md, r.value)
        );
    }
}

fn marshal(md: &Metadata<'_>, parent: Token, out: &mut String) {
    if let Some(m) = md.field_marshal(parent) {
        let r: FieldMarshalRow = md.row(m).unwrap();
        if r.native_type.0 != 0 {
            let _ = writeln!(out, "  marshal {}", blob_hex(md, r.native_type));
        }
    }
}

fn body(cli: &CliImage<'_>, rva: u32, out: &mut String) {
    let md = cli.metadata();
    let Ok(b) = cli.method_body_at(rva) else {
        out.push_str("  body error\n");
        return;
    };
    let _ = writeln!(
        out,
        "  body maxstack={} size={} localsig={} initlocals={}",
        b.max_stack,
        b.size,
        tok(b.local_var_sig),
        bool_text(b.init_locals())
    );
    for c in &b.clauses {
        let what = match c.kind {
            ClauseKind::Catch(t) => format!("catch {}", md.type_token_name(t)),
            ClauseKind::Filter(f) => format!("filter IL_{f:04x}"),
            ClauseKind::Finally => "finally".into(),
            ClauseKind::Fault | ClauseKind::Unknown(_) => "fault".into(),
        };
        let _ = writeln!(
            out,
            "  .try IL_{:04x}+0x{:x} handler IL_{:04x}+0x{:x} {what}",
            c.try_offset, c.try_length, c.handler_offset, c.handler_length
        );
    }
    for i in il::decode(b.code) {
        match i {
            Ok(i) => {
                let _ = writeln!(out, "  IL_{:04x}: {}", i.offset, md.instruction_text(&i));
            }
            Err(IlError::Unknown { offset, value }) => {
                let _ = writeln!(out, "  IL_{offset:04x}: unknown 0x{value:x}");
            }
            Err(IlError::Truncated { offset, opcode }) => {
                match opcode {
                    Some(op) => {
                        let _ = writeln!(out, "  IL_{offset:04x}: {} truncated", op.name);
                    }
                    None => {
                        let _ = writeln!(out, "  IL_{offset:04x}: truncated");
                    }
                }
                return;
            }
        }
    }
}

fn resource(cli: &CliImage<'_>, offset: u32, out: &mut String) {
    let Ok((data, _)) = cli.resource_at(offset) else {
        out.push_str("  data unreadable\n");
        return;
    };
    let _ = writeln!(out, "  data size=0x{:x}", data.len());
    if !resources::is_resources(data) {
        return;
    }
    let set = resources::parse_resources(data).unwrap();
    let mut entries: Vec<_> = set.entries.iter().collect();
    entries.sort_by(|a, b| a.name.encode_utf16().cmp(b.name.encode_utf16()));
    for e in entries {
        let _ = writeln!(
            out,
            "  entry {} type={} data={}",
            quote(&e.name),
            e.type_name,
            hex(e.data)
        );
    }
}

/// A property's getter, setter, and others (by exact semantics, as SRM
/// matches them).
fn accessors(md: &Metadata<'_>, assoc: Token) -> (u32, u32, Vec<u32>) {
    let (mut get, mut set) = (0, 0);
    let mut others = Vec::new();
    for s in md.method_semantics(assoc) {
        let r: MethodSemanticsRow = md.row(s).unwrap();
        match r.semantics {
            0x01 => set = r.method,
            0x02 => get = r.method,
            0x04 => others.push(r.method),
            _ => {}
        }
    }
    (get, set, others)
}

fn event_accessors(md: &Metadata<'_>, assoc: Token) -> (u32, u32, u32, Vec<u32>) {
    let (mut add, mut remove, mut raise) = (0, 0, 0);
    let mut others = Vec::new();
    for s in md.method_semantics(assoc) {
        let r: MethodSemanticsRow = md.row(s).unwrap();
        match r.semantics {
            0x08 => add = r.method,
            0x10 => remove = r.method,
            0x20 => raise = r.method,
            0x04 => others.push(r.method),
            _ => {}
        }
    }
    (add, remove, raise, others)
}

fn join_rids(v: &[u32]) -> String {
    v.iter().map(u32::to_string).collect::<Vec<_>>().join(",")
}

fn method_block(cli: Option<&CliImage<'_>>, md: &Metadata<'_>, rid: u32) -> String {
    let mut out = String::new();
    let m: MethodDefRow = md.row(rid).unwrap();
    let owner = md.method_owner(rid).unwrap_or(0);
    let _ = writeln!(
        out,
        "MethodDef[{rid}] rva=0x{:x} impl=0x{:04x} flags=0x{:04x} name={} sig={} owner={}",
        m.rva,
        m.impl_flags,
        m.flags,
        md.string(m.name),
        method_sig(md, m.signature),
        rid_tok(TableId::TypeDef, owner)
    );
    let _ = writeln!(out, "  params {}", ranges(md.method_params(rid)));
    let token = Token::new(TableId::MethodDef, rid);
    generic_params(md, token, &mut out);
    if let Some(i) = md.impl_map(token) {
        let r: ImplMapRow = md.row(i).unwrap();
        if r.import_scope != 0 {
            let _ = writeln!(
                out,
                "  import flags=0x{:04x} name={} module={}",
                r.mapping_flags,
                md.string(r.import_name),
                rid_tok(TableId::ModuleRef, r.import_scope)
            );
        }
    }
    if m.rva != 0
        && let Some(cli) = cli
    {
        body(cli, m.rva, &mut out);
    }
    out
}

fn sequence_points(md: &Metadata<'_>, rid: u32, out: &mut String) {
    let r: MethodDebugInformationRow = md.row(rid).unwrap();
    let blob = md.blob(r.sequence_points).unwrap_or(&[]);
    let local = if r.sequence_points.0 == 0 {
        0
    } else {
        turbonop_clr::reader::decode_compressed_u32(blob).map_or(0, |(v, _)| v)
    };
    let kickoff = md.kickoff_method(rid).unwrap_or(0);
    let _ = writeln!(
        out,
        "MethodDebugInformation[{rid}] document={} kickoff={} localsig={}",
        rid_tok(TableId::Document, r.document),
        rid_tok(TableId::MethodDef, kickoff),
        rid_tok(TableId::StandAloneSig, local)
    );
    if r.sequence_points.0 == 0 {
        return;
    }
    let sp = pdb::sequence_points(blob, r.document).unwrap();
    for p in sp.points {
        if p.is_hidden() {
            let _ = writeln!(
                out,
                "  sp IL_{:04x} document={} hidden",
                p.il_offset,
                rid_tok(TableId::Document, p.document)
            );
        } else {
            let _ = writeln!(
                out,
                "  sp IL_{:04x} document={} {}:{}-{}:{}",
                p.il_offset,
                rid_tok(TableId::Document, p.document),
                p.start_line,
                p.start_column,
                p.end_line,
                p.end_column
            );
        }
    }
}

fn dump_pdb(md: &Metadata<'_>, out: &mut String) {
    for (rid, d) in md.tables().iter::<DocumentRow>() {
        let _ = writeln!(
            out,
            "Document[{rid}] name={} hashalg={} hash={} language={}",
            pdb::document_name(md, d.name).unwrap(),
            guid(md, d.hash_algorithm),
            blob_hex(md, d.hash),
            guid(md, d.language)
        );
    }
    for rid in 1..=md.tables().rows(TableId::MethodDebugInformation) {
        sequence_points(md, rid, out);
    }
    for (rid, s) in md.tables().iter::<LocalScopeRow>() {
        let _ = writeln!(
            out,
            "LocalScope[{rid}] method={} import={} start=0x{:x} length=0x{:x} variables={} constants={}",
            rid_tok(TableId::MethodDef, s.method),
            rid_tok(TableId::ImportScope, s.import_scope),
            s.start_offset,
            s.length,
            ranges(md.scope_variables(rid)),
            ranges(md.scope_constants(rid))
        );
    }
    for (rid, v) in md.tables().iter::<LocalVariableRow>() {
        let _ = writeln!(
            out,
            "LocalVariable[{rid}] flags=0x{:04x} index={} name={}",
            v.attributes,
            v.index,
            md.string(v.name)
        );
    }
    for (rid, v) in md.tables().iter::<LocalConstantRow>() {
        let _ = writeln!(
            out,
            "LocalConstant[{rid}] name={} sig={}",
            md.string(v.name),
            blob_hex(md, v.signature)
        );
    }
    for (rid, v) in md.tables().iter::<ImportScopeRow>() {
        let _ = writeln!(
            out,
            "ImportScope[{rid}] parent={} imports={}",
            rid_tok(TableId::ImportScope, v.parent),
            blob_hex(md, v.imports)
        );
    }
    for (rid, v) in md.tables().iter::<CustomDebugInformationRow>() {
        let _ = writeln!(
            out,
            "CustomDebugInformation[{rid}] parent={} kind={} value={}",
            coded(v.parent),
            guid(md, v.kind),
            blob_hex(md, v.value)
        );
    }
}

/// The dump of `data`: a PE image's, or a portable PDB's.
pub fn dump(data: &[u8]) -> String {
    let mut out = String::new();
    let cli;
    let md_owned;
    let md: &Metadata<'_> = if data.starts_with(b"BSJB") {
        md_owned = Metadata::parse(data).unwrap();
        cli = None;
        &md_owned
    } else {
        cli = Some(CliImage::parse_bytes(data).unwrap().unwrap());
        let c = cli.as_ref().unwrap();
        let h = c.header();
        let d = |d: turbonop_clr::Directory| format!("0x{:x}+0x{:x}", d.rva, d.size);
        let _ = writeln!(
            out,
            "cor20 runtime={}.{} flags=0x{:08x} entrypoint=0x{:08x}",
            h.major_runtime_version, h.minor_runtime_version, h.flags, h.entry_point
        );
        let _ = writeln!(
            out,
            "cor20 metadata={} resources={} strongname={}",
            d(h.metadata),
            d(h.resources),
            d(h.strong_name_signature)
        );
        let _ = writeln!(
            out,
            "cor20 codemanager={} vtablefixups={} eatjumps={} managednative={}",
            d(h.code_manager_table),
            d(h.vtable_fixups),
            d(h.export_address_table_jumps),
            d(h.managed_native_header)
        );
        c.metadata()
    };
    let cli = cli.as_ref();
    let _ = writeln!(out, "metadata version={}", md.version());
    for (name, heap) in [
        ("String", &b"#Strings"[..]),
        ("UserString", b"#US"),
        ("Guid", b"#GUID"),
        ("Blob", b"#Blob"),
    ] {
        let (offset, mut size) = md.stream(heap).map_or((0, 0), |s| (s.offset, s.size));
        if name == "String" {
            size = md.strings().unpadded_len() as u32;
        }
        let offset = if size == 0 { 0 } else { offset };
        let _ = writeln!(out, "heap {name} offset=0x{offset:x} size=0x{size:x}");
    }
    let tables_at = md.tables_stream().map_or(0, |s| s.offset);
    for (t, rows) in md.row_counts() {
        let l = md.tables().layout(t);
        let _ = writeln!(
            out,
            "table {} rows={rows} rowsize={} offset=0x{:x}",
            table_name(t),
            l.row_size,
            tables_at + l.offset
        );
    }
    if let Some(p) = md.pdb() {
        let _ = writeln!(
            out,
            "pdb id={} entrypoint={}",
            hex(&p.id),
            tok(p.entry_point)
        );
    }
    let enums = EnumTypes::of(md);

    if let Some(m) = md.row::<ModuleRow>(1) {
        let _ = writeln!(
            out,
            "Module[1] generation={} name={} mvid={} encid={} encbaseid={}",
            m.generation,
            md.string(m.name),
            guid(md, m.mvid),
            guid(md, m.enc_id),
            guid(md, m.enc_base_id)
        );
    }
    for (rid, t) in md.tables().iter::<TypeRefRow>() {
        let _ = writeln!(
            out,
            "TypeRef[{rid}] scope={} ns={} name={} full={}",
            coded(t.resolution_scope),
            md.string(t.type_namespace),
            md.string(t.type_name),
            try_or(md.type_ref_name(rid))
        );
    }
    for (rid, t) in md.tables().iter::<TypeDefRow>() {
        let extends = if t.extends.is_null() {
            "-".to_string()
        } else {
            md.type_token_name(t.extends.token_or_null())
        };
        let _ = writeln!(
            out,
            "TypeDef[{rid}] flags=0x{:08x} ns={} name={} full={} extends={extends}",
            t.flags,
            md.string(t.type_namespace),
            md.string(t.type_name),
            try_or(md.type_def_name(rid))
        );
        if let Some(e) = md.enclosing_type(rid) {
            let _ = writeln!(out, "  enclosing {}", rid_tok(TableId::TypeDef, e));
        }
        if let Some(l) = md.class_layout(rid) {
            let l: ClassLayoutRow = md.row(l).unwrap();
            if l.packing_size != 0 || l.class_size != 0 {
                let _ = writeln!(
                    out,
                    "  layout pack={} size={}",
                    l.packing_size, l.class_size
                );
            }
        }
        for i in md.interface_impls(rid) {
            let r: InterfaceImplRow = md.row(i).unwrap();
            let _ = writeln!(
                out,
                "  InterfaceImpl[{i}] {}",
                md.type_token_name(r.interface.token_or_null())
            );
        }
        generic_params(md, Token::new(TableId::TypeDef, rid), &mut out);
        let _ = writeln!(out, "  fields {}", ranges(md.type_fields(rid)));
        let _ = writeln!(out, "  methods {}", ranges(md.type_methods(rid)));
        let _ = writeln!(out, "  properties {}", ranges(md.type_properties(rid)));
        let _ = writeln!(out, "  events {}", ranges(md.type_events(rid)));
        let _ = writeln!(out, "  nested {}", ranges(md.nested_types(rid)));
    }
    for (rid, f) in md.tables().iter::<FieldRow>() {
        let sig = try_or(
            md.blob(f.signature)
                .and_then(signature::parse_field_sig)
                .and_then(|t| md.type_sig_name(&t)),
        );
        let _ = writeln!(
            out,
            "Field[{rid}] flags=0x{:04x} name={} sig={sig} owner={}",
            f.flags,
            md.string(f.name),
            rid_tok(TableId::TypeDef, md.field_owner(rid).unwrap_or(0))
        );
        let token = Token::new(TableId::Field, rid);
        constant(md, token, &mut out);
        if let Some(r) = md.field_rva(rid) {
            let r: FieldRVARow = md.row(r).unwrap();
            if r.rva != 0 {
                let _ = writeln!(out, "  rva 0x{:x}", r.rva);
            }
        }
        if let Some(l) = md.field_layout(rid) {
            let l: FieldLayoutRow = md.row(l).unwrap();
            let _ = writeln!(out, "  offset {}", l.offset.cast_signed());
        }
        marshal(md, token, &mut out);
    }
    // Method bodies, on every core.
    let methods = md.tables().rows(TableId::MethodDef) as usize;
    let blocks = turbonop_par::map_range(methods, 64, |i| method_block(cli, md, i as u32 + 1));
    for b in blocks {
        out.push_str(&b);
    }
    for (rid, p) in md.tables().iter::<ParamRow>() {
        let _ = writeln!(
            out,
            "Param[{rid}] flags=0x{:04x} seq={} name={}",
            p.flags,
            p.sequence,
            md.string(p.name)
        );
        let token = Token::new(TableId::Param, rid);
        constant(md, token, &mut out);
        marshal(md, token, &mut out);
    }
    for (rid, r) in md.tables().iter::<MemberRefRow>() {
        let _ = writeln!(
            out,
            "MemberRef[{rid}] parent={} name={} full={}",
            coded(r.class),
            md.string(r.name),
            try_or(md.member_name(Token::new(TableId::MemberRef, rid)))
        );
    }
    for (rid, c) in md.tables().iter::<ConstantRow>() {
        let _ = writeln!(
            out,
            "Constant[{rid}] type=0x{:02x} parent={} value={}",
            c.ty,
            coded(c.parent),
            blob_hex(md, c.value)
        );
    }
    for (rid, c) in md.tables().iter::<CustomAttributeRow>() {
        let value = match md.custom_attribute_value(rid, &enums) {
            Ok(v) => v.text(),
            Err(_) => format!("undecodable {}", blob_hex(md, c.value)),
        };
        let _ = writeln!(
            out,
            "CustomAttribute[{rid}] parent={} ctor={} value={value}",
            coded(c.parent),
            try_or(md.member_name(c.ty.token_or_null()))
        );
    }
    for (rid, d) in md.tables().iter::<DeclSecurityRow>() {
        let _ = writeln!(
            out,
            "DeclSecurity[{rid}] action={} parent={} set={}",
            d.action,
            coded(d.parent),
            blob_hex(md, d.permission_set)
        );
    }
    for (rid, s) in md.tables().iter::<StandAloneSigRow>() {
        let sig = try_or(
            md.blob(s.signature)
                .and_then(signature::parse_standalone_sig)
                .and_then(|s| match s {
                    StandAloneSig::Locals(l) => l
                        .iter()
                        .map(|t| md.type_sig_name(t))
                        .collect::<turbonop_core::Result<Vec<_>>>()
                        .map(|v| format!("locals ({})", v.join(", "))),
                    StandAloneSig::Method(m) => md.method_sig_name(&m),
                    StandAloneSig::Field(_) => Err(turbonop_core::Error::new(
                        turbonop_core::ErrorKind::Malformed,
                        0,
                        "field",
                    )),
                }),
        );
        let _ = writeln!(out, "StandAloneSig[{rid}] {sig}");
    }
    for (rid, e) in md.tables().iter::<EventRow>() {
        let (add, remove, raise, others) = event_accessors(md, Token::new(TableId::Event, rid));
        let _ = writeln!(
            out,
            "Event[{rid}] flags=0x{:04x} name={} type={} add={} remove={} raise={} others={}",
            e.event_flags,
            md.string(e.name),
            md.type_token_name(e.event_type.token_or_null()),
            rid_tok(TableId::MethodDef, add),
            rid_tok(TableId::MethodDef, remove),
            rid_tok(TableId::MethodDef, raise),
            join_rids(&others)
        );
    }
    for (rid, p) in md.tables().iter::<PropertyRow>() {
        let token = Token::new(TableId::Property, rid);
        let (get, set, others) = accessors(md, token);
        let sig = try_or(
            md.blob(p.ty)
                .and_then(signature::parse_property_sig)
                .and_then(|m| md.method_sig_name(&m)),
        );
        let _ = writeln!(
            out,
            "Property[{rid}] flags=0x{:04x} name={} sig={sig} get={} set={} others={}",
            p.flags,
            md.string(p.name),
            rid_tok(TableId::MethodDef, get),
            rid_tok(TableId::MethodDef, set),
            join_rids(&others)
        );
        constant(md, token, &mut out);
    }
    for (rid, m) in md.tables().iter::<MethodImplRow>() {
        let _ = writeln!(
            out,
            "MethodImpl[{rid}] class={} body={} decl={}",
            rid_tok(TableId::TypeDef, m.class),
            try_or(md.member_name(m.method_body.token_or_null())),
            try_or(md.member_name(m.method_declaration.token_or_null()))
        );
    }
    for (rid, m) in md.tables().iter::<ModuleRefRow>() {
        let _ = writeln!(out, "ModuleRef[{rid}] name={}", md.string(m.name));
    }
    for rid in 1..=md.tables().rows(TableId::TypeSpec) {
        let _ = writeln!(out, "TypeSpec[{rid}] {}", try_or(md.type_spec_name(rid)));
    }
    if let Some(a) = md.assembly() {
        let _ = writeln!(
            out,
            "Assembly[1] hash=0x{:x} version={} flags=0x{:x} key={} name={} culture={}",
            a.hash_alg_id,
            md.assembly_version().unwrap(),
            a.flags,
            blob_hex(md, a.public_key),
            md.string(a.name),
            md.string(a.culture)
        );
    }
    for (rid, a) in md.tables().iter::<AssemblyRefRow>() {
        let _ = writeln!(
            out,
            "AssemblyRef[{rid}] version={}.{}.{}.{} flags=0x{:x} key={} name={} culture={} hash={}",
            a.major_version,
            a.minor_version,
            a.build_number,
            a.revision_number,
            a.flags,
            blob_hex(md, a.public_key_or_token),
            md.string(a.name),
            md.string(a.culture),
            blob_hex(md, a.hash_value)
        );
    }
    for (rid, f) in md.tables().iter::<FileRow>() {
        let _ = writeln!(
            out,
            "File[{rid}] metadata={} name={} hash={}",
            bool_text(f.flags & 1 == 0),
            md.string(f.name),
            blob_hex(md, f.hash_value)
        );
    }
    for (rid, e) in md.tables().iter::<ExportedTypeRow>() {
        let forwarder = e.flags & 0x0020_0000 != 0
            && e.implementation.table() == Some(TableId::AssemblyRef)
            && !e.implementation.is_null();
        let _ = writeln!(
            out,
            "ExportedType[{rid}] flags=0x{:08x} forwarder={} typedef=0x{:x} ns={} name={} impl={}",
            e.flags,
            bool_text(forwarder),
            e.type_def_id,
            md.string(e.type_namespace),
            md.string(e.type_name),
            coded(e.implementation)
        );
    }
    for (rid, r) in md.tables().iter::<ManifestResourceRow>() {
        let _ = writeln!(
            out,
            "ManifestResource[{rid}] offset=0x{:x} flags=0x{:x} name={} impl={}",
            r.offset,
            r.flags,
            md.string(r.name),
            coded(r.implementation)
        );
        if r.implementation.is_null()
            && let Some(cli) = cli
        {
            resource(cli, r.offset, &mut out);
        }
    }
    for rid in 1..=md.tables().rows(TableId::MethodSpec) {
        let _ = writeln!(
            out,
            "MethodSpec[{rid}] {}",
            try_or(md.member_name(Token::new(TableId::MethodSpec, rid)))
        );
    }
    if md.tables().rows(TableId::Document) > 0 || md.pdb().is_some() {
        dump_pdb(md, &mut out);
    }
    for (offset, s) in md.user_strings().iter() {
        let _ = writeln!(
            out,
            "US 0x{offset:x} {}",
            turbonop_clr::names::quote_utf16(s.units())
        );
    }
    out
}

/// The first line where `ours` and `theirs` differ, with context, or
/// `None` if they're the same.
pub fn first_difference(ours: &str, theirs: &str) -> Option<String> {
    let a: Vec<&str> = ours.lines().collect();
    let b: Vec<&str> = theirs.lines().collect();
    let n = a.len().max(b.len());
    for i in 0..n {
        if a.get(i) != b.get(i) {
            let from = i.saturating_sub(3);
            let mut s = format!("line {}:\n", i + 1);
            for j in from..i {
                let _ = writeln!(s, "   {}", a.get(j).unwrap_or(&""));
            }
            let _ = writeln!(s, " - {}", b.get(i).unwrap_or(&"<end>"));
            let _ = writeln!(s, " + {}", a.get(i).unwrap_or(&"<end>"));
            return Some(s);
        }
    }
    None
}

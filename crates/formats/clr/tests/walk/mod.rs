//! Everything turbonop-clr reads from a file, read: every row, name,
//! signature, body, instruction, attribute, resource, and PDB record. For
//! damaged files (which must give errors, never panics) and for real ones
//! (which must give no errors).

// (Each test binary uses some of it.)
#![allow(dead_code)]

use turbonop_clr::attributes::{self, EnumTypes};
use turbonop_clr::il;
use turbonop_clr::pdb;
use turbonop_clr::resources;
use turbonop_clr::signature;
use turbonop_clr::tables::*;
use turbonop_clr::{CliImage, Metadata, TableId, Token};

/// What went wrong (where it shouldn't, in a good file).
#[derive(Default, Debug)]
pub struct Errors(pub Vec<String>);

impl Errors {
    fn note<T>(&mut self, what: &str, r: turbonop_core::Result<T>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                if self.0.len() < 50 {
                    self.0.push(format!("{what}: {e}"));
                }
                None
            }
        }
    }
}

/// Reads all of `md`'s tables, heaps, and what they refer to.
pub fn metadata(md: &Metadata<'_>, e: &mut Errors) {
    metadata_with(md, EnumTypes::of(md), e);
}

/// [`metadata`], with what's known of other assemblies' enums.
pub fn metadata_with(md: &Metadata<'_>, enums: EnumTypes, e: &mut Errors) {
    let _ = md.strings().iter().count();
    let _ = md.blobs().iter().count();
    for (_, s) in md.user_strings().iter() {
        let _ = s.to_string_lossy();
    }
    for &t in TableId::ALL {
        for rid in 1..=md.tables().rows(t) {
            let _ = md.tables().columns(t, rid);
        }
    }
    for rid in 1..=md.tables().rows(TableId::TypeRef) {
        e.note("TypeRef name", md.type_ref_name(rid));
    }
    for (rid, t) in md.tables().iter::<TypeDefRow>() {
        e.note("TypeDef name", md.type_def_name(rid));
        let _ = attributes::serialized_name(md, rid);
        let _ = md.type_token_name(t.extends.token_or_null());
        let _ = md.type_fields(rid).count();
        let _ = md.type_methods(rid).count();
        let _ = md.type_events(rid);
        let _ = md.type_properties(rid);
        let _ = md.nested_types(rid);
        let _ = md.enclosing_type(rid);
        let _ = md.class_layout(rid);
        let _ = md.interface_impls(rid);
        let _ = md.method_impls(rid);
        let token = Token::new(TableId::TypeDef, rid);
        for g in md.generic_params(token) {
            let _ = md.generic_param_constraints(g);
        }
        let _ = md.decl_security(token);
    }
    for (rid, f) in md.tables().iter::<FieldRow>() {
        if let Some(b) = e.note("Field blob", md.blob(f.signature))
            && let Some(t) = e.note("Field signature", signature::parse_field_sig(b))
        {
            e.note("Field type name", md.type_sig_name(&t));
        }
        e.note(
            "Field name",
            md.member_name(Token::new(TableId::Field, rid)),
        );
        let t = Token::new(TableId::Field, rid);
        let _ = (
            md.constant(t),
            md.field_marshal(t),
            md.field_rva(rid),
            md.field_layout(rid),
        );
    }
    for (rid, m) in md.tables().iter::<MethodDefRow>() {
        e.note(
            "MethodDef name",
            md.member_name(Token::new(TableId::MethodDef, rid)),
        );
        let _ = md.method_params(rid).count();
        let _ = md.impl_map(Token::new(TableId::MethodDef, rid));
        let _ = md.semantics_of_method(rid);
        let _ = m;
    }
    for rid in 1..=md.tables().rows(TableId::MemberRef) {
        e.note(
            "MemberRef name",
            md.member_name(Token::new(TableId::MemberRef, rid)),
        );
    }
    for rid in 1..=md.tables().rows(TableId::MethodSpec) {
        e.note(
            "MethodSpec name",
            md.member_name(Token::new(TableId::MethodSpec, rid)),
        );
    }
    for rid in 1..=md.tables().rows(TableId::TypeSpec) {
        e.note("TypeSpec name", md.type_spec_name(rid));
    }
    for (_, s) in md.tables().iter::<StandAloneSigRow>() {
        if let Some(b) = e.note("StandAloneSig blob", md.blob(s.signature)) {
            // (Field-kind ones are C++/CLI's.)
            if let Some(signature::StandAloneSig::Locals(l)) =
                e.note("StandAloneSig", signature::parse_standalone_sig(b))
            {
                for t in l {
                    e.note("local type name", md.type_sig_name(&t));
                }
            }
        }
    }
    for (_, p) in md.tables().iter::<PropertyRow>() {
        if let Some(b) = e.note("Property blob", md.blob(p.ty)) {
            e.note("Property signature", signature::parse_property_sig(b));
        }
    }
    for rid in 1..=md.tables().rows(TableId::CustomAttribute) {
        e.note("custom attribute", md.custom_attribute_value(rid, &enums));
    }
    for (_, d) in md.tables().iter::<DeclSecurityRow>() {
        if let Some(b) = e.note("DeclSecurity blob", md.blob(d.permission_set)) {
            e.note(
                "permission set",
                attributes::decode_permission_set(b, &enums),
            );
        }
    }
    for (_, m) in md.tables().iter::<FieldMarshalRow>() {
        if let Some(b) = e.note("FieldMarshal blob", md.blob(m.native_type)) {
            e.note("marshal descriptor", attributes::decode_marshal(b));
        }
    }
    for rid in 1..=md.tables().rows(TableId::Document) {
        e.note("document name", pdb::document_row_name(md, rid));
    }
    for rid in 1..=md.tables().rows(TableId::MethodDebugInformation) {
        e.note("sequence points", pdb::method_sequence_points(md, rid));
        let _ = md.kickoff_method(rid);
        let _ = md.local_scopes(rid);
    }
    for rid in 1..=md.tables().rows(TableId::LocalScope) {
        let _ = md.scope_variables(rid).count();
        let _ = md.scope_constants(rid).count();
    }
    let _ = (md.assembly_name(), md.assembly_version(), md.module_name());
}

/// Reads all of a CLI image: its metadata, every method body and its
/// instructions (as text), resources, and headers.
pub fn image(cli: &CliImage<'_>, e: &mut Errors) {
    image_with(cli, EnumTypes::of(cli.metadata()), e);
}

/// [`image`], with what's known of other assemblies' enums.
pub fn image_with(cli: &CliImage<'_>, enums: EnumTypes, e: &mut Errors) {
    let md = cli.metadata();
    metadata_with(md, enums, e);
    for (_, b) in cli.method_bodies() {
        let Some(b) = e.note("method body", b) else {
            continue;
        };
        for i in il::decode(b.code) {
            match i {
                Ok(i) => {
                    let _ = md.instruction_text(&i);
                }
                Err(err) => {
                    if e.0.len() < 50 {
                        e.0.push(format!("IL: {err:?}"));
                    }
                }
            }
        }
        for c in &b.clauses {
            if let turbonop_clr::body::ClauseKind::Catch(t) = c.kind {
                let _ = md.type_token_name(t);
            }
        }
    }
    for r in cli.embedded_resources() {
        let Some(r) = e.note("embedded resource", r) else {
            continue;
        };
        if resources::is_resources(r.data)
            && let Some(set) = e.note(".resources", resources::parse_resources(r.data))
        {
            for entry in &set.entries {
                e.note("resource value", entry.value());
            }
        }
    }
    if let Some(s) = cli.strong_name_signature() {
        e.note("strong-name signature", s);
    }
    e.note("ReadyToRun header", cli.ready_to_run());
    let _ = cli.compiler_identifier();
    e.note("v-table fixups", cli.vtable_fixups());
    let _ = cli.entry_point();
}

/// Reads all of `data`: a PE image or a portable PDB.
pub fn file(data: &[u8], e: &mut Errors) {
    if data.starts_with(b"BSJB") {
        if let Some(md) = e.note("metadata", Metadata::parse(data)) {
            metadata(&md, e);
        }
        return;
    }
    if let Some(Some(cli)) = e.note("CLI image", CliImage::parse_bytes(data)) {
        image(&cli, e);
    }
}

/// The machine's framework assemblies: the newest .NET runtime's
/// directory (from `dotnet --list-runtimes`, else the usual places), if
/// there is one.
pub fn framework_dir() -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    let mut best: Option<(Vec<u32>, PathBuf)> = None;
    let mut consider = |version: &str, path: PathBuf| {
        let key: Vec<u32> = version
            .split(['.', '-'])
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        if path.join("System.Private.CoreLib.dll").is_file()
            && best.as_ref().is_none_or(|(k, _)| key > *k)
        {
            best = Some((key, path));
        }
    };
    if let Ok(out) = std::process::Command::new("dotnet")
        .arg("--list-runtimes")
        .output()
    {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some(rest) = line.strip_prefix("Microsoft.NETCore.App ")
                && let Some((version, dir)) = rest.split_once(" [")
            {
                consider(version, Path::new(dir.trim_end_matches(']')).join(version));
            }
        }
    }
    for root in [
        "C:/Program Files/dotnet/shared/Microsoft.NETCore.App",
        "/usr/share/dotnet/shared/Microsoft.NETCore.App",
        "/usr/lib/dotnet/shared/Microsoft.NETCore.App",
        "/usr/local/share/dotnet/shared/Microsoft.NETCore.App",
    ] {
        if let Ok(entries) = std::fs::read_dir(root) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                consider(&name, e.path());
            }
        }
    }
    best.map(|(_, p)| p)
}

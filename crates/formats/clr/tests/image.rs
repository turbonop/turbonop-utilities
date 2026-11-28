//! What the fixtures hold, as turbonop-clr's readers decode it: the values
//! the oracle checks only as bytes (resources, marshalling descriptors,
//! permission sets), and the image-level readers (entry point, strong
//! name, method bodies, sequence points).

use turbonop_global_alloc as _;

use std::path::PathBuf;

use turbonop_clr::attributes::{self, EnumTypes, PermissionSet, native_type};
use turbonop_clr::resources::{self, ResourceValue};
use turbonop_clr::tables::*;
use turbonop_clr::{CliImage, EntryPoint, Metadata, TableId, Token, il, pdb};

fn testdata(name: &str) -> Vec<u8> {
    std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/clr")
            .join(name),
    )
    .unwrap()
}

fn field(md: &Metadata<'_>, ty: &str, name: &str) -> u32 {
    let (ns, n) = ty.rsplit_once('.').unwrap();
    let t = md.find_type(ns, n).unwrap();
    md.type_fields(t)
        .find(|&f| md.string(md.row::<FieldRow>(f).unwrap().name) == name)
        .unwrap()
}

#[test]
fn hello_exe() {
    let data = testdata("Hello.exe");
    let cli = CliImage::parse_bytes(&data).unwrap().unwrap();
    assert!(!cli.pe().optional_header().unwrap().is_64());
    assert_eq!(cli.header().flag_names(), ["IL only", "32-bit required"]);
    let EntryPoint::Token(t) = cli.entry_point() else {
        panic!("{:?}", cli.entry_point());
    };
    let md = cli.metadata();
    assert_eq!(md.member_name(t).unwrap(), "int32 Program::Main(string[])");
    let body = cli.method_body(t.rid()).unwrap().unwrap();
    let text: Vec<String> = il::decode(body.code)
        .map(|i| md.instruction_text(&i.unwrap()))
        .collect();
    assert_eq!(
        text,
        [
            "ldstr \"Hello\"",
            "call void [System.Console]System.Console::WriteLine(string)",
            "ldarg.0",
            "ldlen",
            "conv.i4",
            "ret"
        ]
    );
    assert!(cli.strong_name_signature().is_none());
    assert!(cli.ready_to_run().unwrap().is_none());
    assert!(cli.embedded_resources().is_empty());
}

#[test]
fn sample_resources() {
    let data = testdata("Sample.dll");
    let cli = CliImage::parse_bytes(&data).unwrap().unwrap();
    let md = cli.metadata();
    let embedded: Vec<_> = cli
        .embedded_resources()
        .into_iter()
        .map(Result::unwrap)
        .collect();
    let names: Vec<String> = embedded
        .iter()
        .map(|r| {
            md.string(md.row::<ManifestResourceRow>(r.rid).unwrap().name)
                .into_owned()
        })
        .collect();
    assert_eq!(names, ["Sample.Strings.resources", "Sample.notes.txt"]);
    assert_eq!(
        embedded[1].data,
        b"Notes embedded as a plain manifest resource.\r\n"
    );
    let set = resources::parse_resources(embedded[0].data).unwrap();
    assert_eq!(set.version, 2);
    let value = |name: &str| {
        set.entries
            .iter()
            .find(|e| e.name == name)
            .unwrap()
            .value()
            .unwrap()
    };
    assert_eq!(
        value("greeting"),
        ResourceValue::String("Hello, resources".into())
    );
    assert_eq!(value("nothing"), ResourceValue::Null);
    assert_eq!(value("flag"), ResourceValue::Bool(true));
    assert_eq!(value("letter"), ResourceValue::Char(u16::from(b'q')));
    assert_eq!(value("sbyte"), ResourceValue::Int(-100));
    assert_eq!(value("ushort"), ResourceValue::UInt(65000));
    assert_eq!(value("long"), ResourceValue::Int(-1_234_567_890_123));
    assert_eq!(
        value("ulong"),
        ResourceValue::UInt(18_000_000_000_000_000_000)
    );
    assert_eq!(value("float"), ResourceValue::Single(1.25f32.to_bits()));
    assert_eq!(value("double"), ResourceValue::Double((-2.5f64).to_bits()));
    assert_eq!(value("bytes"), ResourceValue::Bytes(&[1, 2, 3, 4, 5]));
    assert_eq!(value("stream"), ResourceValue::Stream(&[9, 8, 7]));
    assert_eq!(
        value("unicode \u{e9}"),
        ResourceValue::String("\u{2603} snow".into())
    );
    // 1 day, 2:03:04.005.
    let ticks = ((((24 + 2) * 60 + 3) * 60 + 4) * 1000 + 5) * 10_000i64;
    assert_eq!(value("span"), ResourceValue::TimeSpan(ticks));
    // The stored hashes are ResourceWriter's.
    for e in &set.entries {
        assert_eq!(e.hash, resources::name_hash(&e.name), "{}", e.name);
    }
    // The entries' sizes cover the data section.
    let total: u32 = set.entries.iter().map(|e| e.size).sum();
    assert_eq!(
        set.data_section as usize + total as usize,
        embedded[0].data.len()
    );
}

#[test]
fn sample_signing_marshalling_security() {
    let data = testdata("Sample.dll");
    let cli = CliImage::parse_bytes(&data).unwrap().unwrap();
    let md = cli.metadata();
    // Public-signed: room for the signature, zeroed.
    let sig = cli.strong_name_signature().unwrap().unwrap();
    assert_eq!(sig.data.len(), 128);
    assert!(sig.is_zero());
    assert_eq!(md.assembly_version().unwrap().to_string(), "1.2.3.4");
    let marshal = |f: u32| {
        let m = md.field_marshal(Token::new(TableId::Field, f)).unwrap();
        attributes::decode_marshal(
            md.blob(md.row::<FieldMarshalRow>(m).unwrap().native_type)
                .unwrap(),
        )
        .unwrap()
    };
    let m = marshal(field(md, "Sample.Native", "Name"));
    assert_eq!(m.native_type, native_type::LPWSTR);
    let m = marshal(field(md, "Sample.Native", "Four"));
    assert_eq!(
        (m.native_type, m.count, m.element_type),
        (native_type::FIXEDARRAY, Some(4), Some(native_type::I4))
    );
    let m = marshal(field(md, "Sample.Native", "Counted"));
    assert_eq!(
        (m.native_type, m.param_index),
        (native_type::ARRAY, Some(1))
    );
    let m = marshal(field(md, "Sample.Native", "Fixed"));
    assert_eq!(
        (m.native_type, m.count),
        (native_type::FIXEDSYSSTRING, Some(32))
    );
    // [Guard(SecurityAction.Demand, Level = "high")] on Circle.
    let circle = md.find_type("Sample", "Circle").unwrap();
    let ds = md.decl_security(Token::new(TableId::TypeDef, circle));
    assert_eq!(ds.len(), 1);
    let row: DeclSecurityRow = md.row(ds[0]).unwrap();
    assert_eq!(row.action, 2);
    let enums = EnumTypes::of(md);
    let PermissionSet::Attributes(a) =
        attributes::decode_permission_set(md.blob(row.permission_set).unwrap(), &enums).unwrap()
    else {
        panic!()
    };
    assert!(
        a[0].type_name
            .starts_with("Sample.GuardAttribute, Sample, Version=1.2.3.4")
    );
    assert_eq!(a[0].named[0].name, "Level");
    assert_eq!(a[0].named[0].arg.text(), "string(\"high\")");
    // Explicit layout.
    let union = md.find_type("Sample", "Union").unwrap();
    let l: ClassLayoutRow = md.row(md.class_layout(union).unwrap()).unwrap();
    assert_eq!((l.packing_size, l.class_size), (4, 16));
    let f = field(md, "Sample.Union", "L");
    let fl: FieldLayoutRow = md.row(md.field_layout(f).unwrap()).unwrap();
    assert_eq!(fl.offset, 8);
    // Circle.Table's initial data, by its FieldRVA.
    let rvas: Vec<u32> = md
        .tables()
        .iter::<FieldRVARow>()
        .map(|(_, r)| r.rva)
        .collect();
    assert_eq!(rvas.len(), 2);
    for rva in rvas {
        assert!(cli.bytes_from_rva(rva).is_ok());
    }
}

#[test]
fn sample_bodies_in_parallel_match_one_by_one() {
    let data = testdata("Sample.dll");
    let cli = CliImage::parse_bytes(&data).unwrap().unwrap();
    let all = cli.method_bodies();
    let with_rva = cli
        .metadata()
        .tables()
        .iter::<MethodDefRow>()
        .filter(|(_, m)| m.rva != 0)
        .count();
    assert_eq!(all.len(), with_rva);
    for (rid, b) in all {
        assert_eq!(b.unwrap(), cli.method_body(rid).unwrap().unwrap());
    }
    // Exceptions(): a filter, two catches, a finally, a catch-all.
    let md = cli.metadata();
    let code = md.find_type("Sample", "Code").unwrap();
    let m = md
        .type_methods(code)
        .find(|&m| md.string(md.row::<MethodDefRow>(m).unwrap().name) == "Exceptions")
        .unwrap();
    let b = cli.method_body(m).unwrap().unwrap();
    assert!(b.fat);
    let kinds: Vec<String> = b
        .clauses
        .iter()
        .map(|c| match c.kind {
            turbonop_clr::body::ClauseKind::Catch(t) => format!("catch {}", md.type_token_name(t)),
            k => format!("{k:?}"),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "Filter(17)",
            "catch [System.Runtime]System.Exception",
            "Finally",
            "catch [System.Runtime]System.Object"
        ]
    );
}

#[test]
fn sample_pdb() {
    let data = testdata("Sample.pdb");
    let md = Metadata::parse(&data).unwrap();
    assert!(pdb::is_pdb(&md));
    assert_eq!(md.version(), "PDB v1.0");
    let p = md.pdb().unwrap();
    // The type system's row counts are the assembly's.
    let dll = testdata("Sample.dll");
    let cli = CliImage::parse_bytes(&dll).unwrap().unwrap();
    for &t in TableId::ALL.iter().filter(|t| !t.is_debug()) {
        if p.referenced_type_system_tables & (1 << t as u32) != 0 {
            assert_eq!(
                p.type_system_rows[t as usize],
                cli.metadata().tables().rows(t),
                "{t}"
            );
        }
    }
    assert_eq!(
        pdb::document_row_name(&md, 1).unwrap().unwrap(),
        "R:\\tools\\clr\\Sample.cs"
    );
    // Every method with a body has sequence points, inside its code.
    for (rid, m) in cli.metadata().tables().iter::<MethodDefRow>() {
        let sp = pdb::method_sequence_points(&md, rid).unwrap();
        if m.rva == 0 {
            assert!(sp.is_none());
            continue;
        }
        let Some(sp) = sp else { continue };
        let code = u32::try_from(cli.method_body(rid).unwrap().unwrap().code.len()).unwrap();
        assert!(sp.points.iter().all(|p| p.il_offset < code), "{rid}");
    }
    // The async and iterator methods' MoveNext know their kickoff
    // methods.
    assert_eq!(md.tables().rows(TableId::StateMachineMethod), 2);
}

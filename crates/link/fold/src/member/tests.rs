use super::*;

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/coff")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn identities(m: &Member) -> Vec<(Vec<u8>, u128)> {
    let mut v: Vec<_> = m
        .units
        .iter()
        .map(|u| (u.leader.clone(), u.identity))
        .collect();
    v.sort();
    v
}

#[test]
fn an_objects_units_and_types() {
    let m = read(&fixture("objects.obj"));
    assert_eq!(m.kind, Kind::Coff);
    assert!(!m.units.is_empty());
    // /Gy /Z7: each function's unit has its unwind data and .debug$S.
    assert!(m.units.iter().any(|u| u.bytes.get(Category::Unwind) > 0));
    assert!(
        m.units
            .iter()
            .any(|u| u.bytes.get(Category::DebugSymbols) > 0)
    );
    assert!(
        m.units
            .iter()
            .all(|u| u.bytes.get(Category::Code) + u.bytes.get(Category::Data) > 0)
    );
    // Every byte is counted once.
    assert_eq!(m.bytes.total(), m.size);
    match &m.types {
        Types::Records {
            records,
            precomp,
            pch,
            forward,
        } => {
            assert!(records.len() > 10);
            assert!(!precomp && !pch);
            assert_eq!(*forward, 0);
        }
        other => panic!("{other:?}"),
    }
    assert!(m.unknown_sections.is_empty(), "{:?}", m.unknown_sections);
}

#[test]
fn bigobj_doesnt_change_identities_or_types() {
    let small = read(&fixture("objects.obj"));
    let big = read(&fixture("objects-big.obj"));
    assert_eq!(identities(&small), identities(&big));
    assert_eq!(small.environment, big.environment);
    let (Types::Records { records: a, .. }, Types::Records { records: b, .. }) =
        (&small.types, &big.types)
    else {
        panic!("types");
    };
    // The same types, but for the command line (an LF_STRING_ID) and the
    // LF_BUILDINFO that names it.
    let set = |r: &Vec<Hashed>| {
        r.iter()
            .map(|x| x.hash)
            .collect::<std::collections::BTreeSet<_>>()
    };
    let (a, b) = (set(a), set(b));
    assert_eq!(a.difference(&b).count(), 2);
    assert_eq!(b.difference(&a).count(), 2);
}

#[test]
fn zi_objects_name_a_type_server() {
    assert!(matches!(
        read(&fixture("objects-zi.obj")).types,
        Types::TypeServer
    ));
}

#[test]
fn ltcg_and_import_members_pass_through() {
    assert_eq!(read(&fixture("ltcg.obj")).kind, Kind::Ltcg);
    let lib = fixture("imports.lib");
    let archive = turbonop_archive::Archive::parse(&lib).unwrap();
    let kinds: Vec<Kind> = archive.files().map(|m| read(m.data).kind).collect();
    assert!(kinds.contains(&Kind::Import), "{kinds:?}");
}

#[test]
fn garbage_is_unrecognized() {
    assert!(matches!(
        read(b"not an object at all").kind,
        Kind::Unrecognized(_)
    ));
    assert!(matches!(read(&[]).kind, Kind::Unrecognized(_)));
}

#[test]
fn directives_split_on_spaces_outside_quotes() {
    let d = directives(
        b"\xEF\xBB\xBF /FAILIFMISMATCH:\"_MSC_VER=1900\" /DEFAULTLIB:\"a b.lib\"  /INCLUDE:x\0",
    );
    assert_eq!(
        d,
        vec![
            &b"/FAILIFMISMATCH:\"_MSC_VER=1900\""[..],
            &b"/DEFAULTLIB:\"a b.lib\""[..],
            &b"/INCLUDE:x"[..]
        ]
    );
}

fn record(kind: u16, body: &[u8]) -> Vec<u8> {
    let mut r = ((body.len() + 2) as u16).to_le_bytes().to_vec();
    r.extend_from_slice(&kind.to_le_bytes());
    r.extend_from_slice(body);
    r
}

fn stream(records: &[Vec<u8>]) -> Vec<u8> {
    let mut s = 4u32.to_le_bytes().to_vec();
    for r in records {
        s.extend_from_slice(r);
    }
    s
}

fn modifier(of: u32) -> Vec<u8> {
    let mut b = of.to_le_bytes().to_vec();
    b.extend_from_slice(&[1, 0]);
    record(leaf::LF_MODIFIER, &b)
}

#[test]
fn types_hash_by_structure_not_index() {
    // `const T` over `const int`, at different indices in two streams.
    let a = stream(&[modifier(0x74), modifier(0x1000)]);
    let b = stream(&[
        record(leaf::LF_LABEL, &[0, 0]),
        modifier(0x74),
        modifier(0x1001),
    ]);
    let (Types::Records { records: ra, .. }, Types::Records { records: rb, .. }) =
        (types(&a, false), types(&b, false))
    else {
        panic!("records");
    };
    assert_eq!(ra[0].hash, rb[1].hash);
    assert_eq!(ra[1].hash, rb[2].hash);
    assert_ne!(ra[0].hash, ra[1].hash);
    // A record naming one not before it is counted.
    let f = stream(&[modifier(0x1001), modifier(0x74)]);
    assert!(matches!(
        types(&f, false),
        Types::Records { forward: 1, .. }
    ));
}

#[test]
fn types_unreadable_and_type_servers() {
    assert!(matches!(types(&[1, 0, 0, 0], false), Types::Unreadable(_)));
    let overrun = stream(&[vec![0x40, 0, 0x01, 0x10]]);
    assert!(matches!(types(&overrun, false), Types::Unreadable(_)));
    let unknown = stream(&[record(0x1234, &[0; 4])]);
    assert!(matches!(types(&unknown, false), Types::Unreadable(_)));
    let server = stream(&[record(leaf::LF_TYPESERVER2, &[0; 24])]);
    assert!(matches!(types(&server, false), Types::TypeServer));
}

#[test]
fn precomp_users_index_after_the_pch_range() {
    let mut precomp = 0x1000u32.to_le_bytes().to_vec();
    precomp.extend_from_slice(&5u32.to_le_bytes());
    precomp.extend_from_slice(&0xABCDu32.to_le_bytes());
    precomp.extend_from_slice(b"pch.obj\0");
    // 0x1005 is this stream's first record; 0x1002 is in the PCH.
    let s = stream(&[
        record(leaf::LF_PRECOMP, &precomp),
        modifier(0x1002),
        modifier(0x1005),
    ]);
    match types(&s, false) {
        Types::Records {
            records,
            precomp,
            forward,
            ..
        } => {
            assert!(precomp);
            assert_eq!(records.len(), 2);
            assert_eq!(forward, 0);
        }
        other => panic!("{other:?}"),
    }
}

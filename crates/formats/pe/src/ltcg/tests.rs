use super::*;

#[test]
fn readers() {
    let mut r = Reader::new(
        &[
            0x2d, 0x1f, 0xaf, 0x97, 0x01, 0x00, 0x74, 0x80, 0x05, 0x10, 0, 0, 0x80, 0xff, 0xff,
        ],
        0,
    );
    assert_eq!(r.id(), Some(0x1f2d));
    assert_eq!(r.id(), Some(0x97af));
    assert_eq!(r.c32(), Some(0x74));
    assert_eq!(r.c32(), Some(0x1005));
    assert_eq!(r.c16(), Some(0xffff));
    assert_eq!(r.u8(), None);
    // LEB128: a type descriptor's type (0x1000, 0x1FD2, 0x74).
    let mut r = Reader::new(
        &[0x80, 0x20, 0xD2, 0x3F, 0x74, 0xFF, 0xFF, 0xFF, 0xFF, 0x7F],
        0,
    );
    assert_eq!(r.leb(), Some(0x1000));
    assert_eq!(r.leb(), Some(0x1FD2));
    assert_eq!(r.leb(), Some(0x74));
    // (More than 32 bits.)
    assert_eq!(r.leb(), None);
    let mut r = Reader::new(&[3, b'a', b'b', b'c', 0x80, 0x05, 0x00, b'x'], 0);
    assert_eq!(r.lstr(), Some(&b"abc"[..]));
    assert_eq!(r.lstr(), None);
}

#[test]
fn not_ltcg() {
    assert!(!LtcgObject::is_ltcg(&[0; 40]));
    let mut h = [0u8; 40];
    h[2] = 0xff;
    h[3] = 0xff;
    h[4] = 1;
    h[12..28].copy_from_slice(&CLASS_ID);
    assert!(LtcgObject::is_ltcg(&h));
    // (No IL object after it.)
    assert!(LtcgObject::parse(&h[..32]).is_err());
}

/// A `.cil$gl` of a header and the records given, and the directives.
fn table(records: &[&[u8]]) -> Vec<u8> {
    let mut g = alloc::vec![0u8; 30];
    for r in records {
        g.extend_from_slice(r);
    }
    g.extend_from_slice(&[0x0A, 0, 0, 1, 0x04, b'k', b'.', b'l', b'i', b'b', 0]);
    g
}

/// `int f(int a) { return a + 1; }`'s record (from an object cl wrote).
const BODY: &[u8] = &[
    0x0e, 0x6d, 0x0e, 0x00, b'?', b'f', b'@', b'@', b'Y', b'A', b'H', b'H', b'@', b'Z', 0, 0x86,
    0x01, 0x05, 0x04, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 0x80, 0x01, 0x10, 0, 0, 0x82, 0x20, 0,
    0, 0x58, 0x00, 0x07, 0x01, 0, 0, 0, 0x60, 0x10, 0x30, 0x00, 0x01, 0x01, 0x6c, 0x0e, 0x01,
];

/// `int x = 5;`'s.
const DATA: &[u8] = &[
    0x01, 0x00, 0x6c, 0x0e, 0x00, b'?', b'x', b'@', b'@', b'3', b'H', b'A', 0, 0x86, 0x01, 0x00,
    0x02, 0x01, 0x04, 0x80, 0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 0x74, 0x00,
];

#[test]
fn records_walked() {
    let g = table(&[BODY, DATA, b"\x1cstd\0\0\0\x01\0\0\0\0\0\0\0"]);
    let t = Globals::parse(&g).unwrap();
    assert_eq!(t.walked, Ok(()));
    assert_eq!(t.symbols.len(), 2);
    let f = &t.symbols[0];
    assert_eq!(f.name, b"?f@@YAHH@Z");
    assert_eq!(f.end, 30 + BODY.len());
    let il = f.il.unwrap();
    assert_eq!(
        (il.type_index, il.id_index, il.il_offset),
        (0x1001, 0x2082, 0x58)
    );
    assert_eq!((il.weight, il.params), (7, 1));
    assert_eq!(il.body_flags & function::EMITTED, 0x20);
    assert_eq!(il.refs().collect::<Vec<_>>(), [(0x0e6c, RefKind::Direct)]);
    let x = &t.symbols[1];
    assert_eq!(
        x.kind,
        SymbolKind::Data {
            storage: Storage::Defined,
            writable: true,
            size: 4
        }
    );
    assert_eq!(x.type_index, Some(0x74));
    assert!(x.traits.attributes & data::INITIALIZED != 0);
    assert_eq!(t.namespaces[0].name, b"std");
    assert_eq!(t.directives[0].text, b"k.lib");
    // Both roots: listed.
    assert_eq!(t.lib_listed(), [true, true]);
}

#[test]
fn inline_listed_only_if_reached() {
    // An inline function (attributes 0x20, not emitted) that x refers to,
    // and one nothing does.
    let mut inl = BODY.to_vec();
    inl[19] = 0x20; // attributes: inline
    inl[45] = 0x40; // body flags: not emitted
    let mut other = inl.clone();
    other[1] = 0x70; // its id
    let mut data = DATA.to_vec();
    let n = data.len();
    data[n - 1] = 1; // one reference: to f (0x0e6d)
    data.extend_from_slice(&[0x6d, 0x0e, 0x01]);
    let g = table(&[&inl, &data, &other]);
    let t = Globals::parse(&g).unwrap();
    assert_eq!(t.walked, Ok(()));
    assert_eq!(t.lib_listed(), [true, true, false]);
}

#[test]
fn hostile_tables() {
    // Every prefix and every single-byte change of a table: never a panic,
    // and a table cut short is not walked.
    let g = table(&[BODY, DATA]);
    for n in 0..g.len() {
        if let Ok(t) = Globals::parse(&g[..n]) {
            assert!(t.walked.is_err() || n == g.len());
            let _ = t.lib_listed();
        }
    }
    let mut m = g.clone();
    for i in 0..g.len() {
        for v in [0x00, 0x7F, 0x80, 0xFF] {
            m[i] = v;
            if let Ok(t) = Globals::parse(&m) {
                let _ = t.lib_listed();
                let _ = t.il_sizes(100);
            }
        }
        m[i] = g[i];
    }
}

#[test]
fn locals_and_initializers() {
    // `int f(int a) { int loc = a * 3; return loc + 1; }`'s stream.
    let sy: &[u8] = &[
        0x03, 0x01, 0x6e, 0x0e, 0x1f, 0x00, 0x01, 0x01, 0x0d, 0x01, 0x01, 0x01, 0x6c, 0x0e, 0x00,
        b'a', 0, 0x86, 0x01, 0x00, 0x03, 0x04, 0x04, 0x00, 0x01, 0x01, 0, 0, 0, 0, 0, 0, 0, 0x74,
        0x0d, 0x02, 0x01, 0x02, 0x6f, 0x0e, 0x00, b'l', b'o', b'c', 0, 0x86, 0x01, 0x00, 0x01,
        0x04, 0x04, 0x00, 0x01, 0x01, 0, 0, 0, 0, 0, 0, 0, 0x74, 0x06,
    ];
    let l = Locals::parse(sy).unwrap();
    assert_eq!(l.walked, Ok(()));
    assert_eq!(l.functions.len(), 1);
    let names: Vec<&[u8]> = l.functions[0].locals().map(|v| v.name).collect();
    assert_eq!(names, [&b"a"[..], b"loc"]);
    assert!(matches!(
        l.functions[0].locals().next().unwrap().kind,
        LocalKind::Variable {
            class: 3,
            size: 4,
            type_index: 0x74,
            ..
        }
    ));
    assert_eq!(l.functions[0].scope_id(), Some(0x0e6e));
    for n in 0..sy.len() {
        let l = Locals::parse(&sy[..n]).unwrap();
        assert!(l.walked.is_err() || n == 0);
    }
    // `int x[10] = {1}; int* p = &x[2];` and a string.
    let ini: &[u8] = &[
        0x00, 0x6c, 0x0e, 0x00, 0x01, 0x01, 0x04, 0x01, 0x08, 0x24, 0x07, 0x00, 0x6d, 0x0e, 0x00,
        0x02, 0x6c, 0x0e, 0x08, 0x08, 0x07, 0x00, 0x6e, 0x0e, 0x00, 0x03, 0x04, b'a', b'b', b'c',
        0, 0x07, 0x00, 0x6f, 0x0e, 0x00, 0x01, 0x01, 0x02, 0x80, 0xfe, 0xff, 0x01, 0x01, 0x08,
        0xfd, 0x07, 0x0f,
    ];
    let i = Initializers::parse(ini).unwrap();
    assert_eq!(i.walked, Ok(()));
    assert_eq!(i.list.len(), 4);
    assert_eq!(i.list[0].size(), 40);
    assert_eq!(
        i.list[1].items,
        [InitItem::Address {
            target: 0x0e6c,
            addend: 8,
            size: 8
        }]
    );
    assert_eq!(i.of(0x0e6e).unwrap().items, [InitItem::Bytes(b"abc\0")]);
    let v: Vec<_> = i.list[3].items.iter().map(InitItem::integer).collect();
    assert_eq!(v, [Some(0xfffe), Some(u64::MAX - 2)]);
    for n in 0..ini.len() {
        let p = Initializers::parse(&ini[..n]).unwrap();
        assert!(p.walked.is_err() || n == 0);
    }
}

#[test]
fn units() {
    let mut ex = alloc::vec![0u8; 0x58];
    ex[0] = 0x5b;
    ex[1] = 0x58;
    let open = [
        0x4F, 0x1F, 0x80, 0, 0x18, 0x20, 0x80, 0x4F, 0x20, 0x80, 0xFE, 0x00,
    ];
    ex.extend_from_slice(&open);
    ex.extend_from_slice(&[0x47, 0x54, 0x00]);
    ex.extend_from_slice(&open);
    ex.extend_from_slice(&[0x54, 0x00]);
    let u = IlUnits::find(&ex);
    assert_eq!(u.starts, [0x58, 0x58 + 15]);
    assert_eq!(u.size_at(0x58), Some(15));
    assert_eq!(u.range(1), Some((0x58 + 15, 0x58 + 29)));
    assert_eq!(u.size_at(0x59), None);
    assert!(IlUnits::find(&ex[..0x60]).is_empty());
}

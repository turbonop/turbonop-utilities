use super::*;

#[test]
fn maps_of_both_linkers() {
    let link = "\
 Preferred load address is 0000000140000000

  Address         Publics by Value              Rva+Base               Lib:Object

 0000:00000000       __guard_flags              0000000000000000     <absolute>
 0001:00000000       main                       0000000140001000 f   hello.obj
 0001:00000040       printf                     0000000140001040 f i hello.obj
";
    assert_eq!(
        symbols::read_map(link),
        [(0x1000, "main".to_owned()), (0x1040, "printf".to_owned())]
    );
    let turbolink = "00001000  main  hello.obj\n00002000  __xi_a  x.lib(y.obj)\n";
    assert_eq!(
        symbols::read_map(turbolink),
        [(0x1000, "main".to_owned()), (0x2000, "__xi_a".to_owned())]
    );
}

#[test]
fn parts_count_past_their_limit() {
    let mut p = Part::new("x", "X", 2);
    for i in 0..5 {
        p.add(format!("d{i}"));
    }
    p.field("same", 1, 1);
    p.field("other", 1, 2);
    assert_eq!(p.differences, 6);
    assert_eq!(p.lines, ["d0", "d1"]);
    let r = Report {
        kind: "image",
        parts: vec![p, Part::new("y", "Y", 2)],
    };
    assert!(!r.identical());
    let o = Options {
        a: "a".into(),
        b: "b".into(),
        debug: [None, None],
        map: [None, None],
        rows: 2,
        only: Some(vec!["x".into()]),
        json: false,
    };
    let text = r.text(&o);
    assert!(text.contains("X: 6 differences"), "{text}");
    assert!(text.contains("... and 4 more"), "{text}");
    assert!(text.contains("Y: the same"), "{text}");
    assert!(o.wants("x") && !o.wants("y"));
    let j = r.json(&o);
    assert_eq!(j["identical"], false);
    assert_eq!(j["parts"][0]["differences"], 6);
}

#[test]
fn multisets_and_order() {
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    let mut p = Part::new("x", "X", 10);
    pdb::tests_api::multiset(&mut p, "t", &s(&["a", "b", "b"]), &s(&["b", "a", "c"]));
    assert_eq!(p.differences, 2, "{:?}", p.lines);
    let mut q = Part::new("x", "X", 10);
    pdb::tests_api::sequence(&mut q, "t", &s(&["a", "b"]), &s(&["b", "a"]));
    assert_eq!(q.differences, 1);
    assert!(q.lines[0].contains("another order"), "{:?}", q.lines);
}

#[test]
fn types_renumbered_are_the_same() {
    // A has a struct at 0x1000 and a pointer to it at 0x1001; B has an
    // unrelated modifier first, so the struct is 0x1001 and the pointer
    // (to 0x1001) 0x1002.
    let structure = |name: &str| {
        // LF_STRUCTURE: count, properties, field list, derived, vshape,
        // size (numeric), name.
        let mut b = Vec::new();
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(&0x80u16.to_le_bytes()); // forward ref
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.push(0);
        b
    };
    let pointer = |to: u32| {
        let mut b = to.to_le_bytes().to_vec();
        b.extend_from_slice(&0x1000Cu32.to_le_bytes());
        b
    };
    let modifier = || {
        let mut b = 0x74u32.to_le_bytes().to_vec();
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&0u16.to_le_bytes());
        b
    };
    use turbonop_codeview::types::leaf;
    let s = structure("S");
    let pa = pointer(0x1000);
    let m = modifier();
    let pb = pointer(0x1001);
    let a = [(leaf::LF_STRUCTURE, &s[..]), (leaf::LF_POINTER, &pa[..])];
    let b = [
        (leaf::LF_MODIFIER, &m[..]),
        (leaf::LF_STRUCTURE, &s[..]),
        (leaf::LF_POINTER, &pb[..]),
    ];
    let (ca, cb) = (pdb::tests_api::hashes(&a), pdb::tests_api::hashes(&b));
    assert_eq!(ca[0], cb[1], "the struct");
    assert_eq!(ca[1], cb[2], "the pointer to it");
    // A pointer to something else is not the same.
    let pc = pointer(0x1000);
    let c = [
        (leaf::LF_MODIFIER, &m[..]),
        (leaf::LF_STRUCTURE, &s[..]),
        (leaf::LF_POINTER, &pc[..]),
    ];
    assert_ne!(ca[1], pdb::tests_api::hashes(&c)[2]);
}

#[test]
fn pdb_magic() {
    assert!(is_pdb(b"Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0rest"));
    assert!(!is_pdb(b"MZ"));
}

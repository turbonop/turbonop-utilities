use super::*;

#[test]
fn def_files() {
    let def = parse_def(
        "; a comment\n\
         LIBRARY \"mylib\" BASE=0x10000000\n\
         HEAPSIZE 0x20000,0x1000\n\
         VERSION 3.7\n\
         EXPORTS\n\
         \tfirst\n\
         \tsecond = impl2 @5 NONAME\n\
         \tthird=impl3 @ 6 DATA PRIVATE ; trailing comment\n\
         \tfourth @7\n\
         SECTIONS\n\
         \t.shared RWS\n",
    )
    .unwrap();
    assert_eq!(def.name.as_deref(), Some("mylib"));
    assert!(def.library);
    assert_eq!(def.base, Some(0x1000_0000));
    assert_eq!(def.heap, Some((0x20000, Some(0x1000))));
    assert_eq!(def.version, Some((3, 7)));
    let names: Vec<_> = def.exports.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["first", "second", "third", "fourth"]);
    assert_eq!(def.exports[1].internal.as_deref(), Some("impl2"));
    assert_eq!(def.exports[1].ordinal, Some(5));
    assert!(def.exports[1].noname);
    assert_eq!(def.exports[2].internal.as_deref(), Some("impl3"));
    assert!(def.exports[2].data && def.exports[2].private);
    assert_eq!(def.exports[3].ordinal, Some(7));
    assert_eq!(def.sections, [".shared,RWS"]);
    assert!(parse_def("BOGUS x").is_err());
    assert!(parse_def("EXPORTS\n f @0").is_err());
    assert!(parse_def("EXPORTS\n f WHAT").is_err());
}

#[test]
fn ordinals() {
    let e = |name: &str, ord: Option<u16>| Export {
        name: name.into(),
        internal: None,
        ordinal: ord,
        noname: false,
        data: false,
        private: false,
        export_as: None,
    };
    let done = finish(&[
        e("zeta", None),
        e("alpha", Some(3)),
        e("mid", None),
        e("alpha", None),
    ])
    .unwrap();
    let got: Vec<_> = done.iter().map(|x| (x.name.as_str(), x.ordinal)).collect();
    // Duplicates dropped; unnumbered ones after the largest, by name.
    assert_eq!(
        got,
        [("zeta", Some(5)), ("alpha", Some(3)), ("mid", Some(4))]
    );
    assert!(finish(&[e("a", Some(1)), e("b", Some(1))]).is_err());
}

#[test]
fn directory() {
    let mut a = Export::parse("b_func").unwrap();
    a.ordinal = Some(2);
    let mut b = Export::parse("a_func").unwrap();
    b.ordinal = Some(4);
    let mut c = Export::parse("hidden,NONAME").unwrap();
    c.ordinal = Some(3);
    let mut f = Export::parse("fwd=other.Real").unwrap();
    f.ordinal = Some(5);
    let forwarders = [None, None, None, Some("other.Real".to_owned())];
    let d = plan(vec![a, b, c, f], "x.dll", &forwarders);
    assert_eq!((d.base, d.functions), (2, 4));
    assert_eq!(
        d.names.iter().map(|n| n.0.as_str()).collect::<Vec<_>>(),
        ["a_func", "b_func", "fwd"]
    );
    let mut out = vec![0u8; d.size as usize];
    d.write(
        &mut out,
        0x5000,
        "x.dll",
        7,
        |i| 0x1000 + crate::n32(i),
        &forwarders,
    );
    let u32_at = |o: usize| u32::from_le_bytes(out[o..o + 4].try_into().unwrap());
    assert_eq!(u32_at(4), 7);
    assert_eq!(u32_at(16), 2); // base
    assert_eq!(u32_at(20), 4); // functions
    assert_eq!(u32_at(24), 3); // names
    // The address table, by ordinal - base: b_func(2), hidden(3), a_func(4).
    assert_eq!(u32_at(40), 0x1000);
    assert_eq!(u32_at(44), 0x1002);
    assert_eq!(u32_at(48), 0x1001);
    // The forwarder's entry is its string's address.
    let fwd = u32_at(52) - 0x5000;
    assert_eq!(&out[fwd as usize..fwd as usize + 10], b"other.Real");
    // Name ordinals: a_func -> 2, b_func -> 0, fwd -> 3.
    let ords = 40 + 16 + 12;
    let u16_at = |o: usize| u16::from_le_bytes(out[o..o + 2].try_into().unwrap());
    assert_eq!(
        (u16_at(ords), u16_at(ords + 2), u16_at(ords + 4)),
        (2, 0, 3)
    );
    let name0 = u32_at(56) - 0x5000;
    assert_eq!(&out[name0 as usize..name0 as usize + 6], b"a_func");
    assert_eq!(out.len(), d.size as usize);
}

extern crate std;

use super::*;

fn ok(spec: &str) -> (Export<'_>, bool) {
    let o = parse_export_option(spec.as_bytes()).unwrap();
    (o.export, o.extra_delimiter)
}

/// The forms measured with lib.exe 14.52.
#[test]
fn measured_forms() {
    let (e, w) = ok("C,@5,NONAME,DATA");
    assert_eq!(
        (e.name, e.ordinal, e.noname, e.kind, w),
        (&b"C"[..], Some(5), true, ExportKind::Data, false)
    );
    let (e, _) = ok("D=E");
    assert_eq!((e.name, e.internal), (&b"D"[..], Some(&b"E"[..])));
    let (e, _) = ok("A=B=C");
    assert_eq!(e.internal, Some(&b"B=C"[..]));
    let (e, w) = ok("A8=");
    assert_eq!((e.internal, w), (Some(&b""[..]), true));
    assert_eq!(ok("A2,@5NONAME").0.ordinal, Some(5));
    assert!(!ok("A2,@5NONAME").0.noname);
    assert_eq!(ok("A10,@ 5").0.ordinal, Some(5));
    assert!(ok("A4,").1);
    assert!(ok("A,,").1);
    assert!(ok("A,DATA,").1);
    // An empty field ends it.
    let (e, w) = ok("A5,,DATA");
    assert_eq!((e.kind, w), (ExportKind::Code, false));
    assert_eq!(ok("A,,FOO").0.name, b"A");
    assert_eq!(ok("A,@5,,DATA").0.kind, ExportKind::Code);
    assert_eq!(ok("A6,data").0.kind, ExportKind::Data);
    let (e, _) = ok("A11,@6,NONAME,PRIVATE");
    assert!(e.noname && e.private);
    assert_eq!(ok("A,exportas,b").0.export_as, Some(&b"b"[..]));
    assert_eq!(ok("=B7").0.name, b"");
    assert_eq!(ok(" A18").0.name, b" A18");
    assert_eq!(ok("B C").0.name, b"B C");
    for bad in [
        "G,CONSTANT",
        "A3,DATA,@3",
        "A12,PRIVATE,DATA",
        "A13,@7,DATA,NONAME",
        "A14,EXPORTAS",
        "A15,EXPORTAS,X,DATA",
        "A17,constant",
        "A23 , DATA",
        "A24,@8,@9",
        "B,FOO",
        "A,noname",
        "A,EXPORTAS,B,C",
    ] {
        assert_eq!(
            parse_export_option(bad.as_bytes()),
            Err(OptionError::Invalid),
            "{bad}"
        );
    }
    for (bad, text) in [
        ("B,@", ""),
        ("A9,@0", "0"),
        ("A19,@65536", "65536"),
        ("A20,@-1", "-1"),
        ("A21,@0x10", "0x10"),
    ] {
        assert_eq!(
            parse_export_option(bad.as_bytes()),
            Err(OptionError::InvalidOrdinal(text.as_bytes())),
            "{bad}"
        );
    }
}

#[test]
fn directives() {
    let d = b"   /EXPORT:Extra=Plain,@7 /EXPORT:Dd=Data1,DATA /DEFAULTLIB:\"LIBCMT\" -export:\"a b\" /EXPORTS:no ";
    let got = drectve_exports(d);
    assert_eq!(
        got,
        [
            b"Extra=Plain,@7".to_vec(),
            b"Dd=Data1,DATA".to_vec(),
            b"a b".to_vec()
        ]
    );
}

//! Each case was given to `lib.exe /DEF` (MSVC 14.52) as a file of its
//! own; what it accepted is accepted here, with the same meaning, and what
//! it rejected is rejected with the same error.

extern crate std;

use alloc::vec;
use std::string::String;

use super::*;

fn ok(text: &str) -> ModuleDefinition<'_> {
    parse(text.as_bytes()).unwrap_or_else(|e| panic!("{text:?}: {e}"))
}

fn err(text: &str) -> ParseError<'_> {
    parse(text.as_bytes()).expect_err(text)
}

fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn export(text: &str) -> Export<'_> {
    let def = ok(text);
    assert_eq!(def.entries.len(), 1, "{text:?}");
    def.entries[0]
}

fn file(text: &str) -> String {
    s(&ok(text).file_name(b"out"))
}

#[test]
fn module_names() {
    assert_eq!(file("NAME prog\n"), "prog.exe");
    assert_eq!(file("NAME prog.dll\n"), "prog.dll");
    assert_eq!(file("NAME\n"), "out.dll");
    assert_eq!(file("LIBRARY foo\n"), "foo.dll");
    assert_eq!(file("LIBRARY foo.sys\n"), "foo.sys");
    assert_eq!(file("LIBRARY foo.bar.baz\n"), "foo.bar.baz");
    assert_eq!(file("LIBRARY \"foo bar.dll\"\n"), "foo bar.dll");
    assert_eq!(file("LIBRARY \"foo\"\n"), "foo.dll");
    assert_eq!(file("LIBRARY 'foo.dll'\n"), "'foo.dll'");
    assert_eq!(file("LIBRARY \"foo.dll\"x\n"), "foo.dllx");
    assert_eq!(file("LIBRARY \"foo\n"), "foo.dll");
    assert_eq!(file("LIBRARY foo;c\n"), "foo.dll");
    assert_eq!(file("LIBRARY BASE=0x1000\n"), "BASE=0x1000.dll");
    assert_eq!(file("LIBRARY .dll\n"), ".dll");
    assert_eq!(file("LIBRARY foo.\n"), "foo.");
    assert_eq!(file("LIBRARY\n"), "out.dll");
    assert_eq!(file("EXPORTS\n  A\n"), "out.dll");
    assert_eq!(file(""), "out.dll");
    assert_eq!(file("LIBRARY\tfoo\n"), "foo.dll");
    // A drive and directory are dropped, with a warning.
    let def = ok("LIBRARY c:\\x\\y/foo.dll\n");
    assert_eq!(s(&def.file_name(b"out")), "foo.dll");
    assert_eq!(def.warnings, [Warning::DirectoryIgnored { line: 1 }]);
    // The second NAME or LIBRARY is ignored, with a warning.
    let def = ok("NAME a\nLIBRARY b\nEXPORTS\n  A\n  LIBRARY c\n  B\n");
    assert_eq!(s(&def.file_name(b"out")), "a.exe");
    assert_eq!(
        def.warnings,
        [
            Warning::SecondModule { line: 2 },
            Warning::SecondModule { line: 5 },
            Warning::Ignored {
                line: 6,
                statement: b"B"
            }
        ]
    );
}

#[test]
fn module_options() {
    let m = ok("LIBRARY \"a b.dll\" BASE=0x1000\n").module.unwrap();
    assert_eq!((m.kind, m.base), (ModuleKind::Library, Some(0x1000)));
    let base = |t: &str| ok(t).module.unwrap().base;
    assert_eq!(base("LIBRARY foo BASE=4096\n"), Some(4096));
    assert_eq!(base("LIBRARY foo base=010\n"), Some(8));
    assert_eq!(base("LIBRARY foo BASE=09\n"), Some(0));
    assert_eq!(base("LIBRARY foo BASE=1k\n"), Some(1));
    assert_eq!(base("LIBRARY foo BASE=0x1000,0x2000\n"), Some(0x1000));
    assert_eq!(base("LIBRARY foo BASE=0x100000000\n"), Some(0x1_0000_0000));
    assert_eq!(base("LIBRARY foo BASE=0x1000 BASE=0x2000\n"), Some(0x2000));
    assert_eq!(base("LIBRARY foo BASE=0x1000 ; c\n"), Some(0x1000));
    let m = ok("NAME foo WINDOWAPI BASE=0x1000\n").module.unwrap();
    assert!(m.windowapi && !m.windowcompat && m.kind == ModuleKind::Name);
    assert!(ok("NAME foo windowcompat\n").module.unwrap().windowcompat);
    assert!(ok("LIBRARY foo WINDOWAPI\n").module.unwrap().windowapi);
    for bad in [
        "LIBRARY foo bar\n",
        "LIBRARY foo EXPORTS\n",
        "LIBRARY foo BASE = 0x1000\n",
        "LIBRARY foo BASE=\n",
        "LIBRARY foo BASE=0x\n",
        "LIBRARY foo BASE=zz\n",
        "NAME foo NOTWINDOWCOMPAT\n",
        "LIBRARY foo INITINSTANCE\n",
    ] {
        assert!(
            matches!(err(bad), ParseError::Syntax { line: 1, .. }),
            "{bad}"
        );
    }
}

#[test]
fn export_forms() {
    let e = export("EXPORTS\n  Plain\n");
    assert_eq!(
        e,
        Export {
            name: b"Plain",
            internal: None,
            ordinal: None,
            noname: false,
            kind: ExportKind::Code,
            private: false,
            export_as: None,
            line: 2
        }
    );
    let e = export("EXPORTS A=B @5 NONAME DATA PRIVATE\n");
    assert_eq!(
        (
            e.name, e.internal, e.ordinal, e.noname, e.kind, e.private, e.line
        ),
        (
            &b"A"[..],
            Some(&b"B"[..]),
            Some(5),
            true,
            ExportKind::Data,
            true,
            1
        )
    );
    // `=`, with spaces or not; what the internal name may hold.
    for t in [
        "  A=B\n",
        "  A = B\n",
        "  A =B\n",
        "  A= B\n",
        "\tA\t=\tB\n",
    ] {
        let text = std::format!("EXPORTS\n{t}");
        let e = export(&text);
        assert_eq!((e.name, e.internal), (&b"A"[..], Some(&b"B"[..])), "{t:?}");
    }
    assert_eq!(export("EXPORTS\n  A==B\n").internal, Some(&b"=B"[..]));
    assert_eq!(export("EXPORTS\n  A=B==C\n").internal, Some(&b"B==C"[..]));
    assert_eq!(export("EXPORTS\n  A=B=C\n").internal, Some(&b"B=C"[..]));
    assert_eq!(export("EXPORTS\n  A=\n").internal, Some(&b""[..]));
    let e = export("EXPORTS\n  =A\n");
    assert_eq!((e.name, e.internal), (&b""[..], Some(&b"A"[..])));
    // Forwarders.
    let e = export("EXPORTS\n  A=other.dll.Func\n");
    assert_eq!(e.forwarder(), Some((&b"other.dll"[..], &b"Func"[..])));
    assert_eq!(
        export("EXPORTS\n  A=other.#5\n").forwarder(),
        Some((&b"other"[..], &b"#5"[..]))
    );
    assert_eq!(export("EXPORTS\n  A=B\n").forwarder(), None);
    // Names hold what isn't a space, tab, or `=`.
    for name in [
        "\"Quoted\"",
        "'A'",
        "A@5",
        "A,B",
        "?a@@3HA",
        "$x",
        "@f@8",
        "_g@12",
        "\"a",
    ] {
        let text = std::format!("EXPORTS\n  {name}\n");
        assert_eq!(export(&text).name, name.as_bytes());
    }
    assert_eq!(export("EXPORTS\n  \"a;b\"\n").name, b"\"a");
    assert_eq!(
        parse(b"EXPORTS\n  \xe9t\xe9\n").unwrap().entries[0].name,
        b"\xe9t\xe9"
    );
    // Ordinals.
    let ord = |t: &str| {
        let text = std::format!("EXPORTS\n  {t}\n");
        let e = parse(text.as_bytes()).unwrap().entries[0];
        (e.ordinal, e.noname, e.kind, e.private, e.export_as.map(s))
    };
    assert_eq!(ord("A @5").0, Some(5));
    assert_eq!(ord("A @ 5").0, Some(5));
    assert_eq!(ord("A\t@\t3").0, Some(3));
    assert_eq!(ord("A @65535").0, Some(65535));
    // (What follows the digits is ignored: not NONAME.)
    assert_eq!(ord("A @5NONAME").0, Some(5));
    assert!(!ord("A @5NONAME").1);
    assert_eq!(ord("A @7x").0, Some(7));
    assert!(ord("A @5 noname").1);
    assert_eq!(ord("A=B@4 @3").0, Some(3));
    // Keywords in any case, in order.
    assert_eq!(ord("A data").2, ExportKind::Data);
    assert!(ord("A private").3);
    assert_eq!(ord("A exportas B").4.as_deref(), Some("B"));
    assert_eq!(ord("A EXPORTAS \"B\"").4.as_deref(), Some("\"B\""));
    let e = ord("F @9 NONAME EXPORTAS G");
    assert_eq!((e.0, e.4.as_deref()), (Some(9), Some("G")));
    let e = ord("F DATA EXPORTAS G");
    assert_eq!((e.2, e.4.as_deref()), (ExportKind::Data, Some("G")));
    assert_eq!(ord("A DATA PRIVATE").2, ExportKind::Data);
    let def = ok("EXPORTS\n  A @7 CONSTANT\n");
    assert_eq!(def.entries[0].kind, ExportKind::Constant);
    assert_eq!(def.warnings, [Warning::Constant { line: 2 }]);
    // Statement words are names as an export's first word, but the
    // statements' own.
    for name in [
        "DATA",
        "NONAME",
        "PRIVATE",
        "EXPORTAS",
        "DESCRIPTION",
        "CODE",
        "STUB",
        "BASE",
        "exports",
    ] {
        let text = std::format!("EXPORTS\n  First\n  {name}\n  Last\n");
        let def = parse(text.as_bytes()).unwrap();
        let names: Vec<&[u8]> = def.entries.iter().map(|e| e.name).collect();
        assert_eq!(names, [&b"First"[..], name.as_bytes(), b"Last"], "{name}");
    }
}

#[test]
fn export_errors() {
    let cases = [
        ("A NONAME", "NONAME"),
        ("A @5 NONAME NONAME", "NONAME"),
        ("C @7 PRIVATE NONAME", "NONAME"),
        ("B @6 DATA NONAME", "NONAME"),
        ("D DATA @8", "@8"),
        ("A @5 @6", "@6"),
        ("A PRIVATE DATA", "DATA"),
        ("A DATA DATA", "DATA"),
        ("A CONSTANT DATA", "DATA"),
        ("A PRIVATE PRIVATE", "PRIVATE"),
        ("F EXPORTAS G DATA", "DATA"),
        ("F EXPORTAS G PRIVATE", "PRIVATE"),
        ("A PRIVATE EXPORTAS B", "EXPORTAS"),
        ("A @1 NONAME DATA PRIVATE EXPORTAS B", "EXPORTAS"),
        ("F EXPORTAS G EXPORTAS H", "EXPORTAS"),
        ("A EXPORTAS", "EXPORTAS"),
        ("A # x", "#"),
        ("A == B", "B"),
        ("\"A B\"", "B\""),
    ];
    for (t, word) in cases {
        let text = std::format!("LIBRARY foo\nEXPORTS\n  {t}\n");
        match parse(text.as_bytes()) {
            Err(ParseError::Syntax { line, word: w }) => {
                assert_eq!((line, s(w)), (3, String::from(word)), "{t}");
            }
            e => panic!("{t}: {e:?}"),
        }
    }
    for (t, want) in [
        ("A @0", "0"),
        ("A @65536", "65536"),
        ("A @0x10", "0x10"),
        ("A @", ""),
        ("A @-1", "-1"),
    ] {
        let text = std::format!("EXPORTS\n  {t}\n");
        match parse(text.as_bytes()) {
            Err(ParseError::InvalidOrdinal { line, text }) => {
                assert_eq!((line, s(text)), (2, String::from(want)), "{t}");
            }
            e => panic!("{t}: {e:?}"),
        }
    }
}

#[test]
fn duplicates() {
    // The first entry counts; a different one is warned of.
    let def = ok("EXPORTS\n  A\n  A\n  A DATA\n  A=X\n  B\n");
    assert_eq!(def.entries.len(), 5);
    let exports = def.exports();
    assert_eq!(exports.len(), 2);
    assert_eq!(exports[0].kind, ExportKind::Code);
    assert_eq!(
        def.warnings,
        [
            Warning::DuplicateExport {
                line: 4,
                name: b"A"
            },
            Warning::DuplicateExport {
                line: 5,
                name: b"A"
            }
        ]
    );
    // One name, one ordinal twice: fine; two names: an error.
    assert!(ok("EXPORTS\n  A @1\n  A @1\n").warnings.is_empty());
    assert_eq!(
        ok("EXPORTS\n  A @1\n  A @2\n").exports()[0].ordinal,
        Some(1)
    );
    assert_eq!(
        err("EXPORTS\n  A @1\n  B @1\n"),
        ParseError::DuplicateOrdinal { ordinal: 1 }
    );
}

#[test]
fn statements() {
    // Statements in capitals only; the rest are ignored, with warnings.
    let def = ok("library foo\nexports\n  A\n  B data\nDESCRIPTION \"d\"\nSTUB:stub.exe\n");
    assert!(def.module.is_none() && def.entries.is_empty());
    let ignored: Vec<String> = def
        .warnings
        .iter()
        .map(|w| match w {
            Warning::Ignored { statement, .. } => s(statement),
            w => panic!("{w:?}"),
        })
        .collect();
    assert_eq!(
        ignored,
        [
            "library",
            "exports",
            "A",
            "B",
            "DESCRIPTION",
            "STUB:stub.exe"
        ]
    );
    // A statement ends EXPORTS; another EXPORTS continues it.
    let def = ok("EXPORTS A\n  B\nHEAPSIZE 1\n  C\nEXPORTS\n  D\nSECTIONS\n  E\n");
    let names: Vec<&[u8]> = def.entries.iter().map(|e| e.name).collect();
    assert_eq!(names, [&b"A"[..], b"B", b"D"]);
    assert_eq!(def.sections.len(), 1);
    assert_eq!(
        def.warnings,
        [Warning::Ignored {
            line: 4,
            statement: b"C"
        }]
    );
    // Comments, blank lines, carriage returns.
    let def = ok("; c\r\n\r\nLIBRARY foo ; c\r\nEXPORTS ; c\r\n  A ; c\r\n  B@4 ; c\r\n");
    let names: Vec<&[u8]> = def.entries.iter().map(|e| e.name).collect();
    assert_eq!(names, [&b"A"[..], b"B@4"]);
    assert!(def.warnings.is_empty());
    assert_eq!(ok("; only\n"), ModuleDefinition::default());
}

#[test]
fn sections() {
    use SectionAttribute::*;
    let def = ok(concat!(
        "SECTIONS .one READ\n  .two write EXECUTE Shared\n",
        "  .three DISCARDABLE READONLY READWRITE\n",
        "  .four EXECUTEONLY EXECUTEREAD NONSHARED\n  .none\n  .x READ READ\t; c\n"
    ));
    let got: Vec<(String, Vec<SectionAttribute>)> = def
        .sections
        .iter()
        .map(|x| (s(x.name), x.attributes.clone()))
        .collect();
    assert_eq!(
        got,
        [
            (".one".into(), vec![Read]),
            (".two".into(), vec![Write, Execute, Shared]),
            (".three".into(), vec![Discardable, ReadOnly, ReadWrite]),
            (".four".into(), vec![ExecuteOnly, ExecuteRead, NonShared]),
            (".none".into(), vec![]),
            (".x".into(), vec![Read, Read]),
        ]
    );
    for bad in [
        "CLASS c READ",
        "NONSENSE",
        "PRELOAD",
        "NONDISCARDABLE",
        "SINGLE",
        "MULTIPLE",
        "IOPL",
        "NOIOPL",
        "CONFORMING",
        "MOVEABLE",
        "FIXED",
        "LOADONCALL",
        "NONE",
        "ALIGN",
        "E",
        "R W",
        "EXECUTEX",
    ] {
        let text = std::format!("SECTIONS\n  .x {bad}\n");
        assert_eq!(
            parse(text.as_bytes()).unwrap_err(),
            ParseError::Syntax {
                line: 2,
                word: b"SECTIONS"
            },
            "{bad}"
        );
    }
    assert!(matches!(
        err("SECTIONS\n  \".x y\" READ\n"),
        ParseError::Syntax { .. }
    ));
}

#[test]
fn sizes() {
    let heap = |t: &str| ok(t).heap.unwrap();
    let stack = |t: &str| ok(t).stack.unwrap();
    let sz = |r, c| Sizes {
        reserve: r,
        commit: c,
    };
    assert_eq!(
        heap("HEAPSIZE 0x10000,0x1000\n"),
        sz(Some(0x10000), Some(0x1000))
    );
    assert_eq!(heap("HEAPSIZE 100\n"), sz(Some(100), None));
    assert_eq!(heap("HEAPSIZE ,100\n"), sz(None, Some(100)));
    assert_eq!(heap("HEAPSIZE 100,50 ; c\n"), sz(Some(100), Some(50)));
    assert_eq!(
        heap("HEAPSIZE 0x100000000\n"),
        sz(Some(0x1_0000_0000), None)
    );
    assert_eq!(heap("HEAPSIZE 4294967295\n"), sz(Some(4_294_967_295), None));
    assert_eq!(heap("HEAPSIZE 09\n"), sz(Some(0), None));
    assert_eq!(stack("STACKSIZE 1k\n"), sz(Some(1), None));
    assert_eq!(
        stack("STACKSIZE 0X1000,0x10\n"),
        sz(Some(0x1000), Some(0x10))
    );
    assert_eq!(stack("STACKSIZE 010\n"), sz(Some(8), None));
    assert_eq!(stack("STACKSIZE 12 34\n"), sz(Some(12), None));
    assert_eq!(stack("STACKSIZE -1\n"), sz(Some(u64::MAX), None));
    // The rest of the line after the word is ignored: EXPORTS too.
    let def = ok("HEAPSIZE 100 EXPORTS\n  A\n");
    assert!(def.entries.is_empty());
    // The last statement counts.
    assert_eq!(heap("HEAPSIZE 1\nHEAPSIZE 2\n"), sz(Some(2), None));
    for bad in [
        "HEAPSIZE 100,\n",
        "HEAPSIZE\n",
        "HEAPSIZE x\n",
        "STACKSIZE 0x\n",
        "STACKSIZE 4096 , 512\n",
    ] {
        assert!(
            matches!(err(bad), ParseError::Syntax { line: 1, .. }),
            "{bad}"
        );
    }
    assert_eq!(
        err("STACKSIZE 12,34,56\n"),
        ParseError::CommitAboveReserve { line: 1 }
    );
    assert_eq!(
        err("LIBRARY foo\nHEAPSIZE 12,34\n"),
        ParseError::CommitAboveReserve { line: 2 }
    );
}

#[test]
fn versions() {
    let v = |t: &str| ok(t).version.unwrap();
    let ver = |major, minor| Version { major, minor };
    assert_eq!(v("VERSION 3.14\n"), ver(3, 14));
    assert_eq!(v("VERSION 1\n"), ver(1, 0));
    assert_eq!(v("VERSION 1.0\n"), ver(1, 0));
    assert_eq!(v("VERSION  2.5\n"), ver(2, 5));
    assert_eq!(v("VERSION 2.65535\n"), ver(2, 65535));
    assert_eq!(v("VERSION 7\r\n"), ver(7, 0));
    for bad in [
        "VERSION 2.5  \n",
        "VERSION 1.2 ; x\n",
        "VERSION 1.2\tEXPORTS\n",
        "VERSION 65536.1\n",
        "VERSION 2.65536\n",
        "VERSION 1.\n",
        "VERSION 1.2.3\n",
        "VERSION 0x10\n",
        "VERSION\n",
    ] {
        assert_eq!(
            err(bad),
            ParseError::Syntax {
                line: 1,
                word: b"VERSION"
            },
            "{bad:?}"
        );
    }
}

#[test]
fn errors_display() {
    for e in [
        ParseError::Syntax {
            line: 1,
            word: b"X",
        },
        ParseError::InvalidOrdinal {
            line: 1,
            text: b"0",
        },
        ParseError::DuplicateOrdinal { ordinal: 1 },
        ParseError::CommitAboveReserve { line: 1 },
    ] {
        assert!(!std::format!("{e}").is_empty());
    }
}

/// Every prefix of a file parses or fails, never panics.
#[test]
fn truncated() {
    let text = b"LIBRARY \"a b\" BASE=0x10\nSECTIONS .x READ\nHEAPSIZE 1,1\nVERSION 1.2\nEXPORTS\n  A=B @5 NONAME DATA PRIVATE\n  C EXPORTAS D\n";
    for len in 0..text.len() {
        let _ = parse(&text[..len]);
    }
}

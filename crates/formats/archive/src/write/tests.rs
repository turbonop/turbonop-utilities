// Test archives are small: offsets fit in 32 bits.
#![allow(clippy::cast_possible_truncation)]

extern crate std;

use alloc::vec;
use std::string::String;

use super::*;

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

fn sym(name: &[u8], member: u32) -> IndexSymbol<'_> {
    IndexSymbol {
        name: Cow::Borrowed(name),
        member,
    }
}

fn members<'a>(fields: Fields) -> Vec<NewMember<'a>> {
    vec![
        NewMember {
            name: b"a.obj",
            fields,
            data: b"AAA",
        },
        NewMember {
            name: b"a_long_member_name.obj",
            fields,
            data: b"BBBB",
        },
        NewMember {
            name: b"dir/c.obj",
            fields,
            data: b"C",
        },
        NewMember {
            name: b"a_long_member_name.obj",
            fields,
            data: b"DD",
        },
    ]
}

fn index<'a>() -> Index<'a> {
    Index {
        symbols: vec![
            sym(b"zed", 0),
            sym(b"alpha", 0),
            sym(b"mid", 1),
            sym(b"bb", 3),
        ],
        ec_symbols: None,
        member_list: None,
    }
}

#[test]
fn fields() {
    let f = Fields::new(b"123", b"0", b"", b"644").unwrap();
    assert_eq!(
        (f.date(), f.uid(), f.gid(), f.mode()),
        (&b"123"[..], &b"0"[..], &b""[..], &b"644"[..])
    );
    assert!(Fields::new(b"1234567890123", b"", b"", b"").is_none());
    assert!(Fields::new(b"", b"1234567", b"", b"").is_none());
    assert!(Fields::new(b"", b"", b"", b"123456789").is_none());
    let d = Fields::dated(1_795_628_341, 0o100_666);
    assert_eq!((d.date(), d.mode()), (&b"1795628341"[..], &b"100666"[..]));
    assert_eq!(Fields::dated(0, 0).date(), b"0");
    assert_eq!(Fields::BLANK.date(), b"");
}

#[test]
fn styles() {
    let lib = Style::LibExe { time: 1234 };
    let f = |f: Fields| (text(f.date()), text(f.uid()), text(f.gid()), text(f.mode()));
    let t = |d: &str, u: &str, g: &str, m: &str| (d.into(), u.into(), g.into(), m.into());
    assert_eq!(f(lib.table_fields()), t("1234", "", "", "0"));
    assert_eq!(f(lib.long_names_fields()), t("1234", "", "", "0"));
    assert_eq!(f(lib.object_fields(99, false)), t("99", "", "", "100666"));
    assert_eq!(f(lib.object_fields(99, true)), t("99", "", "", "100444"));
    assert_eq!(f(lib.import_fields()), t("1234", "", "", "0"));
    let repro = Style::LibExeBrepro;
    assert_eq!(f(repro.table_fields()), t("-1", "", "", "0"));
    assert_eq!(f(repro.object_fields(99, false)), t("-1", "", "", "100666"));
    assert_eq!(f(repro.import_fields()), t("-1", "", "", "0"));
    let llvm = Style::LlvmLib;
    assert_eq!(f(llvm.table_fields()), t("0", "0", "0", "0"));
    assert_eq!(f(llvm.long_names_fields()), t("", "", "", ""));
    assert_eq!(f(llvm.object_fields(99, true)), t("0", "0", "0", "644"));
    assert_eq!(f(llvm.import_fields()), t("0", "0", "0", "644"));
    assert!(llvm.conventions().tables_padded_inside);
    assert!(!lib.conventions().tables_padded_inside);
    assert_eq!(lib.conventions().padding, b'\n');
}

/// An archive written as `lib.exe` writes one, read back field by field.
#[test]
fn lib_exe_archive() {
    let style = Style::LibExe { time: 1234 };
    let m = members(style.object_fields(77, false));
    let out = write(&m, &index(), style, SizeLimit::FourGiB).unwrap();
    let ar = Archive::parse(&out).unwrap();
    let all: Vec<Member<'_>> = ar.members().collect();
    let names: Vec<String> = all.iter().map(|m| text(&m.name)).collect();
    assert_eq!(
        names,
        [
            "/",
            "/",
            "//",
            "a.obj",
            "a_long_member_name.obj",
            "dir/c.obj",
            "a_long_member_name.obj"
        ]
    );
    // The headers: `name/` or `/offset`, each long name once.
    let header = |i: usize| text(&out[all[i].header_offset as usize..][..60]);
    let expect = |name: &str, date: &str, mode: &str, size: usize| {
        std::format!(
            "{name:<16}{date:<12}{:<6}{:<6}{mode:<8}{size:<10}`\n",
            "",
            ""
        )
    };
    assert_eq!(header(0), expect("/", "1234", "0", 37));
    assert_eq!(header(3), expect("a.obj/", "77", "100666", 3));
    assert!(header(4).starts_with("/0              77"));
    assert!(header(5).starts_with("/23 "));
    assert!(header(6).starts_with("/0 "));
    assert_eq!(all[2].data, b"a_long_member_name.obj\0dir/c.obj\0");
    // Odd sizes are padded with `\n` after (the first linker member's 37
    // bytes too).
    assert_eq!(all[0].data.len(), 37);
    assert_eq!(out[all[0].data_offset as usize + 37], b'\n');
    assert_eq!(out[all[3].data_offset as usize + 3], b'\n');
    assert_eq!(out.len() % 2, 0);
    // The first linker member in the index's order, the second sorted.
    let first = ar.first_linker_member().unwrap();
    let first: Vec<(&[u8], u64)> = first.iter().map(|s| (s.name, s.member)).collect();
    let at = |i: usize| all[i + 3].header_offset;
    assert_eq!(
        first,
        [
            (&b"zed"[..], at(0)),
            (b"alpha", at(0)),
            (b"mid", at(1)),
            (b"bb", at(3))
        ]
    );
    let (list, second) = ar.second_linker_member().unwrap();
    assert_eq!(list, [at(0), at(1), at(2), at(3)]);
    let second: Vec<&[u8]> = second.iter().map(|s| s.name).collect();
    assert_eq!(second, [&b"alpha"[..], b"bb", b"mid", b"zed"]);
    assert_eq!(ar.symbols()[1].member, at(3));
    // Read back, it is what was written.
    let c = Contents::read(&ar).unwrap();
    assert_eq!(c.members, m);
    assert_eq!(c.index, index());
    assert_eq!(c.style, Some(style));
    assert_eq!(c.conventions, style.conventions());
    assert_eq!(c.write(SizeLimit::FourGiB).unwrap(), out);
}

/// `llvm-lib` pads the tables inside their sizes: NUL for the linker
/// members, `\n` for the long names.
#[test]
fn llvm_lib_archive() {
    let style = Style::LlvmLib;
    let m = members(style.object_fields(0, false));
    let out = write(&m, &index(), style, SizeLimit::FourGiB).unwrap();
    let ar = Archive::parse(&out).unwrap();
    let all: Vec<Member<'_>> = ar.members().collect();
    assert_eq!(all[0].data.len(), 38);
    assert!(all[0].data.ends_with(b"bb\0\0"));
    let header = |i: usize| text(&out[all[i].header_offset as usize..][..60]);
    let expect = |name: &str, f: &str, size: usize| std::format!("{name:<16}{f:<32}{size:<10}`\n");
    assert_eq!(header(0), expect("/", "0           0     0     0", 38));
    assert_eq!(header(2), expect("//", "", 34));
    assert_eq!(all[2].data, b"a_long_member_name.obj\0dir/c.obj\0\n");
    let c = Contents::read(&ar).unwrap();
    assert!(c.conventions.tables_padded_inside);
    assert_eq!(c.style, Some(style));
    assert_eq!(c.write(SizeLimit::FourGiB).unwrap(), out);
}

/// Microsoft's SDK libraries' conventions: dates `4294967295`, NUL
/// padding.
#[test]
fn other_conventions() {
    let fields = Fields::new(b"4294967295", b"", b"", b"0").unwrap();
    let conventions = Conventions {
        table_fields: fields,
        long_names_fields: fields,
        tables_padded_inside: false,
        padding: 0,
    };
    let m = members(fields);
    let out = write(&m, &index(), conventions, SizeLimit::FourGiB).unwrap();
    let ar = Archive::parse(&out).unwrap();
    let all: Vec<Member<'_>> = ar.members().collect();
    assert_eq!(out[all[0].data_offset as usize + all[0].data.len()], 0);
    assert_eq!(out[all[3].data_offset as usize + 3], 0);
    let c = Contents::read(&ar).unwrap();
    assert_eq!(c.conventions, conventions);
    assert_eq!(
        c.style,
        Some(Style::LibExe {
            time: 4_294_967_295
        })
    );
    assert_eq!(c.write(SizeLimit::FourGiB).unwrap(), out);
}

/// lib.exe's import libraries list the members out of archive order in
/// the second linker member; some of Microsoft's have holes (offset 0).
#[test]
fn member_list() {
    let style = Style::LibExeBrepro;
    let m = members(style.import_fields());
    let mut index = index();
    index.member_list = Some(vec![Some(2), None, Some(0), Some(3), Some(1)]);
    let out = write(&m, &index, style, SizeLimit::FourGiB).unwrap();
    let ar = Archive::parse(&out).unwrap();
    let files: Vec<Member<'_>> = ar.files().collect();
    let (list, second) = ar.second_linker_member().unwrap();
    let at = |i: usize| files[i].header_offset;
    assert_eq!(list, [at(2), 0, at(0), at(3), at(1)]);
    // Each symbol names its member's place in the list: alpha (member 0,
    // third), b (member 3, fourth), mid (member 1, fifth), zed (third).
    let data = ar.members().nth(1).unwrap().data;
    let numbers: Vec<u16> = data[4 + 5 * 4 + 4..][..8]
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    assert_eq!(numbers, [3, 4, 5, 3]);
    assert_eq!(second[1].member, at(3));
    let c = Contents::read(&ar).unwrap();
    assert_eq!(c.index, index);
    assert_eq!(c.write(SizeLimit::FourGiB).unwrap(), out);
    // A symbol of a member the list leaves out has nowhere to point.
    index.member_list = Some(vec![Some(0), Some(1), Some(2)]);
    assert_eq!(
        write(&m, &index, style, SizeLimit::FourGiB),
        Err(WriteError::NoSuchMember { symbol: 3 })
    );
    index.member_list = Some(vec![Some(9)]);
    assert_eq!(
        write(&m, &index, style, SizeLimit::FourGiB),
        Err(WriteError::NoSuchListedMember { position: 0 })
    );
}

/// `/<ECSYMBOLS>/` comes after `//`, sorted, numbering members as the
/// second linker member does.
#[test]
fn ec_symbols() {
    let style = Style::LibExeBrepro;
    let m = members(style.object_fields(0, false));
    let mut index = index();
    index.ec_symbols = Some(vec![sym(b"#f", 2), sym(b"$thunk", 1), sym(b"", 3)]);
    let out = write(&m, &index, style, SizeLimit::FourGiB).unwrap();
    let ar = Archive::parse(&out).unwrap();
    let kinds: Vec<MemberKind> = ar.members().take(4).map(|m| m.kind).collect();
    assert_eq!(
        kinds,
        [
            MemberKind::SymbolTable,
            MemberKind::SymbolTable,
            MemberKind::LongNames,
            MemberKind::EcSymbolTable
        ]
    );
    let files: Vec<Member<'_>> = ar.files().collect();
    let ec: Vec<(&[u8], u64)> = ar
        .ec_symbols()
        .unwrap()
        .iter()
        .map(|s| (s.name, s.member))
        .collect();
    assert_eq!(
        ec,
        [
            (&b""[..], files[3].header_offset),
            (b"#f", files[2].header_offset),
            (b"$thunk", files[1].header_offset)
        ]
    );
    let c = Contents::read(&ar).unwrap();
    assert_eq!(c.write(SizeLimit::FourGiB).unwrap(), out);
    // Without long names, it follows the second linker member.
    let short: Vec<NewMember<'_>> = m
        .iter()
        .map(|m| NewMember {
            name: b"x.obj",
            ..*m
        })
        .collect();
    let out = write(&short, &index, style, SizeLimit::FourGiB).unwrap();
    let ar = Archive::parse(&out).unwrap();
    assert_eq!(ar.members().nth(2).unwrap().kind, MemberKind::EcSymbolTable);
}

/// `lib.exe` sorts each member's symbols and lets a later member's
/// supersede a common symbol; `llvm-lib` keeps the order, and the first.
#[test]
fn building_the_index() {
    let common = |n: &'static [u8]| MemberSymbol {
        name: Cow::Borrowed(n),
        common: true,
    };
    let d = |n: &'static [u8]| MemberSymbol::defined(n);
    let members = || {
        vec![
            (
                0,
                MemberSymbols {
                    symbols: vec![d(b"z"), common(b"c"), d(b"a"), d(b"a"), common(b"cc")],
                    ec_symbols: vec![d(b"e")],
                },
            ),
            (
                1,
                MemberSymbols {
                    symbols: vec![d(b"c"), d(b"a"), common(b"cc"), common(b"only")],
                    ec_symbols: vec![d(b"e"), d(b"f")],
                },
            ),
            (
                2,
                MemberSymbols {
                    symbols: vec![common(b"cc"), d(b"z")],
                    ec_symbols: vec![],
                },
            ),
        ]
    };
    let names = |i: &Index<'_>| -> Vec<(String, u32)> {
        i.symbols
            .iter()
            .map(|s| (text(&s.name), s.member))
            .collect()
    };
    let s = |n: &str, m: u32| (String::from(n), m);
    let lib = Index::build(members(), Tool::LibExe, true);
    assert_eq!(
        names(&lib),
        [s("a", 0), s("z", 0), s("c", 1), s("only", 1), s("cc", 2)]
    );
    let ec: Vec<(String, u32)> = lib
        .ec_symbols
        .as_ref()
        .unwrap()
        .iter()
        .map(|s| (text(&s.name), s.member))
        .collect();
    assert_eq!(ec, [s("e", 0), s("f", 1)]);
    assert_eq!(lib.member_list, None);
    let llvm = Index::build(members(), Tool::LlvmLib, false);
    assert_eq!(
        names(&llvm),
        [s("z", 0), s("c", 0), s("a", 0), s("cc", 0), s("only", 1)]
    );
    assert_eq!(llvm.ec_symbols, None);
    // Members given out of archive order make the member list.
    let mut out_of_order = members();
    out_of_order.swap(0, 2);
    let i = Index::build(out_of_order, Tool::LibExe, false);
    assert_eq!(i.member_list, Some(vec![Some(2), Some(1), Some(0)]));
    // (cc: member 2's common, superseded by member 1's, then member 0's.)
    assert_eq!(names(&i)[..2], [s("z", 2), s("a", 1)]);
    assert!(names(&i).contains(&s("cc", 0)));
}

#[test]
fn limits() {
    let header = |size: u64| MemberHeader {
        name: b"x.obj",
        fields: Fields::BLANK,
        size,
    };
    let none = Index::default();
    let style = Style::LibExeBrepro;
    // At most 65,535 members.
    let many = vec![header(0); MAX_MEMBERS];
    assert!(Layout::new(&many, &none, style, SizeLimit::FourGiB).is_ok());
    let many = vec![header(0); MAX_MEMBERS + 1];
    assert_eq!(
        Layout::new(&many, &none, style, SizeLimit::FourGiB).unwrap_err(),
        WriteError::TooManyMembers { count: 65_536 }
    );
    // lib.exe's 4 GiB, and llvm-lib's offsets.
    let big = [header(1 << 31), header((1 << 31) - 1000), header(1 << 31)];
    let e = Layout::new(&big, &none, style, SizeLimit::FourGiB).unwrap_err();
    assert!(matches!(e, WriteError::TooLarge { size } if size > 1 << 32));
    let two = [header(1 << 31), header((1 << 31) - 400)];
    let l = Layout::new(&two, &none, style, SizeLimit::FourGiB).unwrap();
    assert!(l.size() <= u64::from(u32::MAX));
    let l = Layout::new(&big, &none, style, SizeLimit::Offsets).unwrap();
    assert!(l.size() > 1 << 32 && l.offset(2) < 1 << 32);
    let four = [big[0], big[1], big[2], header(0)];
    assert!(matches!(
        Layout::new(&four, &none, style, SizeLimit::Offsets).unwrap_err(),
        WriteError::OffsetTooLarge { member: 3, .. }
    ));
    assert_eq!(
        Layout::new(&[header(10_000_000_000)], &none, style, SizeLimit::Offsets).unwrap_err(),
        WriteError::MemberTooLarge { member: 0 }
    );
    // NULs end names and symbols.
    let nul = MemberHeader {
        name: b"a_long_name_with\0nul.obj",
        ..header(0)
    };
    assert_eq!(
        Layout::new(&[nul], &none, style, SizeLimit::FourGiB).unwrap_err(),
        WriteError::BadName { member: 0 }
    );
    let bad = Index {
        symbols: vec![sym(b"ok", 0), sym(b"n\0ul", 0)],
        ..Index::default()
    };
    assert_eq!(
        Layout::new(&[header(0)], &bad, style, SizeLimit::FourGiB).unwrap_err(),
        WriteError::BadSymbol { symbol: 1 }
    );
    let bad = Index {
        symbols: vec![sym(b"ok", 1)],
        ..Index::default()
    };
    assert_eq!(
        Layout::new(&[header(0)], &bad, style, SizeLimit::FourGiB).unwrap_err(),
        WriteError::NoSuchMember { symbol: 0 }
    );
    let bad = Index {
        ec_symbols: Some(vec![sym(b"ok", 2)]),
        ..Index::default()
    };
    assert_eq!(
        Layout::new(&[header(0)], &bad, style, SizeLimit::FourGiB).unwrap_err(),
        WriteError::NoSuchMember { symbol: 0 }
    );
    for e in [
        WriteError::TooLarge { size: 1 },
        WriteError::OffsetTooLarge {
            member: 0,
            offset: 1,
        },
        WriteError::TooManyMembers { count: 1 },
        WriteError::MemberTooLarge { member: 0 },
        WriteError::BadName { member: 0 },
        WriteError::BadSymbol { symbol: 0 },
        WriteError::NoSuchMember { symbol: 0 },
        WriteError::NoSuchListedMember { position: 0 },
    ] {
        assert!(!std::format!("{e}").is_empty());
    }
}

/// No members: the two linker members, empty.
#[test]
fn empty_archive() {
    let out = write(
        &[],
        &Index::default(),
        Style::LibExeBrepro,
        SizeLimit::FourGiB,
    )
    .unwrap();
    let ar = Archive::parse(&out).unwrap();
    let all: Vec<Member<'_>> = ar.members().collect();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].data, [0, 0, 0, 0]);
    assert_eq!(all[1].data, [0; 8]);
    let c = Contents::read(&ar).unwrap();
    assert!(c.members.is_empty());
    assert_eq!(c.write(SizeLimit::FourGiB).unwrap(), out);
}

/// What can't be read back.
#[test]
fn reading_back() {
    assert_eq!(
        Contents::read(&Archive::parse(MAGIC).unwrap()).unwrap_err(),
        ReadError::NotMicrosoft
    );
    assert_eq!(
        Contents::read(&Archive::parse(crate::THIN_MAGIC).unwrap()).unwrap_err(),
        ReadError::NotMicrosoft
    );
    let out = write(
        &members(Fields::BLANK),
        &index(),
        Style::LibExeBrepro,
        SizeLimit::FourGiB,
    )
    .unwrap();
    // Every truncation reads back (as something) or fails, never panics.
    for len in 8..out.len() {
        if let Ok(ar) = Archive::parse(&out[..len]) {
            let _ = Contents::read(&ar);
        }
    }
    for e in [
        ReadError::NotMicrosoft,
        ReadError::TableAmongFiles { offset: 1 },
        ReadError::BadTable("x"),
        ReadError::BadMember { offset: 1 },
    ] {
        assert!(!std::format!("{e}").is_empty());
    }
}

/// Members laid out, then written one at a time in any order.
#[test]
fn writing_in_parts() {
    let style = Style::LlvmLib;
    let m = members(style.object_fields(0, false));
    let whole = write(&m, &index(), style, SizeLimit::FourGiB).unwrap();
    let headers: Vec<MemberHeader<'_>> = m
        .iter()
        .map(|m| MemberHeader {
            name: m.name,
            fields: m.fields,
            size: m.data.len() as u64,
        })
        .collect();
    let layout = Layout::new(&headers, &index(), style, SizeLimit::FourGiB).unwrap();
    assert_eq!(layout.len(), 4);
    assert!(!layout.is_empty());
    let mut out = vec![0xEE; layout.size() as usize];
    let (head, spans) = layout.split_mut(&mut out);
    head.copy_from_slice(layout.head());
    for (i, span) in spans.into_iter().enumerate().rev() {
        layout.write_member(i, m[i].data, span);
    }
    assert_eq!(out, whole);
    // Or the data copied in pieces, in any order.
    let mut out = vec![0xEE; layout.size() as usize];
    let mut copies = layout.prepare_mut(&mut out, |i| m[i].data, 3);
    assert_eq!(copies.len(), 5); // 3 bytes, then 3 and 1, then 1, then 2
    for (to, from) in copies.iter_mut().rev() {
        to.copy_from_slice(from);
    }
    drop(copies);
    assert_eq!(out, whole);
    for i in 0..4 {
        let at = layout.offset(i) as usize;
        assert_eq!(&whole[at..at + 60], layout.header(i));
    }
}

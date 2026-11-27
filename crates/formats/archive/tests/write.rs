//! The test libraries (testdata/coff and testdata/coff/archives, from
//! tools/coff/build.cmd and tools/coff/archives.cmd: made by lib.exe with
//! and without /Brepro, and by llvm-lib) written again: each must come out
//! byte for byte as it was, its index must be the one the tool that made
//! it builds from its members' symbols, and each import object must write
//! back as it was. Then what those libraries show of each tool, checked
//! one fact at a time.

// The test libraries are small: their offsets and counts fit anywhere.
#![allow(clippy::cast_possible_truncation)]

use turbonop_global_alloc as _;

use std::path::PathBuf;

use turbonop_archive::symbols::{Tool, is_import_descriptor, member_symbols};
use turbonop_archive::write::{Contents, Index, IndexSymbol, SizeLimit, Style};
use turbonop_archive::{Archive, ArchiveSymbol, MemberKind};
use turbonop_pe::import_object::{ImportNameType, ImportObject, ImportType};

fn coff() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/coff")
}

fn libraries() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for dir in [coff(), coff().join("archives")] {
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "lib"))
            .collect();
        names.sort();
        for p in names {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            out.push((name, std::fs::read(&p).unwrap()));
        }
    }
    assert!(out.len() > 60, "{} libraries", out.len());
    out
}

fn library(name: &str) -> Vec<u8> {
    std::fs::read(coff().join("archives").join(name)).unwrap()
}

/// The index `tool` builds from `contents`' members, in the order the
/// second linker member lists them (`lib /DEF` lists the `.def` file's
/// order, not the archive's).
fn computed<'a>(contents: &Contents<'a>) -> Index<'a> {
    let tool = match contents.style {
        Some(Style::LlvmLib) => Tool::LlvmLib,
        _ => Tool::LibExe,
    };
    let ec = contents.index.ec_symbols.is_some();
    let imports = contents
        .members
        .iter()
        .any(|m| ImportObject::is_import_object(m.data));
    let order: Vec<u32> = match &contents.index.member_list {
        Some(list) => list.iter().flatten().copied().collect(),
        None => (0..contents.members.len() as u32).collect(),
    };
    let members = order.into_iter().map(|i| {
        let data = contents.members[i as usize].data;
        let mut s = match member_symbols(data, tool, ec) {
            Ok(s) => s,
            // An LTCG object: its symbols as the library has them.
            Err(_) => turbonop_archive::write::MemberSymbols {
                symbols: contents
                    .index
                    .symbols
                    .iter()
                    .filter(|s| s.member == i)
                    .map(|s| turbonop_archive::write::MemberSymbol::defined(s.name.clone()))
                    .collect(),
                ec_symbols: Vec::new(),
            },
        };
        // An ARM64EC import library indexes its descriptors in both tables.
        if ec && imports {
            let both: Vec<_> = s
                .symbols
                .iter()
                .filter(|n| is_import_descriptor(&n.name))
                .cloned()
                .collect();
            s.ec_symbols.extend(both);
        }
        (i, s)
    });
    Index::build(members, tool, ec)
}

fn sorted<'a>(v: &Option<Vec<IndexSymbol<'a>>>) -> Option<Vec<IndexSymbol<'a>>> {
    v.clone().map(|mut v| {
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    })
}

#[test]
fn every_library_writes_back_as_it_was() {
    for (name, data) in libraries() {
        let ar = Archive::parse(&data).unwrap();
        let contents = Contents::read(&ar).unwrap_or_else(|e| panic!("{name}: {e}"));
        let out = contents.write(SizeLimit::FourGiB).unwrap();
        assert!(out == data, "{name}: written differently");
        // Laid out member by member, in any order, it is the same.
        let layout = contents.layout(SizeLimit::FourGiB).unwrap();
        let mut parts = vec![0u8; layout.size() as usize];
        let (head, spans) = layout.split_mut(&mut parts);
        head.copy_from_slice(layout.head());
        for (i, span) in spans.into_iter().enumerate().rev() {
            layout.write_member(i, contents.members[i].data, span);
        }
        assert!(parts == data, "{name}: written in parts differently");

        let built = computed(&contents);
        assert_eq!(built.symbols, contents.index.symbols, "{name}: index");
        assert_eq!(
            sorted(&built.ec_symbols),
            sorted(&contents.index.ec_symbols),
            "{name}: EC index"
        );
        for m in &contents.members {
            if ImportObject::is_import_object(m.data) {
                let i = ImportObject::parse(m.data).unwrap();
                assert_eq!(i.to_vec().unwrap(), m.data, "{name}: {i:?}");
            }
        }
    }
}

/// Each tool's header fields, as `lib.exe` (with and without `/Brepro`)
/// and `llvm-lib` wrote them.
#[test]
fn header_fields() {
    let fields = |lib: &str| -> Vec<(String, String, String, String, String)> {
        let data = library(lib);
        let ar = Archive::parse(&data).unwrap();
        ar.members()
            .map(|m| {
                let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
                (s(&m.name), s(m.date), s(m.uid), s(m.gid), s(m.mode))
            })
            .collect()
    };
    let llvm = fields("sort-llvm.lib");
    assert_eq!(
        llvm[0],
        ("/".into(), "0".into(), "0".into(), "0".into(), "0".into())
    );
    assert_eq!(
        llvm[2],
        ("//".into(), "".into(), "".into(), "".into(), "".into())
    );
    assert_eq!(
        (&*llvm[3].1, &*llvm[3].2, &*llvm[3].3, &*llvm[3].4),
        ("0", "0", "0", "644")
    );
    let repro = fields("sort-brepro.lib");
    for f in &repro {
        assert_eq!(f.1, "-1");
        assert_eq!((f.2.as_str(), f.3.as_str()), ("", ""));
    }
    assert_eq!(repro[0].4, "0");
    assert_eq!(repro[3].4, "100666");
    let dated = fields("sort.lib");
    assert!(dated[0].1.parse::<u64>().unwrap() > 1_700_000_000);
    assert_eq!(dated[0].4, "0");
    assert_eq!(dated[3].4, "100666");
    // Import objects: mode 0, and in llvm-lib's, 644.
    let imports = fields("def-x64.lib");
    assert!(imports[3..].iter().all(|f| f.4 == "0" && f.1 == "-1"));
    let imports = fields("def-x64_llvm-x64-llvm.lib");
    assert!(imports[3..].iter().all(|f| f.4 == "644" && f.1 == "0"));
}

/// Member names: up to 15 bytes in the header, longer ones in `//` (once
/// each), `lib.exe` storing `\` for `/` and `llvm-lib` a long name for
/// any with `/`; and the members in the reverse of the command line.
#[test]
fn member_names() {
    let names = |lib: &str| -> Vec<(String, String)> {
        let data = library(lib);
        let ar = Archive::parse(&data).unwrap();
        ar.files()
            .map(|m| {
                let h = &data[m.header_offset as usize..][..16];
                (
                    String::from_utf8_lossy(&m.name).into_owned(),
                    String::from_utf8_lossy(h).trim_end().to_owned(),
                )
            })
            .collect()
    };
    let got = names("names-brepro.lib");
    let want = [
        ("sub2\\long_name_again.obj", "/0"),
        ("sub\\long_name_again.obj", "/25"),
        ("sub2\\x.obj", "sub2\\x.obj/"),
        ("sub\\x.obj", "sub\\x.obj/"),
        ("n234567890abc.obj", "/49"),
        ("n234567890ab.obj", "/67"),
        ("n234567890a.obj", "n234567890a.obj/"),
        ("n234567890.obj", "n234567890.obj/"),
        ("n23456789.obj", "n23456789.obj/"),
    ];
    let got: Vec<(&str, &str)> = got.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    assert_eq!(got, want);
    assert_eq!(names("names-llvm.lib"), names("names-brepro.lib"));
    assert_eq!(names("slash-brepro.lib")[0].1, "sub\\fs.obj/");
    assert_eq!(
        names("slash-llvm.lib")[0],
        ("sub/fs.obj".into(), "/0".into())
    );
    // Every import object of an import library has the one name.
    let data = library("def-lib_quoted.lib");
    let ar = Archive::parse(&data).unwrap();
    assert!(ar.files().all(|m| &*m.name == b"with space.dll"));
}

fn first_member(lib: &str) -> Vec<(String, String)> {
    let data = library(lib);
    let ar = Archive::parse(&data).unwrap();
    let name_of = |s: &ArchiveSymbol<'_>| {
        String::from_utf8_lossy(&ar.member_at_offset(s.member).unwrap().name).into_owned()
    };
    ar.first_linker_member()
        .unwrap()
        .iter()
        .map(|s| (String::from_utf8_lossy(s.name).into_owned(), name_of(s)))
        .collect()
}

/// The first linker member: `lib.exe` sorts each member's symbols,
/// `llvm-lib` keeps their order; both name each symbol once, for the first
/// member; the second linker member sorts all of them by their bytes.
#[test]
fn symbol_order_and_duplicates() {
    let lib = first_member("sort-brepro.lib");
    let s1: Vec<&str> = lib
        .iter()
        .filter(|(_, m)| m == "s1.obj")
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(
        s1,
        [
            "??_C@_0x",
            "?x@@YAXXZ",
            "Beta",
            "_under",
            "a",
            "a$b",
            "a.b",
            "a@b",
            "a_b",
            "aa",
            "dupself",
            "zeta",
            "~t",
            "\u{fffd}hi"
        ]
    );
    let llvm = first_member("sort-llvm.lib");
    let s1: Vec<&str> = llvm
        .iter()
        .filter(|(_, m)| m == "s1.obj")
        .map(|(n, _)| n.as_str())
        .collect();
    assert_eq!(
        s1,
        [
            "zeta",
            "Beta",
            "_under",
            "aa",
            "a",
            "\u{fffd}hi",
            "~t",
            "a_b",
            "a.b",
            "a@b",
            "?x@@YAXXZ",
            "??_C@_0x",
            "a$b",
            "dupself"
        ]
    );
    // alpha and A: the first member in the archive (the last on the
    // command line) that defines them.
    for lib in [&lib, &llvm] {
        assert!(lib.contains(&("alpha".into(), "s3_with_a_long_name.obj".into())));
        assert_eq!(lib.iter().filter(|(n, _)| n == "alpha").count(), 1);
    }
    let data = library("sort-brepro.lib");
    let (_, second) = Archive::parse(&data)
        .unwrap()
        .second_linker_member()
        .unwrap();
    let names: Vec<&[u8]> = second.iter().map(|s| s.name).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted);
    assert_eq!(names.last(), Some(&&b"\x80hi"[..]));
}

/// A common symbol yields to a later member's definition in `lib.exe`'s
/// index, and to a later common one; `llvm-lib` keeps the first.
#[test]
fn common_symbols() {
    let lib = first_member("common-brepro.lib");
    let who =
        |lib: &[(String, String)], n: &str| lib.iter().find(|(s, _)| s == n).unwrap().1.clone();
    // In archive order: c_common.obj, c_def.obj, c_weak.obj, c_def2.obj.
    assert_eq!(who(&lib, "x"), "c_def.obj"); // common, then defined
    assert_eq!(who(&lib, "z"), "c_common.obj"); // defined, then common
    assert_eq!(who(&lib, "w"), "c_def.obj"); // common 8, then common 16
    assert_eq!(who(&lib, "abs1"), "c_def.obj"); // absolute, then common
    assert_eq!(who(&lib, "wk"), "c_common.obj"); // weak, then defined
    assert_eq!(who(&lib, "wk2"), "c_weak.obj");
    let lib = first_member("commons-brepro.lib");
    // d1.obj (16), d2.obj (8), d3.obj (4): the last.
    assert_eq!(who(&lib, "big_first"), "d3.obj");
    assert_eq!(who(&lib, "eq"), "d3.obj");
    let llvm = first_member("common-llvm.lib");
    assert_eq!(who(&llvm, "x"), "c_common.obj");
    assert_eq!(who(&llvm, "w"), "c_common.obj");
}

/// Which symbols each tool indexes (see `turbonop_archive::symbols`).
#[test]
fn indexed_symbols() {
    let names = |lib: &str| -> Vec<String> {
        let mut v: Vec<String> = first_member(lib).into_iter().map(|(n, _)| n).collect();
        v.sort();
        v
    };
    assert_eq!(
        names("kinds-brepro.lib"),
        [
            "abcdefgh",
            "comdat_leader",
            "e_abs0",
            "e_aux",
            "ext_abs",
            "ext_common",
            "ext_def",
            "ext_long_name_more_than_8",
            "ext_val2",
            "target",
            "w_abs_c2",
            "w_s0v5_c1",
            "w_s0v5_c3",
            "w_s1_c1",
            "w_s1_c2",
            "w_s1_c4",
            "weak_alias",
            "weak_in_sect"
        ]
    );
    assert_eq!(
        names("kinds-llvm.lib"),
        [
            "abcdefgh",
            "comdat_leader",
            "e_abs0",
            "e_aux",
            "ext_abs",
            "ext_common",
            "ext_def",
            "ext_long_name_more_than_8",
            "ext_val2",
            "target",
            "w_s0v5_c3",
            "weak_alias",
            "weak_in_sect"
        ]
    );
}

/// ARM64EC and ARM64X: EC and x64 objects' symbols in `/<ECSYMBOLS>/`
/// (after `//`), the others in the linker members.
#[test]
fn ec_symbols() {
    for lib in [
        "mix-brepro.lib",
        "mix-llvm.lib",
        "x-brepro.lib",
        "ec-llvm.lib",
    ] {
        let data = library(lib);
        let ar = Archive::parse(&data).unwrap();
        let kinds: Vec<MemberKind> = ar
            .members()
            .take_while(|m| m.kind != MemberKind::File)
            .map(|m| m.kind)
            .collect();
        assert_eq!(&kinds[..2], [MemberKind::SymbolTable; 2], "{lib}");
        assert_eq!(kinds.last(), Some(&MemberKind::EcSymbolTable), "{lib}");
        let ec = ar.ec_symbols().unwrap();
        let machine_of = |s: &ArchiveSymbol<'_>| {
            let m = ar.member_at_offset(s.member).unwrap();
            u16::from_le_bytes([m.data[0], m.data[1]])
        };
        assert!(ec.iter().all(|s| machine_of(s) != 0xAA64), "{lib}");
        let regular = ar.first_linker_member().unwrap();
        assert!(regular.iter().all(|s| machine_of(s) == 0xAA64), "{lib}");
    }
    let data = library("mix-brepro.lib");
    let ar = Archive::parse(&data).unwrap();
    let ec: Vec<&[u8]> = ar.ec_symbols().unwrap().iter().map(|s| s.name).collect();
    assert!(ec.contains(&&b"x64_func"[..]) && ec.contains(&&b"#ec_func"[..]));
}

/// Import objects of every type and name type write back as they were.
#[test]
fn import_objects_of_every_kind() {
    let mut kinds = std::collections::BTreeSet::new();
    for (name, data) in libraries() {
        let ar = Archive::parse(&data).unwrap();
        for m in ar.files() {
            if ImportObject::is_import_object(m.data) {
                let i = ImportObject::parse(m.data).unwrap();
                assert_eq!(i.to_vec().unwrap(), m.data, "{name}");
                kinds.insert((format!("{:?}", i.kind), format!("{:?}", i.name_type)));
                kinds.insert(("third name".into(), i.export_name.is_some().to_string()));
            }
        }
    }
    for t in [ImportType::Code, ImportType::Data, ImportType::Const] {
        assert!(kinds.contains(&(format!("{t:?}"), format!("{:?}", ImportNameType::Name))));
    }
    for n in [
        ImportNameType::Ordinal,
        ImportNameType::NoPrefix,
        ImportNameType::Undecorate,
        ImportNameType::ExportAs,
    ] {
        assert!(
            kinds.iter().any(|(_, k)| *k == format!("{n:?}")),
            "{n:?}: {kinds:?}"
        );
    }
}

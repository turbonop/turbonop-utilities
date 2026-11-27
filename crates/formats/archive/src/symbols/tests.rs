// Test objects are small.
#![allow(clippy::cast_possible_truncation)]

extern crate std;

use alloc::vec;
use std::string::String;

use super::*;

const EXTERNAL: u8 = 2;
const STATIC: u8 = 3;
const WEAK: u8 = 105;

/// A symbol: its name, value, section, storage class, and auxiliary
/// records.
type Symbol<'a> = (&'a str, u32, i16, u8, &'a [[u8; 18]]);

/// A COFF object with one section and these symbols.
fn object(machine: u16, symbols: &[Symbol<'_>]) -> Vec<u8> {
    let mut strings = vec![0u8; 4];
    let mut records = Vec::new();
    let mut count = 0u32;
    for &(name, value, section, class, aux) in symbols {
        if name.len() <= 8 {
            let mut f = [0u8; 8];
            f[..name.len()].copy_from_slice(name.as_bytes());
            records.extend_from_slice(&f);
        } else {
            records.extend_from_slice(&[0; 4]);
            records.extend_from_slice(&(strings.len() as u32).to_le_bytes());
            strings.extend_from_slice(name.as_bytes());
            strings.push(0);
        }
        records.extend_from_slice(&value.to_le_bytes());
        records.extend_from_slice(&section.to_le_bytes());
        records.extend_from_slice(&0u16.to_le_bytes());
        records.push(class);
        records.push(aux.len() as u8);
        for a in aux {
            records.extend_from_slice(a);
        }
        count += 1 + aux.len() as u32;
    }
    let len = strings.len() as u32;
    strings[..4].copy_from_slice(&len.to_le_bytes());
    let mut out = Vec::new();
    out.extend_from_slice(&machine.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(20u32 + 40 + 4).to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(b".text\0\0\0");
    for v in [0u32, 0, 4, 60, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&0x6000_0020u32.to_le_bytes());
    out.extend_from_slice(&[0xC3; 4]);
    out.extend_from_slice(&records);
    out.extend_from_slice(&strings);
    out
}

fn weak(tag: u32, kind: u32) -> [u8; 18] {
    let mut a = [0u8; 18];
    a[..4].copy_from_slice(&tag.to_le_bytes());
    a[4..8].copy_from_slice(&kind.to_le_bytes());
    a
}

fn kinds() -> Vec<u8> {
    let w = |k| [weak(1, k)];
    object(
        machine::AMD64,
        &[
            (".text", 0, 1, STATIC, &[[0; 18]]),
            ("defined", 0, 1, EXTERNAL, &[]),
            ("undefined", 0, 0, EXTERNAL, &[]),
            ("common", 8, 0, EXTERNAL, &[]),
            ("absolute", 5, -1, EXTERNAL, &[]),
            ("static", 0, 1, STATIC, &[]),
            ("w_nolib", 0, 0, WEAK, &w(1)),
            ("w_lib", 0, 0, WEAK, &w(2)),
            ("w_alias", 0, 0, WEAK, &w(3)),
            ("w_anti", 0, 0, WEAK, &w(4)),
            ("w_in_section", 0, 1, WEAK, &w(1)),
            ("w_absolute", 0, -1, WEAK, &w(2)),
            ("w_value", 5, 0, WEAK, &w(1)),
            ("label", 0, 1, 6, &[]),
            ("extdef", 0, 1, 5, &[]),
            ("clrtoken", 0, -1, 107, &[]),
            ("section", 0, 1, 104, &[]),
            ("a_name_longer_than_eight", 2, 1, EXTERNAL, &[]),
        ],
    )
}

fn names(s: &[MemberSymbol<'_>]) -> Vec<String> {
    s.iter()
        .map(|s| String::from_utf8_lossy(&s.name).into_owned())
        .collect()
}

#[test]
fn coff_objects() {
    let data = kinds();
    let lib = member_symbols(&data, Tool::LibExe, false).unwrap();
    assert_eq!(
        names(&lib.symbols),
        [
            "defined",
            "common",
            "absolute",
            "w_alias",
            "w_in_section",
            "w_absolute",
            "w_value",
            "a_name_longer_than_eight"
        ]
    );
    assert!(lib.ec_symbols.is_empty());
    let common: Vec<bool> = lib.symbols.iter().map(|s| s.common).collect();
    assert_eq!(
        common,
        [false, true, false, false, false, false, false, false]
    );
    let llvm = member_symbols(&data, Tool::LlvmLib, false).unwrap();
    assert_eq!(
        names(&llvm.symbols),
        [
            "defined",
            "common",
            "absolute",
            "w_alias",
            "a_name_longer_than_eight"
        ]
    );
    // In an ARM64EC archive, x64 and ARM64EC objects' symbols are EC
    // symbols; ARM64 objects' are not.
    let ec = member_symbols(&data, Tool::LibExe, true).unwrap();
    assert!(ec.symbols.is_empty());
    assert_eq!(names(&ec.ec_symbols), names(&lib.symbols));
    let arm64 = object(machine::ARM64, &[("f", 0, 1, EXTERNAL, &[])]);
    let s = member_symbols(&arm64, Tool::LibExe, true).unwrap();
    assert_eq!(
        (names(&s.symbols), s.ec_symbols.len()),
        (vec!["f".into()], 0)
    );
    let arm64ec = object(machine::ARM64EC, &[("#f", 0, 1, EXTERNAL, &[])]);
    let s = member_symbols(&arm64ec, Tool::LibExe, true).unwrap();
    assert_eq!(names(&s.ec_symbols), ["#f"]);
    assert!(is_ec_machine(machine::AMD64) && !is_ec_machine(machine::I386));
}

/// What `lib.exe` refuses (and `llvm-lib` takes).
#[test]
fn refused() {
    let debug = object(machine::AMD64, &[("x", 0, -2, EXTERNAL, &[])]);
    assert_eq!(
        member_symbols(&debug, Tool::LibExe, false).unwrap_err(),
        SymbolsError::Refused { symbol: 0 }
    );
    let s = member_symbols(&debug, Tool::LlvmLib, false).unwrap();
    assert_eq!(names(&s.symbols), ["x"]);
    for aux in [&[][..], &[weak(0, 0)], &[weak(0, 5)]] {
        let bad = object(machine::AMD64, &[("w", 0, 0, WEAK, aux)]);
        assert_eq!(
            member_symbols(&bad, Tool::LibExe, false).unwrap_err(),
            SymbolsError::Refused { symbol: 0 }
        );
        assert!(
            member_symbols(&bad, Tool::LlvmLib, false)
                .unwrap()
                .symbols
                .is_empty()
        );
    }
    assert_eq!(
        member_symbols(b"neither", Tool::LibExe, false).unwrap_err(),
        SymbolsError::NotAnObject
    );
    let mut bad = kinds();
    bad[12..16].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
    assert!(matches!(
        member_symbols(&bad, Tool::LibExe, false),
        Err(SymbolsError::Malformed(_))
    ));
    for e in [
        SymbolsError::Ltcg,
        SymbolsError::NotAnObject,
        SymbolsError::Refused { symbol: 1 },
    ] {
        assert!(!std::format!("{e}").is_empty());
    }
    // Every truncation is an error or some symbols, never a panic.
    let data = kinds();
    for len in 0..data.len() {
        let _ = member_symbols(&data[..len], Tool::LibExe, false);
    }
}

fn testdata(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/coff")
        .join(name);
    std::fs::read(path).unwrap()
}

/// A `/bigobj` object lists what the same object without it lists; an
/// LTCG object's symbols aren't computed.
#[test]
fn bigobj_and_ltcg() {
    let small = testdata("objects.obj");
    let big = testdata("objects-big.obj");
    let a = member_symbols(&small, Tool::LibExe, false).unwrap();
    let b = member_symbols(&big, Tool::LibExe, false).unwrap();
    assert!(!a.symbols.is_empty());
    assert_eq!(a, b);
    assert_eq!(
        member_symbols(&testdata("ltcg.obj"), Tool::LibExe, false).unwrap_err(),
        SymbolsError::Ltcg
    );
}

fn import<'a>(machine: u16, kind: ImportType, symbol: &'a [u8]) -> ImportObject<'a> {
    ImportObject::new(
        machine,
        kind,
        turbonop_pe::import_object::ImportNameType::Name,
        0,
        symbol,
        b"x.dll",
        None,
    )
}

#[test]
fn import_objects() {
    let s = |i: &ImportObject<'_>, tool| names(&import_symbols(i, tool, false).symbols);
    for tool in [Tool::LibExe, Tool::LlvmLib] {
        assert_eq!(
            s(&import(machine::AMD64, ImportType::Code, b"f"), tool),
            ["__imp_f", "f"]
        );
        assert_eq!(
            s(&import(machine::AMD64, ImportType::Const, b"c"), tool),
            ["__imp_c", "c"]
        );
        assert_eq!(
            s(&import(machine::I386, ImportType::Data, b"_d"), tool),
            ["__imp__d"]
        );
    }
    // From the bytes, the same.
    let bytes = import(machine::AMD64, ImportType::Code, b"f")
        .to_vec()
        .unwrap();
    assert_eq!(
        names(&member_symbols(&bytes, Tool::LibExe, false).unwrap().symbols),
        ["__imp_f", "f"]
    );
    // ARM64EC: in /<ECSYMBOLS>/, with the mangled and plain names and
    // __imp_aux_.
    let ec = |kind, symbol: &'static [u8], tool| {
        let i = import(machine::ARM64EC, kind, symbol);
        let m = import_symbols(&i, tool, true);
        assert!(m.symbols.is_empty());
        let mut v = names(&m.ec_symbols);
        v.sort();
        v
    };
    assert_eq!(
        ec(ImportType::Code, b"#f", Tool::LibExe),
        ["#f", "__imp_aux_f", "__imp_f", "f"]
    );
    assert_eq!(
        ec(ImportType::Code, b"?f@@$$hYAXXZ", Tool::LlvmLib),
        [
            "?f@@$$hYAXXZ",
            "?f@@YAXXZ",
            "__imp_?f@@YAXXZ",
            "__imp_aux_?f@@YAXXZ"
        ]
    );
    assert_eq!(
        ec(ImportType::Const, b"c", Tool::LibExe),
        ["#c", "__imp_aux_c", "__imp_c", "c"]
    );
    assert_eq!(
        ec(ImportType::Const, b"c", Tool::LlvmLib),
        ["__imp_aux_c", "__imp_c", "c"]
    );
    // (A constant's C++ name that isn't a function's: lib.exe lists "".)
    assert_eq!(
        ec(ImportType::Const, b"?v@@3HA", Tool::LibExe),
        ["", "?v@@3HA", "__imp_?v@@3HA", "__imp_aux_?v@@3HA"]
    );
    assert_eq!(ec(ImportType::Data, b"d", Tool::LibExe), ["__imp_d"]);
    // Outside an EC archive, an ARM64EC import's symbols are ordinary.
    let i = import(machine::ARM64EC, ImportType::Data, b"d");
    assert_eq!(
        names(&import_symbols(&i, Tool::LibExe, false).symbols),
        ["__imp_d"]
    );
}

#[test]
fn ec_names() {
    assert_eq!(&*ec_demangle(b"#f"), b"f");
    assert_eq!(&*ec_demangle(b"?f@@$$hYAXXZ"), b"?f@@YAXXZ");
    assert_eq!(&*ec_demangle(b"plain"), b"plain");
    assert_eq!(&*ec_demangle(b"?no_h@@YAXXZ"), b"?no_h@@YAXXZ");
    assert_eq!(ec_mangle(b"f").as_deref(), Some(&b"#f"[..]));
    assert_eq!(ec_mangle(b"#f").as_deref(), Some(&b"#f"[..]));
    assert_eq!(
        ec_mangle(b"?f@@YAXXZ").as_deref(),
        Some(&b"?f@@$$hYAXXZ"[..])
    );
    assert_eq!(
        ec_mangle(b"?f@@$$hYAXXZ").as_deref(),
        Some(&b"?f@@$$hYAXXZ"[..])
    );
    assert_eq!(
        ec_mangle(b"?m@C@@QEAAXXZ").as_deref(),
        Some(&b"?m@C@@$$hQEAAXXZ"[..])
    );
    assert_eq!(ec_mangle(b"?v@@3HA"), None);
    assert_eq!(ec_mangle(b"?broken"), None);
    assert!(is_import_descriptor(b"__IMPORT_DESCRIPTOR_x"));
    assert!(is_import_descriptor(b"__NULL_IMPORT_DESCRIPTOR"));
    assert!(is_import_descriptor(b"\x7fx_NULL_THUNK_DATA"));
    assert!(!is_import_descriptor(b"x_NULL_THUNK_DATA"));
}

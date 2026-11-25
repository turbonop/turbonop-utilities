extern crate std;
use std::vec::Vec;

use super::*;

fn object(kind: u16, name_type: u16, hint: u16, names: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&[0, 0, 0xFF, 0xFF, 0, 0]);
    v.extend_from_slice(&0x8664u16.to_le_bytes());
    v.extend_from_slice(&0x1234_5678u32.to_le_bytes());
    v.extend_from_slice(&u32::try_from(names.len()).unwrap().to_le_bytes());
    v.extend_from_slice(&hint.to_le_bytes());
    v.extend_from_slice(&(kind | (name_type << 2)).to_le_bytes());
    v.extend_from_slice(names);
    v
}

#[test]
fn names_by_type() {
    let o = object(0, 1, 7, b"CreateFileW\0KERNEL32.dll\0");
    let i = ImportObject::parse(&o).unwrap();
    assert_eq!(i.machine, 0x8664);
    assert_eq!(i.time_date_stamp, 0x1234_5678);
    assert_eq!(i.ordinal_or_hint, 7);
    assert_eq!(i.kind, ImportType::Code);
    assert_eq!(i.name_type, ImportNameType::Name);
    assert_eq!(
        (i.symbol, i.dll),
        (&b"CreateFileW"[..], &b"KERNEL32.dll"[..])
    );
    assert_eq!(i.import_name(), Some(&b"CreateFileW"[..]));
    assert_eq!(i.size(), 20 + 25);
    let o = object(1, 2, 0, b"_errno\0msvcrt.dll\0");
    let i = ImportObject::parse(&o).unwrap();
    assert_eq!(i.kind, ImportType::Data);
    assert_eq!(i.import_name(), Some(&b"errno"[..]));
    let o = object(0, 3, 0, b"_Sleep@4\0KERNEL32.dll\0");
    assert_eq!(
        ImportObject::parse(&o).unwrap().import_name(),
        Some(&b"Sleep"[..])
    );
    let o = object(0, 0, 42, b"Ord\0X.dll\0");
    let i = ImportObject::parse(&o).unwrap();
    assert_eq!(
        (i.name_type, i.import_name()),
        (ImportNameType::Ordinal, None)
    );
    let o = object(2, 4, 0, b"#f\0X.dll\0f_EC\0");
    let i = ImportObject::parse(&o).unwrap();
    assert_eq!(i.kind, ImportType::Const);
    assert_eq!(i.import_name(), Some(&b"f_EC"[..]));
    let o = object(3, 7, 0, b"a\0b\0");
    let i = ImportObject::parse(&o).unwrap();
    assert_eq!(
        (i.kind, i.name_type),
        (ImportType::Unknown(3), ImportNameType::Unknown(7))
    );
}

#[test]
fn damaged() {
    assert!(ImportObject::parse(b"").is_err());
    assert!(!ImportObject::is_import_object(b"\0\0\xFF\xFF\x02\0"));
    // No DLL name.
    assert!(ImportObject::parse(&object(0, 1, 0, b"onlyname")).is_err());
    // A size of data past the end: the names that are there.
    let mut o = object(0, 1, 0, b"f\0d.dll\0");
    o[12..16].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    assert_eq!(ImportObject::parse(&o).unwrap().dll, b"d.dll");
    for len in 0..o.len() {
        let _ = ImportObject::parse(&o[..len]);
    }
}

/// Every type and name type, with and without a third name, written and
/// read back, byte for byte.
#[test]
fn writes_every_kind() {
    let kinds = [
        ImportType::Code,
        ImportType::Data,
        ImportType::Const,
        ImportType::Unknown(3),
    ];
    let name_types = [
        ImportNameType::Ordinal,
        ImportNameType::Name,
        ImportNameType::NoPrefix,
        ImportNameType::Undecorate,
        ImportNameType::ExportAs,
        ImportNameType::Unknown(5),
        ImportNameType::Unknown(6),
        ImportNameType::Unknown(7),
    ];
    for kind in kinds {
        for name_type in name_types {
            for export_name in [None, Some(&b"Exported"[..]), Some(&b""[..])] {
                let mut i = ImportObject::new(
                    0xAA64,
                    kind,
                    name_type,
                    42,
                    b"?Sym@@YAXXZ",
                    b"some.dll",
                    export_name,
                );
                i.time_date_stamp = 0xDEAD_BEEF;
                let bytes = i.to_vec().unwrap();
                assert_eq!(bytes.len() as u64, i.size());
                let back = ImportObject::parse(&bytes).unwrap();
                assert_eq!(back, i);
                assert_eq!(back.to_vec().unwrap(), bytes);
                let bits = u16::from_le_bytes([bytes[18], bytes[19]]);
                assert_eq!(bits & 3, kind.bits());
                assert_eq!((bits >> 2) & 7, name_type.bits());
            }
        }
    }
    // The header, field by field.
    let mut i = ImportObject::new(
        0x14C,
        ImportType::Data,
        ImportNameType::NoPrefix,
        7,
        b"_errno",
        b"msvcrt.dll",
        None,
    );
    i.version = 1;
    i.reserved = 0x7FF;
    let b = i.to_vec().unwrap();
    assert_eq!(&b[..4], [0, 0, 0xFF, 0xFF]);
    assert_eq!(u16::from_le_bytes([b[4], b[5]]), 1);
    assert_eq!(u16::from_le_bytes([b[6], b[7]]), 0x14C);
    assert_eq!(u32::from_le_bytes([b[12], b[13], b[14], b[15]]), 18);
    assert_eq!(u16::from_le_bytes([b[16], b[17]]), 7);
    assert_eq!(
        u16::from_le_bytes([b[18], b[19]]),
        1 | (2 << 2) | (0x7FF << 5)
    );
    assert_eq!(&b[20..], b"_errno\0msvcrt.dll\0");
    // (A version other than 0 isn't an import object's signature.)
    assert!(ImportObject::parse(&b).is_err());
    i.version = 0;
    let bytes = i.to_vec().unwrap();
    let back = ImportObject::parse(&bytes).unwrap();
    assert_eq!(back.reserved, 0x7FF);
}

#[test]
fn write_errors() {
    let i = ImportObject::new(
        0x8664,
        ImportType::Code,
        ImportNameType::Name,
        0,
        b"a\0b",
        b"x.dll",
        None,
    );
    assert_eq!(i.to_vec(), Err(ImportWriteError::NulInName));
    let i = ImportObject {
        export_name: Some(b"\0"),
        symbol: b"a",
        ..i
    };
    assert_eq!(i.to_vec(), Err(ImportWriteError::NulInName));
    for e in [ImportWriteError::NulInName, ImportWriteError::TooLarge] {
        assert!(!std::format!("{e}").is_empty());
    }
}

/// lib.exe's ARM64EC import by ordinal carries a third name too.
#[test]
fn third_name_of_any_type() {
    let o = object(0, 0, 4, b"#ExpOrd\0ecdll.dll\0ExpOrd\0");
    let i = ImportObject::parse(&o).unwrap();
    assert_eq!(i.export_name, Some(&b"ExpOrd"[..]));
    assert_eq!(i.import_name(), None);
    assert_eq!(i.to_vec().unwrap()[8..], o[8..]);
}

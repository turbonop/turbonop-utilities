extern crate std;

use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;

use turbonop_elf_object::{Data, Header, Object, Section, Symbol, sht};

use super::*;

fn section(name: &'static [u8], kind: u32, data: Data<'static>) -> Section<'static> {
    Section {
        name: Cow::Borrowed(name),
        name_offset: 0,
        kind,
        flags: 0,
        addr: 0,
        link: 0,
        info: 0,
        addralign: 1,
        entsize: 0,
        nobits_size: 0,
        data,
    }
}

/// An object of `machine` with these symbols (name, bind, type, section).
fn object(machine: u16, symbols: &[(&'static [u8], u8, u8, u32)]) -> Vec<u8> {
    let mut ident = [0u8; 16];
    ident[..4].copy_from_slice(b"\x7fELF");
    ident[4] = 2;
    ident[5] = 1;
    ident[6] = 1;
    let mut syms = vec![Symbol {
        name: Cow::Borrowed(b""),
        name_offset: 0,
        value: 0,
        size: 0,
        info: 0,
        other: 0,
        shndx: 0,
        xindex: false,
    }];
    for &(name, bind, kind, shndx) in symbols {
        syms.push(Symbol {
            name: Cow::Borrowed(name),
            info: (bind << 4) | kind,
            shndx,
            ..syms[0].clone()
        });
    }
    let first_global = syms
        .iter()
        .position(|s| s.bind() != 0)
        .unwrap_or(syms.len());
    let mut sections = vec![
        section(b"", 0, Data::NoBits),
        section(b".text", sht::PROGBITS, Data::Bytes(Cow::Borrowed(b"\xc3"))),
        section(b".symtab", sht::SYMTAB, Data::Symbols(syms)),
        section(b".strtab", sht::STRTAB, Data::Bytes(Cow::Borrowed(b"\0"))),
        section(b".shstrtab", sht::STRTAB, Data::Bytes(Cow::Borrowed(b"\0"))),
    ];
    sections[2].link = 3;
    sections[2].info = u32::try_from(first_global).unwrap();
    sections[2].entsize = 24;
    let mut o = Object::new(
        Header {
            ident,
            kind: 1,
            machine,
            version: 1,
            entry: 0,
            flags: 0,
        },
        sections,
    );
    o.shstrndx = 4;
    o.write().unwrap()
}

fn names(data: &[u8], archiver: Archiver) -> Vec<&[u8]> {
    member_symbols(data, archiver).unwrap().symbols
}

#[test]
fn which_symbols() {
    let o = object(
        62,
        &[
            (b"local", 0, 2, 1),
            (b"local_common", 0, 1, 0xFFF2),
            (b"global", 1, 2, 1),
            (b"weak", 2, 2, 1),
            (b"unique", 10, 1, 1),
            (b"absolute", 1, 0, 0xFFF1),
            (b"common", 1, 1, 0xFFF2),
            (b"large_common", 1, 1, 0xFF02),
            (b"undefined", 1, 0, 0),
            (b"weak_undefined", 2, 0, 0),
            (b"loos", 11, 2, 1),
            (b"file", 1, 4, 0xFFF1),
            (b"section", 1, 3, 1),
        ],
    );
    assert_eq!(
        names(&o, Archiver::GnuAr),
        [
            &b"local_common"[..],
            b"global",
            b"weak",
            b"unique",
            b"absolute",
            b"common",
            b"large_common",
            b"file",
            b"section"
        ]
    );
    assert_eq!(
        names(&o, Archiver::LlvmAr),
        [
            &b"global"[..],
            b"weak",
            b"unique",
            b"absolute",
            b"common",
            b"large_common",
            b"loos"
        ]
    );
    // Mapping symbols: llvm-ar leaves them out (AArch64's $x, $d).
    let a = object(183, &[(b"$x.0", 1, 0, 1), (b"f", 1, 2, 1)]);
    assert_eq!(names(&a, Archiver::LlvmAr), [&b"f"[..]]);
    assert_eq!(names(&a, Archiver::GnuAr), [&b"$x.0"[..], b"f"]);
    // On another machine, $x is a name like any other.
    let x = object(62, &[(b"$x.0", 1, 0, 1)]);
    assert_eq!(names(&x, Archiver::LlvmAr), [&b"$x.0"[..]]);
}

#[test]
fn objects_and_not() {
    let empty = member_symbols(b"plain text\n", Archiver::GnuAr).unwrap();
    assert!(!empty.object && empty.symbols.is_empty());
    let o = object(62, &[]);
    let s = member_symbols(&o, Archiver::LlvmAr).unwrap();
    assert!(s.object && s.symbols.is_empty());
    assert_eq!(
        member_symbols(b"BC\xC0\xDE\x35\x14", Archiver::LlvmAr),
        Err(SymbolsError::Unsupported("LLVM bitcode"))
    );
    let mut coff = vec![0u8; 20];
    coff[..2].copy_from_slice(&0x8664u16.to_le_bytes());
    assert_eq!(
        member_symbols(&coff, Archiver::GnuAr),
        Err(SymbolsError::Unsupported("a COFF object"))
    );
    // A damaged ELF file: not an object to GNU ar, an error to llvm-ar.
    let bad = b"\x7fELF\x02\x01\x01";
    assert!(!member_symbols(bad, Archiver::GnuAr).unwrap().object);
    assert_eq!(
        member_symbols(bad, Archiver::LlvmAr),
        Err(SymbolsError::Malformed)
    );
}

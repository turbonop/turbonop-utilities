use std::borrow::Cow;

use turbonop_archive::unix::{Dialect, Entry, NewMember, Options as ArOptions, write};
use turbonop_archive::write::{Fields, IndexSymbol};
use turbonop_elf_object::{Group, Header, SHN_UNDEF, Section, Symbol};
use turbonop_fold::elf::{self as fold, Input};

use super::*;

fn sec<'a>(name: &'a [u8], kind: u32, flags: u64, data: Data<'a>) -> Section<'a> {
    Section {
        name: Cow::Borrowed(name),
        name_offset: 0,
        kind,
        flags,
        addr: 0,
        link: 0,
        info: 0,
        addralign: 1,
        entsize: 0,
        nobits_size: 0,
        data,
    }
}

fn sym(name: &'static [u8], info: u8, other: u8, shndx: u32) -> Symbol<'static> {
    Symbol {
        name: Cow::Borrowed(name),
        name_offset: 0,
        value: 0,
        size: 0,
        info,
        other,
        shndx,
        xindex: false,
    }
}

/// A CIE and an FDE for each of `n` functions.
fn eh_frame(n: usize) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&[1, b'z', b'R', 0, 1, 0x78, 16, 1, 0x1B, 0x0C, 7, 8]);
    for _ in 0..n {
        let at = v.len();
        v.extend_from_slice(&20u32.to_le_bytes());
        v.extend_from_slice(&((at + 4) as u32).to_le_bytes());
        v.extend_from_slice(&[0, 0, 0, 0, 6, 0, 0, 0, 0, 0x41, 0x0E, 0x10, 0, 0, 0, 0]);
    }
    v
}

/// A member defining `own` (in `.text`), which calls the inline function
/// `_Z1fv` (a COMDAT group, calling `ext`), each with an FDE, and debug
/// information naming both through section symbols.
fn member(own: &'static [u8]) -> Vec<u8> {
    let mut ident = [0u8; 16];
    ident[..4].copy_from_slice(b"\x7fELF");
    ident[4] = 2;
    ident[5] = 1;
    ident[6] = 1;
    let header = Header {
        ident,
        kind: 1,
        machine: 62,
        version: 1,
        entry: 0,
        flags: 0,
    };
    let code: &'static [u8] = b"\xe8\0\0\0\0\xc3";
    let text = shf::ALLOC | shf::EXECINSTR;
    let mut sections = vec![
        sec(b"", 0, 0, Data::NoBits),
        sec(
            b".text",
            sht::PROGBITS,
            text,
            Data::Bytes(Cow::Borrowed(code)),
        ),
        sec(
            b".rela.text",
            sht::RELA,
            shf::INFO_LINK,
            Data::Relocations(Vec::new().into()),
        ),
        sec(
            b".group",
            sht::GROUP,
            0,
            Data::Group(Group {
                flags: 1,
                members: vec![4, 5],
            }),
        ),
        sec(
            b".text._Z1fv",
            sht::PROGBITS,
            text | shf::GROUP,
            Data::Bytes(Cow::Borrowed(code)),
        ),
        sec(
            b".rela.text._Z1fv",
            sht::RELA,
            shf::INFO_LINK | shf::GROUP,
            Data::Relocations(Vec::new().into()),
        ),
        sec(
            b".eh_frame",
            0x7000_0001,
            shf::ALLOC,
            Data::Bytes(Cow::Owned(eh_frame(2))),
        ),
        sec(
            b".rela.eh_frame",
            sht::RELA,
            shf::INFO_LINK,
            Data::Relocations(Vec::new().into()),
        ),
        sec(
            b".debug_info",
            sht::PROGBITS,
            0,
            Data::Bytes(Cow::Owned(vec![0xAA; 32])),
        ),
        sec(
            b".rela.debug_info",
            sht::RELA,
            shf::INFO_LINK,
            Data::Relocations(Vec::new().into()),
        ),
        sec(
            b".note.GNU-stack",
            sht::PROGBITS,
            0,
            Data::Bytes(Cow::Borrowed(b"")),
        ),
        sec(b".symtab", sht::SYMTAB, 0, Data::Symbols(Vec::new())),
        sec(
            b".strtab",
            sht::STRTAB,
            0,
            Data::Bytes(Cow::Borrowed(b"\0")),
        ),
        sec(
            b".shstrtab",
            sht::STRTAB,
            0,
            Data::Bytes(Cow::Borrowed(b"\0")),
        ),
    ];
    let symtab = 11u32;
    for s in &mut sections {
        if s.is_relocations() || s.kind == sht::GROUP {
            s.link = symtab;
            s.entsize = if s.kind == sht::GROUP { 4 } else { 24 };
            s.addralign = 8;
        }
    }
    sections[2].info = 1;
    sections[3].info = 4;
    sections[5].info = 4;
    sections[7].info = 6;
    sections[9].info = 8;
    sections[11].link = 12;
    sections[11].entsize = 24;
    sections[11].info = 4;
    sections[11].data = Data::Symbols(vec![
        sym(b"", 0, 0, 0),
        sym(b"", 3, 0, 1),
        sym(b"", 3, 0, 4),
        sym(b"", 3, 0, 6),
        Symbol {
            size: code.len() as u64,
            ..sym(b"_Z1fv", (2 << 4) | 2, 2, 4)
        },
        sym(own, (1 << 4) | 2, 0, 1),
        sym(b"ext", 1 << 4, 0, SHN_UNDEF),
    ]);
    let mut o = Object::new(header, sections);
    o.shstrndx = 13;
    let reloc = |o: &Object<'_>, offset: u64, kind: u32, symbol: u32| {
        let mut r = Relocation {
            offset,
            info: u64::from(kind),
            addend: if kind == 1 { 0 } else { -4 },
        };
        o.set_reloc_symbol(&mut r, symbol);
        r
    };
    let rels = [
        (2, vec![reloc(&o, 1, 4, 4)]),
        (5, vec![reloc(&o, 1, 4, 6)]),
        (
            7,
            vec![reloc(&o, 20 + 8, 2, 1), reloc(&o, 20 + 24 + 8, 2, 2)],
        ),
        (
            9,
            vec![reloc(&o, 0, 1, 1), reloc(&o, 8, 1, 2), reloc(&o, 16, 1, 4)],
        ),
    ];
    for (s, r) in rels {
        o.sections[s].data = Data::Relocations(r.into());
    }
    o.write().unwrap()
}

/// An archive of `members` (name, bytes), with GNU `ar`'s index.
fn archive(members: &[(Vec<u8>, Vec<u8>)]) -> Vec<u8> {
    let mut symbols = Vec::new();
    for (i, (_, d)) in members.iter().enumerate() {
        for s in member_symbols(d, Archiver::GnuAr).unwrap().symbols {
            symbols.push(IndexSymbol {
                name: Cow::Owned(s.to_vec()),
                member: i as u32,
            });
        }
    }
    let new: Vec<NewMember<'_>> = members
        .iter()
        .map(|(n, d)| NewMember {
            entry: Entry {
                name: n,
                origin: None,
                fields: Fields::dated(0, 0o644),
                size: d.len() as u64,
            },
            data: d,
        })
        .collect();
    write(
        &new,
        Some(&symbols),
        &ArOptions::new(Archiver::GnuAr, Dialect::Gnu),
    )
    .unwrap()
}

/// The original library of two members sharing `_Z1fv`, and its fold.
/// Members: (name, bytes).
type Members = Vec<(Vec<u8>, Vec<u8>)>;

fn folded() -> (Members, Members) {
    let orig = vec![
        (b"a.o".to_vec(), member(b"a")),
        (b"b.o".to_vec(), member(b"b")),
    ];
    let inputs: Vec<Input<'_>> = orig
        .iter()
        .map(|(n, d)| Input { name: n, data: d })
        .collect();
    let (out, report) = fold::fold(
        &inputs,
        &fold::Options {
            pull: fold::Pull::Preserve,
        },
    );
    assert_eq!(report.folded, 1);
    let new = out
        .iter()
        .map(|o| {
            let data = match (&o.data, o.input) {
                (Some(d), _) => d.clone(),
                (None, Some(i)) => orig[i].1.clone(),
                (None, None) => unreachable!(),
            };
            (o.name.clone(), data)
        })
        .collect();
    (orig, new)
}

fn check(orig: &[(Vec<u8>, Vec<u8>)], new: &[(Vec<u8>, Vec<u8>)]) -> Report {
    verify(&archive(orig), &archive(new), &Options::default()).unwrap()
}

#[test]
fn a_fold_verifies() {
    let (orig, new) = folded();
    let r = check(&orig, &new);
    assert!(r.passed(), "{}", r.text());
    assert_eq!(r.checked_count("units moved"), 2);
    assert_eq!(r.checked_count("synthesized members"), 1);
    assert_eq!(r.checked_count("debug relocations tombstoned"), 2);
    assert_eq!(r.checked_count("rewritten members"), 2);
}

#[test]
fn a_library_verifies_against_itself() {
    let (orig, _) = folded();
    let r = check(&orig, &orig);
    assert!(r.passed(), "{}", r.text());
    assert_eq!(r.checked_count("rewritten members"), 0);
}

#[test]
fn a_changed_synthesized_unit_fails() {
    let (orig, mut new) = folded();
    let synth = new.len() - 1;
    // The moved function's `ret` (0xC3, its last byte in the section).
    let at = new[synth]
        .1
        .windows(6)
        .position(|w| w == b"\xe8\0\0\0\0\xc3")
        .unwrap();
    new[synth].1[at + 5] = 0x90;
    let r = check(&orig, &new);
    assert!(r.failed("moved units no synthesized member it anchors holds"));
    assert!(r.failed("synthesized members' units that no member gave up"));
}

#[test]
fn a_missing_member_fails() {
    let (orig, mut new) = folded();
    new.remove(1);
    let r = check(&orig, &new);
    assert!(r.failed("original members missing (or out of order)"));
}

#[test]
fn a_debug_place_not_tombstoned_fails() {
    let (orig, mut new) = folded();
    let mut o = Object::parse(&new[0].1).unwrap().clone();
    let i = o
        .sections
        .iter()
        .position(|s| &*s.name == b".debug_info")
        .unwrap();
    let mut b = bytes(&o.sections[i].data).to_vec();
    // The place of the relocation to the moved function's section.
    b[8] = 0x55;
    o.sections[i].data = Data::Bytes(Cow::Owned(b));
    let bytes = o.write().unwrap();
    new[0].1 = bytes;
    let r = check(&orig, &new);
    assert!(
        r.failed("debug places of lost units' addresses that don't hold the tombstone"),
        "{}",
        r.text()
    );
}

#[test]
fn a_changed_member_outside_units_fails() {
    let (orig, mut new) = folded();
    let mut o = Object::parse(&new[1].1).unwrap().clone();
    let i = o
        .sections
        .iter()
        .position(|s| &*s.name == b".text")
        .unwrap();
    o.sections[i].data = Data::Bytes(Cow::Borrowed(b"\xe8\0\0\0\0\x90"));
    let bytes = o.write().unwrap();
    new[1].1 = bytes;
    let r = check(&orig, &new);
    assert!(r.failed("sections outside units whose bytes changed"));
}

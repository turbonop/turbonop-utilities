extern crate std;

use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;

use super::*;

fn section<'a>(name: &'a [u8], kind: u32, flags: u64, data: Data<'a>) -> Section<'a> {
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

fn sym<'a>(name: &'a [u8], info: u8, shndx: u32, value: u64) -> Symbol<'a> {
    Symbol {
        name: Cow::Borrowed(name),
        name_offset: 0,
        value,
        size: 4,
        info,
        other: 0,
        shndx,
        xindex: false,
    }
}

/// A small object: two text sections in a COMDAT group, data, a BSS, a
/// symbol table with locals and globals, relocations, and the string
/// tables, in `class` and `endian`.
fn object(class: Class, endian: Endian, machine: u16) -> Object<'static> {
    let mut ident = [0u8; 16];
    ident[..4].copy_from_slice(b"\x7fELF");
    ident[4] = if class == Class::Elf32 { 1 } else { 2 };
    ident[5] = if endian == Endian::Little { 1 } else { 2 };
    ident[6] = 1;
    let (symsize, relasize) = if class == Class::Elf32 {
        (16, 12)
    } else {
        (24, 24)
    };
    let mut sections = vec![
        Section {
            data: Data::NoBits,
            ..section(b"", sht::NULL, 0, Data::NoBits)
        },
        section(
            b".text",
            sht::PROGBITS,
            shf::ALLOC | shf::EXECINSTR,
            Data::Bytes(Cow::Borrowed(b"\x90\x90\xc3")),
        ),
        section(
            b".group",
            sht::GROUP,
            0,
            Data::Group(Group {
                flags: 1,
                members: vec![3, 4],
            }),
        ),
        section(
            b".text._Z1fv",
            sht::PROGBITS,
            shf::ALLOC | shf::EXECINSTR | shf::GROUP,
            Data::Bytes(Cow::Borrowed(b"\xe8\0\0\0\0\xc3")),
        ),
        section(
            b".rela.text._Z1fv",
            sht::RELA,
            shf::INFO_LINK | shf::GROUP,
            Data::Relocations(vec![]),
        ),
        section(
            b".data",
            sht::PROGBITS,
            shf::ALLOC | shf::WRITE,
            Data::Bytes(Cow::Borrowed(b"\x01\x02\x03\x04\x05")),
        ),
        Section {
            nobits_size: 64,
            addralign: 16,
            ..section(b".bss", sht::NOBITS, shf::ALLOC | shf::WRITE, Data::NoBits)
        },
        section(b".symtab", sht::SYMTAB, 0, Data::Symbols(vec![])),
        section(
            b".strtab",
            sht::STRTAB,
            0,
            Data::Bytes(Cow::Borrowed(b"\0")),
        ),
        section(
            b".shstrtab",
            sht::STRTAB,
            0,
            Data::Bytes(Cow::Borrowed(b"\0")),
        ),
    ];
    sections[2].link = 7;
    sections[2].info = 3;
    sections[2].addralign = 4;
    sections[2].entsize = 4;
    sections[3].addralign = 16;
    sections[4].link = 7;
    sections[4].info = 3;
    sections[4].addralign = 8;
    sections[4].entsize = relasize;
    sections[7].link = 8;
    sections[7].info = 3;
    sections[7].addralign = 8;
    sections[7].entsize = symsize;
    sections[7].data = Data::Symbols(vec![
        sym(b"", 0, 0, 0),
        sym(b"a.c", 4, SHN_ABS, 0),
        sym(b"", 3, 1, 0),
        sym(b"_Z1fv", (2 << 4) | 2, 3, 0),
        sym(b"global", (1 << 4) | 1, 5, 1),
        sym(b"common", (1 << 4) | 1, SHN_COMMON, 8),
        sym(b"undefined", 1 << 4, 0, 0),
    ]);
    let mut o = Object {
        header: Header {
            ident,
            kind: 1,
            machine,
            version: 1,
            entry: 0,
            flags: 0,
        },
        sections,
        shstrndx: 9,
        original: None,
    };
    let mut r = Relocation {
        offset: 1,
        info: 4,
        addend: -4,
    };
    o.set_reloc_symbol(&mut r, 6);
    o.sections[4].data = Data::Relocations(vec![r]);
    o
}

fn round_trip(o: &Object<'_>) {
    let bytes = o.write().unwrap();
    let back = Object::parse(&bytes).unwrap();
    assert!(back.keeps_layout());
    // The model read back is the model written, names and all.
    assert_eq!(back.header, o.header);
    assert_eq!(back.sections.len(), o.sections.len());
    for (a, b) in back.sections.iter().zip(&o.sections) {
        assert_eq!(a.name, b.name);
        assert_eq!(
            (a.kind, a.flags, a.link, a.info, a.addralign, a.entsize),
            (b.kind, b.flags, b.link, b.info, b.addralign, b.entsize)
        );
        match (&a.data, &b.data) {
            (Data::Symbols(x), Data::Symbols(y)) => {
                assert_eq!(x.len(), y.len());
                for (p, q) in x.iter().zip(y) {
                    assert_eq!(
                        (&p.name, p.value, p.size, p.info, p.other, p.shndx, p.xindex),
                        (&q.name, q.value, q.size, q.info, q.other, q.shndx, q.xindex)
                    );
                }
            }
            (Data::Bytes(_), Data::Bytes(_)) if a.kind == sht::STRTAB => {}
            (x, y) => assert_eq!(x, y),
        }
    }
    // And written again, it is the same file.
    assert_eq!(back.write().unwrap(), bytes);
    // Laid out afresh, the same too (the writer's own layout).
    let mut fresh = back.clone();
    fresh.relayout();
    assert_eq!(fresh.write().unwrap(), bytes);
}

#[test]
fn every_class_and_byte_order() {
    for (class, endian, machine) in [
        (Class::Elf64, Endian::Little, 62),
        (Class::Elf64, Endian::Big, 43),
        (Class::Elf32, Endian::Little, 3),
        (Class::Elf32, Endian::Big, 20),
        (Class::Elf64, Endian::Little, 8),
        (Class::Elf64, Endian::Big, 8),
    ] {
        let o = object(class, endian, machine);
        round_trip(&o);
        let bytes = o.write().unwrap();
        let back = Object::parse(&bytes).unwrap();
        let Data::Relocations(rs) = &back.sections[4].data else {
            panic!("relocations");
        };
        assert_eq!(back.reloc_symbol(&rs[0]), 6);
        assert_eq!(rs[0].addend, -4);
    }
}

#[test]
fn reader_agrees_with_turbonop_elf() {
    let o = object(Class::Elf64, Endian::Little, 62);
    let bytes = o.write().unwrap();
    let elf = turbonop_elf::ElfFile::parse(&bytes).unwrap();
    let names: Vec<&[u8]> = elf.sections().map(|s| s.name).collect();
    assert_eq!(names[3], b".text._Z1fv");
    let symbols = elf.all_symbols();
    assert_eq!(symbols[4].name, b"global");
    assert_eq!(symbols[4].shndx, 5);
    assert_eq!(symbols[5].shndx, 0xFFF2);
    let groups = elf.groups();
    assert_eq!(groups.len(), 1);
    let bss = elf.section(6).unwrap();
    assert_eq!((bss.size, bss.addralign, bss.offset % 16), (64, 16, 0));
}

#[test]
fn mips64_relocations() {
    for endian in [Endian::Little, Endian::Big] {
        let o = object(Class::Elf64, endian, 8);
        // r_type R_MIPS_GPREL16 (7), r_type2 R_MIPS_SUB (24), r_type3
        // R_MIPS_HI16 (5), symbol 6.
        let sym_bytes = match endian {
            Endian::Little => 6u32.to_le_bytes(),
            Endian::Big => 6u32.to_be_bytes(),
        };
        let raw = [
            sym_bytes[0],
            sym_bytes[1],
            sym_bytes[2],
            sym_bytes[3],
            0,
            5,
            24,
            7,
        ];
        let info = match endian {
            Endian::Little => u64::from_le_bytes(raw),
            Endian::Big => u64::from_be_bytes(raw),
        };
        let mut r = Relocation {
            offset: 0,
            info,
            addend: 0,
        };
        assert_eq!(o.reloc_symbol(&r), 6);
        assert_eq!(o.reloc_type(&r), 7 | (24 << 8) | (5 << 16));
        o.set_reloc_symbol(&mut r, 0x0102_0304);
        assert_eq!(o.reloc_symbol(&r), 0x0102_0304);
        assert_eq!(o.reloc_type(&r), 7 | (24 << 8) | (5 << 16));
    }
}

#[test]
fn names_changed_rebuild_the_tables() {
    let o = object(Class::Elf64, Endian::Little, 62);
    let bytes = o.write().unwrap();
    let mut back = Object::parse(&bytes).unwrap();
    assert!(back.keeps_layout());
    back.symbols_mut().unwrap()[4].name = Cow::Borrowed(b"renamed_global");
    back.sections[5].name = Cow::Borrowed(b".data.renamed");
    assert!(!back.keeps_layout());
    let again = back.write().unwrap();
    let reread = Object::parse(&again).unwrap();
    assert_eq!(&*reread.symbols()[4].name, b"renamed_global");
    assert_eq!(&*reread.sections[5].name, b".data.renamed");
    assert_eq!(&*reread.symbols()[6].name, b"undefined");
}

#[test]
fn a_shared_string_table() {
    // LLVM's: section names and symbol names in one `.strtab`.
    let mut o = object(Class::Elf64, Endian::Little, 62);
    o.sections.pop();
    o.shstrndx = 8;
    round_trip(&o);
    let bytes = o.write().unwrap();
    let back = Object::parse(&bytes).unwrap();
    assert_eq!(&*back.sections[8].name, b".strtab");
    assert_eq!(&*back.symbols()[3].name, b"_Z1fv");
}

#[test]
fn extended_numbering() {
    // More than 0xFF00 sections: e_shnum 0, the count in section 0, the
    // string table's index through SHN_XINDEX, and symbols in sections
    // past it through SHT_SYMTAB_SHNDX.
    let mut o = object(Class::Elf64, Endian::Little, 62);
    let strtab = o.sections.remove(8);
    let shstrtab = o.sections.remove(8);
    let symtab = o.sections.remove(7);
    for i in 0..0xFF10u32 {
        let name: &'static [u8] =
            alloc::boxed::Box::leak(alloc::format!(".text.f{i}").into_bytes().into_boxed_slice());
        o.sections.push(section(
            name,
            sht::PROGBITS,
            shf::ALLOC | shf::EXECINSTR,
            Data::Bytes(Cow::Borrowed(b"\xc3")),
        ));
    }
    let base = o.sections.len() as u32;
    let mut symtab = symtab;
    symtab.link = base + 2;
    if let Data::Symbols(s) = &mut symtab.data {
        let mut last = sym(b"far", (1 << 4) | 2, 0, 0);
        last.set_section(base - 1);
        assert!(last.xindex);
        s.push(last);
    }
    o.sections.push(symtab);
    let mut shndx = section(b".symtab_shndx", sht::SYMTAB_SHNDX, 0, Data::SymtabShndx);
    shndx.link = base;
    shndx.entsize = 4;
    shndx.addralign = 4;
    o.sections.push(shndx);
    o.sections.push(strtab);
    o.sections.push(shstrtab);
    o.shstrndx = base + 3;
    o.sections[2].link = base;
    o.sections[4].link = base;
    let bytes = o.write().unwrap();
    // e_shnum and e_shstrndx.
    assert_eq!(&bytes[60..64], &[0, 0, 0xFF, 0xFF]);
    let back = Object::parse(&bytes).unwrap();
    assert_eq!(back.sections.len(), o.sections.len());
    assert_eq!(back.shstrndx, base + 3);
    let far = back.symbols().last().unwrap();
    assert_eq!(
        (&*far.name, far.shndx, far.xindex),
        (&b"far"[..], base - 1, true)
    );
    assert_eq!(back.write().unwrap(), bytes);
    let elf = turbonop_elf::ElfFile::parse(&bytes).unwrap();
    assert_eq!(elf.all_symbols().last().unwrap().shndx, base - 1);
}

#[test]
fn retain_sections_renumbers() {
    let o = object(Class::Elf64, Endian::Little, 62);
    let bytes = o.write().unwrap();
    let mut back = Object::parse(&bytes).unwrap();
    // Drop the group and its members.
    let keep = [
        true, true, false, false, false, true, true, true, true, true,
    ];
    let map = back.retain_sections(&keep);
    assert_eq!(map[5], Some(2));
    assert_eq!(map[7], Some(4));
    assert_eq!(back.shstrndx, 6);
    assert_eq!(back.sections[4].link, 5);
    let s = back.symbols();
    assert_eq!(s[3].shndx, SHN_UNDEF);
    assert_eq!(s[4].shndx, 2);
    assert_eq!(s[2].shndx, 1);
    assert_eq!(s[5].shndx, SHN_COMMON);
    // Then the symbol that was in the group.
    let mut keep = vec![true; s.len()];
    keep[3] = false;
    let smap = back.retain_symbols(&keep);
    assert_eq!(smap[4], Some(3));
    assert_eq!(back.sections[4].info, 3);
    let out = back.write().unwrap();
    let reread = Object::parse(&out).unwrap();
    assert_eq!(reread.symbols().len(), 6);
    assert_eq!(&*reread.symbols()[3].name, b"global");
}

#[test]
fn not_objects() {
    assert_eq!(Object::parse(b"!<arch>\n").unwrap_err(), ReadError::NotElf);
    assert_eq!(
        Object::parse(b"\x7fELF\x03\x01").unwrap_err(),
        ReadError::NotElf
    );
    let o = object(Class::Elf64, Endian::Little, 62);
    let mut bytes = o.write().unwrap();
    // A section past the file.
    let n = bytes.len();
    bytes.truncate(n - 64 * 10 + 64 * 5);
    assert!(Object::parse(&bytes).is_err());
}

#[test]
fn rebuilt_tables_share_tails() {
    let names: [&[u8]; 6] = [
        b".text._Z1fv",
        b"_Z1fv",
        b".rela.text._Z1fv",
        b"",
        b"other",
        b"_Z1fv",
    ];
    let (table, offsets) = crate::write::build(names.iter().copied());
    // Each name once, the shorter ones inside `.rela.text._Z1fv`.
    assert_eq!(table, b"\0.rela.text._Z1fv\0other\0");
    for (n, &at) in names.iter().zip(&offsets) {
        let at = at as usize;
        assert_eq!(&table[at..at + n.len()], *n);
        assert_eq!(table[at + n.len()], 0);
    }
    assert_eq!(offsets[3], 0);
    assert_eq!(offsets[1], offsets[5]);
    // The same set in another order: the same bytes.
    let mut shuffled = names;
    shuffled.reverse();
    assert_eq!(crate::write::build(shuffled.iter().copied()).0, table);
}

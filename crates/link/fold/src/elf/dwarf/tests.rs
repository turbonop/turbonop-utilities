//! A member with DWARF 5 describing an inline function (in a COMDAT group)
//! and a function of its own; two such members folded; the synthesized
//! member's DWARF read back.

use std::borrow::Cow;

use turbonop_dwarf::{Abbrevs, Sections, Unit};
use turbonop_elf_object::{Data, Group, Header, Object, Relocation, Section, Symbol, shf, sht};

use crate::elf::{Input, Options, Pull, fold};

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

fn sym(name: &'static [u8], info: u8, other: u8, shndx: u32, size: u64) -> Symbol<'static> {
    Symbol {
        name: Cow::Borrowed(name),
        name_offset: 0,
        value: 0,
        size,
        info,
        other,
        shndx,
        xindex: false,
    }
}

// Section indexes of the member.
const TEXT: u32 = 1;
const F: u32 = 3;
const ABBREV: u32 = 4;
const INFO: u32 = 5;
const STR: u32 = 7;
const LINE: u32 = 8;
// Symbol indexes.
const S_TEXT: u32 = 1;
const S_F: u32 = 2;
const S_ABBREV: u32 = 3;
const S_STR: u32 = 4;
const S_LINE: u32 = 5;

const R_64: u32 = 1;
const R_32: u32 = 10;

fn uleb(v: &mut Vec<u8>, x: u64) {
    crate::elf::put_uleb(v, x);
}

/// The abbreviations, the unit, and its relocations: (offset, type,
/// symbol, addend).
#[allow(clippy::type_complexity)]
fn debug_info(own: &'static [u8]) -> (Vec<u8>, Vec<u8>, Vec<(u64, u32, u32, i64)>) {
    let mut ab = Vec::new();
    // 1: the unit (name, language, low_pc, stmt_list).
    ab.extend_from_slice(&[
        1, 0x11, 1, 0x03, 0x08, 0x13, 0x0b, 0x11, 0x01, 0x10, 0x17, 0, 0,
    ]);
    // 2: a base type (name, byte_size, encoding).
    ab.extend_from_slice(&[2, 0x24, 0, 0x03, 0x08, 0x0b, 0x0b, 0x3e, 0x0b, 0, 0]);
    // 3: a subprogram with children (name, low_pc, high_pc, type, sibling).
    ab.extend_from_slice(&[
        3, 0x2e, 1, 0x03, 0x08, 0x11, 0x01, 0x12, 0x07, 0x49, 0x13, 0x01, 0x13, 0, 0,
    ]);
    // 4: a parameter (name strp, type, location exprloc).
    ab.extend_from_slice(&[4, 0x05, 0, 0x03, 0x0e, 0x49, 0x13, 0x02, 0x18, 0, 0]);
    // 5: a subprogram without (name, low_pc, high_pc).
    ab.extend_from_slice(&[5, 0x2e, 0, 0x03, 0x08, 0x11, 0x01, 0x12, 0x07, 0, 0]);
    ab.push(0);

    let mut d = vec![0, 0, 0, 0, 5, 0, 1, 8, 0, 0, 0, 0];
    let mut rels = vec![(8, R_32, S_ABBREV, 0)];
    d.push(1);
    d.extend_from_slice(b"t.cpp\0");
    d.push(0x21);
    d.extend_from_slice(&[0; 8]);
    rels.push((d.len() as u64, R_32, S_LINE, 0));
    d.extend_from_slice(&[0; 4]);
    let int = d.len() as u32;
    d.push(2);
    d.extend_from_slice(b"int\0");
    d.extend_from_slice(&[4, 5]);
    d.push(5);
    d.extend_from_slice(own);
    d.push(0);
    rels.push((d.len() as u64, R_64, S_TEXT, 0));
    d.extend_from_slice(&[0; 8]);
    d.extend_from_slice(&1u64.to_le_bytes());
    d.push(3);
    d.extend_from_slice(b"_Z1fv\0");
    rels.push((d.len() as u64, R_64, S_F, 0));
    d.extend_from_slice(&[0; 8]);
    d.extend_from_slice(&2u64.to_le_bytes());
    d.extend_from_slice(&int.to_le_bytes());
    let sibling_at = d.len();
    d.extend_from_slice(&[0; 4]);
    d.push(4);
    rels.push((d.len() as u64, R_32, S_STR, 0));
    d.extend_from_slice(&[0; 4]);
    d.extend_from_slice(&int.to_le_bytes());
    d.extend_from_slice(&[2, 0x91, 0x6c]);
    d.push(0);
    let after = d.len() as u32;
    d[sibling_at..sibling_at + 4].copy_from_slice(&after.to_le_bytes());
    d.push(0);
    let len = (d.len() - 4) as u32;
    d[..4].copy_from_slice(&len.to_le_bytes());
    (ab, d, rels)
}

/// A DWARF 5 line table with a sequence for `.text` and one for the inline
/// function (at line 5).
#[allow(clippy::type_complexity)]
fn debug_line() -> (Vec<u8>, Vec<(u64, u32, u32, i64)>) {
    let mut h = vec![1, 1, 1, (-5i8) as u8, 14, 13];
    h.extend_from_slice(&[0, 1, 1, 1, 1, 0, 0, 0, 1, 0, 0, 1]);
    h.extend_from_slice(&[1, 1, 0x08]);
    uleb(&mut h, 1);
    h.extend_from_slice(b"/src\0");
    h.extend_from_slice(&[2, 1, 0x08, 2, 0x0f]);
    uleb(&mut h, 1);
    h.extend_from_slice(b"t.cpp\0");
    h.push(0);
    let mut d = vec![0, 0, 0, 0, 5, 0, 8, 0];
    d.extend_from_slice(&(h.len() as u32).to_le_bytes());
    d.extend_from_slice(&h);
    let mut rels = Vec::new();
    for (s, advance) in [(S_TEXT, 0u8), (S_F, 4)] {
        d.extend_from_slice(&[0, 9, 2]);
        rels.push((d.len() as u64, R_64, s, 0));
        d.extend_from_slice(&[0; 8]);
        if advance != 0 {
            d.extend_from_slice(&[0x03, advance]);
        }
        d.extend_from_slice(&[0x01, 0x02, 0x01, 0, 1, 1]);
    }
    let len = (d.len() - 4) as u32;
    d[..4].copy_from_slice(&len.to_le_bytes());
    (d, rels)
}

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
    let code = shf::ALLOC | shf::EXECINSTR;
    let (ab, info, info_rels) = debug_info(own);
    let (line, line_rels) = debug_line();
    let mut sections = vec![
        sec(b"", 0, 0, Data::NoBits),
        sec(
            b".text",
            sht::PROGBITS,
            code,
            Data::Bytes(Cow::Borrowed(b"\xc3")),
        ),
        sec(
            b".group",
            sht::GROUP,
            0,
            Data::Group(Group {
                flags: 1,
                members: vec![F],
            }),
        ),
        sec(
            b".text._Z1fv",
            sht::PROGBITS,
            code | shf::GROUP,
            Data::Bytes(Cow::Borrowed(b"\x55\xc3")),
        ),
        sec(
            b".debug_abbrev",
            sht::PROGBITS,
            0,
            Data::Bytes(Cow::Owned(ab)),
        ),
        sec(
            b".debug_info",
            sht::PROGBITS,
            0,
            Data::Bytes(Cow::Owned(info)),
        ),
        sec(b".rela.debug_info", sht::RELA, shf::INFO_LINK, Data::NoBits),
        sec(
            b".debug_str",
            sht::PROGBITS,
            shf::MERGE | shf::STRINGS,
            Data::Bytes(Cow::Borrowed(b"x\0")),
        ),
        sec(
            b".debug_line",
            sht::PROGBITS,
            0,
            Data::Bytes(Cow::Owned(line)),
        ),
        sec(b".rela.debug_line", sht::RELA, shf::INFO_LINK, Data::NoBits),
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
        if s.kind == sht::RELA || s.kind == sht::GROUP {
            s.link = symtab;
            s.entsize = if s.kind == sht::GROUP { 4 } else { 24 };
            s.addralign = 8;
        }
    }
    sections[2].info = 6; // the signature: _Z1fv
    sections[6].info = INFO;
    sections[9].info = LINE;
    let t = symtab as usize;
    sections[t].link = symtab + 1;
    sections[t].entsize = 24;
    sections[t].info = 6;
    sections[t].data = Data::Symbols(vec![
        sym(b"", 0, 0, 0, 0),
        sym(b"", 3, 0, TEXT, 0),
        sym(b"", 3, 0, F, 0),
        sym(b"", 3, 0, ABBREV, 0),
        sym(b"", 3, 0, STR, 0),
        sym(b"", 3, 0, LINE, 0),
        sym(b"_Z1fv", (2 << 4) | 2, 0, F, 2),
        sym(own, (1 << 4) | 2, 0, TEXT, 1),
    ]);
    let mut o = Object::new(header, sections);
    o.shstrndx = o.sections.len() as u32 - 1;
    let rel = |o: &Object<'_>, (offset, kind, symbol, addend): (u64, u32, u32, i64)| {
        let mut r = Relocation {
            offset,
            info: u64::from(kind),
            addend,
        };
        o.set_reloc_symbol(&mut r, symbol);
        r
    };
    let ri: Vec<Relocation> = info_rels.into_iter().map(|x| rel(&o, x)).collect();
    let rl: Vec<Relocation> = line_rels.into_iter().map(|x| rel(&o, x)).collect();
    o.sections[6].data = Data::Relocations(ri);
    o.sections[9].data = Data::Relocations(rl);
    o.write().unwrap()
}

fn relocations_for<'o>(o: &'o Object<'_>, section: usize) -> &'o [Relocation] {
    match o
        .sections
        .iter()
        .find(|s| s.is_relocations() && s.info as usize == section)
        .map(|s| &s.data)
    {
        Some(Data::Relocations(rs)) => rs,
        _ => &[],
    }
}

fn index_of(o: &Object<'_>, name: &[u8]) -> usize {
    o.sections.iter().position(|s| &*s.name == name).unwrap()
}

fn bytes(o: &Object<'_>, i: usize) -> Vec<u8> {
    match &o.sections[i].data {
        Data::Bytes(b) => b.to_vec(),
        _ => Vec::new(),
    }
}

/// A debug section's bytes with its relocations to other debug sections
/// applied (their addends), and the others: (offset, section of the symbol
/// named).
fn applied(o: &Object<'_>, name: &[u8]) -> (Vec<u8>, Vec<(u64, usize)>) {
    let i = index_of(o, name);
    let mut b = bytes(o, i);
    let mut other = Vec::new();
    for r in relocations_for(o, i) {
        let s = &o.symbols()[o.reloc_symbol(r) as usize];
        let target = s.shndx as usize;
        let n = if o.reloc_type(r) == R_32 { 4 } else { 8 };
        if o.sections[target].name.starts_with(b".debug_") {
            let v = (r.addend as u64).to_le_bytes();
            b[r.offset as usize..r.offset as usize + n].copy_from_slice(&v[..n]);
        } else {
            other.push((r.offset, target));
        }
    }
    (b, other)
}

fn fold_two() -> (Vec<crate::elf::Output>, crate::elf::Report) {
    let members = [member(b"a"), member(b"b")];
    let names = [b"a.o".to_vec(), b"b.o".to_vec()];
    let inputs: Vec<Input<'_>> = names
        .iter()
        .zip(&members)
        .map(|(n, d)| Input { name: n, data: d })
        .collect();
    fold(
        &inputs,
        &Options {
            pull: Pull::Preserve,
        },
    )
}

#[test]
fn a_folded_functions_dwarf_moves() {
    let (out, report) = fold_two();
    assert_eq!((report.folded, report.synthesized), (1, 1));
    assert_eq!((report.debug_moved, report.debug_dropped), (1, 0));
    assert_eq!(report.debug_roots, 1);
    assert_eq!(report.debug_tombstoned, 0);
    let synth = Object::parse(out[2].data.as_ref().unwrap()).unwrap();
    let text = index_of(&synth, b".text._Z1fv");

    // The unit: its DIE, the inline function and what it uses, not `own`.
    let (info, info_other) = applied(&synth, b".debug_info");
    let abbrev = bytes(&synth, index_of(&synth, b".debug_abbrev"));
    let (line, line_other) = applied(&synth, b".debug_line");
    let (rng, rng_other) = applied(&synth, b".debug_rnglists");
    let str_ = bytes(&synth, index_of(&synth, b".debug_str"));
    let mut s = Sections::new(turbonop_core::Endian::Little);
    s.info = &info;
    s.abbrev = &abbrev;
    s.line = &line;
    s.str = &str_;
    s.rnglists = &rng;
    let units: Vec<Unit<'_>> = Unit::iter(s, turbonop_dwarf::UnitSection::Info, 0).collect();
    assert_eq!(units.len(), 1);
    let u = units[0];
    assert_eq!((u.version, u.address_size), (5, 8));
    let abbrevs = Abbrevs::parse(&abbrev, u.abbrev_offset, s.endian).unwrap();
    let top = u.entry(&abbrevs, u.dies).unwrap();
    assert_eq!(top.tag, 0x11);
    let name = |d: &turbonop_dwarf::Die<'_>| {
        d.attr(0x03)
            .and_then(|a| u.string(&a.value))
            .map(|r| r.bytes.to_vec())
    };
    assert_eq!(name(&top).unwrap(), b"t.cpp");
    let children = u.children(&abbrevs, &top);
    let names: Vec<Vec<u8>> = children.iter().filter_map(name).collect();
    assert_eq!(names, [b"int".to_vec(), b"_Z1fv".to_vec()]);
    let f = &children[1];
    assert!(f.attr(0x01).is_none(), "no DW_AT_sibling");
    // Its type is the copied base type.
    let ty = u.reference(f.attr(0x49).unwrap()).unwrap();
    assert_eq!(ty, children[0].offset);
    // Its parameter, by an indirect string, with its location.
    let params = u.children(&abbrevs, f);
    assert_eq!(params.len(), 1);
    assert_eq!(name(&params[0]).unwrap(), b"x");
    assert_eq!(
        params[0].attr(0x02).unwrap().value,
        turbonop_dwarf::Value::Block(&[0x91, 0x6c])
    );
    // Its low_pc and the unit's range name the copied section: the only
    // relocations to code.
    let targets: Vec<usize> = info_other.iter().map(|&(_, t)| t).collect();
    assert_eq!(targets, [text]);
    assert_eq!(
        rng_other.iter().map(|&(_, t)| t).collect::<Vec<_>>(),
        [text]
    );
    // The line table: the inline function's sequence only.
    assert_eq!(
        line_other.iter().map(|&(_, t)| t).collect::<Vec<_>>(),
        [text]
    );
    let stmt = top.attr(0x10).and_then(|a| u.uint(&a.value)).unwrap();
    let lp = turbonop_dwarf::line_program(&s, stmt, 8).unwrap();
    let lines: Vec<u64> = lp.rows.iter().map(|r| r.line).collect();
    assert_eq!(lines, [5, 5]);

    // The givers: the inline function's low_pc and line sequence
    // tombstoned, `own`'s kept.
    for o in &out[..2] {
        let obj = Object::parse(o.data.as_ref().unwrap()).unwrap();
        let (_, other) = applied(&obj, b".debug_info");
        assert_eq!(other.len(), 1);
        let (_, other) = applied(&obj, b".debug_line");
        assert_eq!(other.len(), 1);
    }
    // Deterministic.
    let (again, _) = fold_two();
    assert_eq!(out[2].data, again[2].data);
}

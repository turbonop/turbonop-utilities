//! `--locals=copy`'s analysis on objects made here: what a unit reaches,
//! what can be copied, and the structural identity.

use super::super::units::{self, Mode, find_with};
use super::*;
use std::borrow::Cow;
use turbonop_coff::{
    File, Name, Relocation, Section, SectionData, SectionDefinition, Symbol, comdat_select,
};

const COPY: Mode = Mode {
    copy_locals: true,
    structural: false,
};
const COMDAT: u32 = 0x1000;
const CODE: u32 = 0x6050_0020;
const RDATA: u32 = 0x4030_0040;
const DATA: u32 = 0xC030_0040;
/// REL32 (x64).
const REL32: u16 = 4;

fn object(data: &[u8]) -> Object<'_> {
    match File::parse(data).unwrap() {
        File::Object(o) => o,
        File::Opaque { .. } => panic!("not a COFF object"),
    }
}

/// A COMDAT section (selection `ANY`) with its section symbol and a symbol
/// `name` (external or static), holding `code`.
pub(crate) fn comdat(
    o: &mut Object<'static>,
    name: &[u8],
    external: bool,
    code: Vec<u8>,
) -> (SectionId, SymbolId) {
    let id = o.push_section(Section::new(
        Name::new(b".text$mn".to_vec()),
        CODE | COMDAT,
        SectionData::Bytes(Cow::Owned(code)),
    ));
    let mut s = Symbol::new(
        Name::new(b".text$mn".to_vec()),
        0,
        SectionNumber::Section(id),
        0,
        storage_class::STATIC,
    );
    s.aux = Aux::Section(SectionDefinition {
        selection: comdat_select::ANY,
        ..Default::default()
    });
    o.push_symbol(s);
    let class = if external {
        storage_class::EXTERNAL
    } else {
        storage_class::STATIC
    };
    let f = o.push_symbol(Symbol::new(
        Name::new(name.to_vec()),
        0,
        SectionNumber::Section(id),
        0x20,
        class,
    ));
    (id, f)
}

/// A section outside COMDATs holding `bytes` at `at` (after `at` bytes of
/// other data) and then `pad` zeros, with a static symbol `name` there and
/// another symbol after it, as a compiler lays out string literals.
pub(crate) fn data(
    o: &mut Object<'static>,
    section: &[u8],
    characteristics: u32,
    name: &[u8],
    at: u32,
    bytes: &[u8],
    pad: usize,
) -> SymbolId {
    let mut v = vec![0x55; at as usize];
    v.extend_from_slice(bytes);
    v.extend(std::iter::repeat_n(0, pad));
    let end = v.len() as u32;
    v.extend_from_slice(b"next\0\0\0\0");
    let id = o.push_section(Section::new(
        Name::new(section.to_vec()),
        characteristics,
        SectionData::Bytes(Cow::Owned(v)),
    ));
    let mut s = Symbol::new(
        Name::new(section.to_vec()),
        0,
        SectionNumber::Section(id),
        0,
        storage_class::STATIC,
    );
    s.aux = Aux::Section(SectionDefinition::default());
    o.push_symbol(s);
    if at > 0 {
        o.push_symbol(Symbol::new(
            Name::new(b"$before".to_vec()),
            0,
            SectionNumber::Section(id),
            0,
            storage_class::STATIC,
        ));
    }
    let sym = o.push_symbol(Symbol::new(
        Name::new(name.to_vec()),
        at,
        SectionNumber::Section(id),
        0,
        storage_class::STATIC,
    ));
    o.push_symbol(Symbol::new(
        Name::new(b"$after".to_vec()),
        end,
        SectionNumber::Section(id),
        0,
        storage_class::STATIC,
    ));
    sym
}

pub(crate) fn reloc(o: &mut Object<'_>, section: SectionId, offset: u32, symbol: SymbolId) {
    o.sections[section.index()].relocations.push(Relocation {
        offset,
        symbol,
        kind: REL32,
    });
}

pub(crate) fn build(f: impl FnOnce(&mut Object<'static>)) -> Vec<u8> {
    build_for(0x8664, f)
}

fn build_for(machine: u16, f: impl FnOnce(&mut Object<'static>)) -> Vec<u8> {
    let mut o = Object::new(turbonop_coff::Format::Regular, machine);
    f(&mut o);
    o.sync_section_definitions();
    o.write().unwrap()
}

/// `f` reading a string `name` (at `at` in its section, then `pad`
/// zeros) by an instruction whose displacement's addend is `addend`.
pub(crate) fn string_user(name: &[u8], at: u32, pad: usize, addend: i32) -> Vec<u8> {
    build(|o| {
        let mut code = vec![0x48, 0x8D, 0x05];
        code.extend_from_slice(&addend.to_le_bytes());
        code.push(0xC3);
        let (text, _) = comdat(o, b"f", true, code);
        let s = data(o, b".rdata", RDATA, name, at, b"hello\0", pad);
        reloc(o, text, 3, s);
    })
}

fn unit_named<'f>(f: &'f Facts, name: &[u8]) -> &'f Unit {
    f.units
        .iter()
        .find(|u| u.externals.iter().any(|n| n == name))
        .unwrap()
}

#[test]
fn a_string_is_a_piece_copied_with_its_unit() {
    let data = string_user(b"$SG1", 0, 2, 0);
    let o = object(&data);
    // Kept, it doesn't fold.
    let keep = units::find(&o).unwrap();
    assert_eq!(keep.units[0].unfoldable, Some(Unfoldable::LocalReference));
    let f = find_with(&o, COPY).unwrap();
    let u = &f.units[0];
    assert_eq!(u.unfoldable, None, "{:?}", u.detail);
    assert_eq!(u.edges.len(), 1);
    assert_eq!(f.pieces.len(), 1);
    let p = &f.pieces[0];
    assert_eq!((p.start, p.end), (0, 8), "the string and its padding");
    assert_eq!(u.closure, vec![Node::Unit(0), Node::Piece(0)]);
}

#[test]
fn the_same_string_elsewhere_is_the_same_unit() {
    // Another name, another place, other padding: the same code.
    let a = string_user(b"$SG1", 0, 2, 0);
    let b = string_user(b"$SG77", 12, 6, 0);
    let (oa, ob) = (object(&a), object(&b));
    let ia = find_with(&oa, COPY).unwrap().units[0].identity;
    let ib = find_with(&ob, COPY).unwrap().units[0].identity;
    assert_eq!(ia, ib);
    // Kept, the names differ, so the identities do.
    assert_ne!(
        units::find(&oa).unwrap().units[0].identity,
        units::find(&ob).unwrap().units[0].identity
    );
    // Other bytes are another unit.
    let c = build(|o| {
        let (text, _) = comdat(o, b"f", true, vec![0x48, 0x8D, 0x05, 0, 0, 0, 0, 0xC3]);
        let s = data(o, b".rdata", RDATA, b"$SG1", 0, b"hellO\0", 2);
        reloc(o, text, 3, s);
    });
    let ic = find_with(&object(&c), COPY).unwrap().units[0].identity;
    assert_ne!(ia, ic);
}

#[test]
fn an_addend_outside_the_piece_holds_it_back() {
    let inside = string_user(b"$SG1", 0, 2, 5);
    let f = find_with(&object(&inside), COPY).unwrap();
    assert_eq!(f.units[0].unfoldable, None, "{:?}", f.units[0].detail);
    for addend in [-1, 100] {
        let outside = string_user(b"$SG1", 0, 2, addend);
        let f = find_with(&object(&outside), COPY).unwrap();
        assert_eq!(
            f.units[0].unfoldable,
            Some(Unfoldable::LocalReference),
            "{addend}"
        );
    }
}

#[test]
fn writable_locals_are_never_copied() {
    let d = build(|o| {
        let (text, _) = comdat(o, b"f", true, vec![0x48, 0x8D, 0x05, 0, 0, 0, 0, 0xC3]);
        let s = data(o, b".data", DATA, b"counter", 0, &[0; 4], 0);
        reloc(o, text, 3, s);
    });
    let f = find_with(&object(&d), COPY).unwrap();
    assert_eq!(f.units[0].unfoldable, Some(Unfoldable::WritableLocal));
}

#[test]
fn code_and_section_symbols_outside_comdats_are_never_copied() {
    // A static function in a section of code outside COMDATs.
    let d = build(|o| {
        let (text, _) = comdat(o, b"f", true, vec![0xE8, 0, 0, 0, 0, 0xC3]);
        let s = data(o, b".text$mn", CODE, b"helper", 0, &[0xC3], 0);
        reloc(o, text, 1, s);
    });
    let f = find_with(&object(&d), COPY).unwrap();
    assert_eq!(f.units[0].unfoldable, Some(Unfoldable::LocalReference));
    // A place in .rdata named by its section symbol.
    let d = build(|o| {
        let (text, _) = comdat(o, b"f", true, vec![0x48, 0x8D, 0x05, 0, 0, 0, 0, 0xC3]);
        data(o, b".rdata", RDATA, b"$SG1", 0, b"x\0", 0);
        let section_symbol = SymbolId(o.symbols.len() as u32 - 3);
        assert!(matches!(
            o.symbols[section_symbol.index()].aux,
            Aux::Section(_)
        ));
        reloc(o, text, 3, section_symbol);
    });
    let f = find_with(&object(&d), COPY).unwrap();
    assert_eq!(f.units[0].unfoldable, Some(Unfoldable::LocalReference));
}

#[test]
fn other_machines_relocations_are_not_read() {
    let d = build_for(0xAA64, |o| {
        let (text, _) = comdat(o, b"f", true, vec![0; 8]);
        let s = data(o, b".rdata", RDATA, b"$SG1", 0, b"x\0", 0);
        // ARM64's PAGEBASE_REL21.
        o.sections[text.index()].relocations.push(Relocation {
            offset: 0,
            symbol: s,
            kind: 4,
        });
    });
    let f = find_with(&object(&d), COPY).unwrap();
    assert_eq!(f.units[0].unfoldable, Some(Unfoldable::LocalReference));
    assert!(
        f.units[0]
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("machine"),
        "{:?}",
        f.units[0].detail
    );
}

/// `f` calling a static COMDAT `h`; `extra` adds more.
pub(crate) fn caller(
    h_name: &[u8],
    extra: impl FnOnce(&mut Object<'static>, SectionId, SymbolId),
) -> Vec<u8> {
    build(|o| {
        let (text, _) = comdat(o, b"f", true, vec![0xE8, 0, 0, 0, 0, 0xC3]);
        let (htext, h) = comdat(o, h_name, false, vec![0x90, 0xC3]);
        reloc(o, text, 1, h);
        extra(o, htext, h);
    })
}

#[test]
fn a_local_comdat_is_copied_with_what_reaches_it() {
    let d = caller(b"?h@?A0x1234@@YAXXZ", |_, _, _| {});
    let o = object(&d);
    let f = find_with(&o, COPY).unwrap();
    let root = unit_named(&f, b"f");
    assert_eq!(root.unfoldable, None, "{:?}", root.detail);
    assert_eq!(root.closure, vec![Node::Unit(0), Node::Unit(1)]);
    let h = &f.units[1];
    assert!(h.pure_local);
    assert!(!h.pinned);
    assert_eq!(h.referrers, vec![0]);
    assert_eq!(h.blocked, None);
    // Never folded by itself.
    assert_eq!(h.unfoldable, Some(Unfoldable::InternalLinkage));
    // Another translation unit's anonymous namespace: the same code.
    let e = caller(b"?h@?A0xbeef@@YAXXZ", |_, _, _| {});
    let g = find_with(&object(&e), COPY).unwrap();
    assert_eq!(unit_named(&g, b"f").identity, root.identity);
}

#[test]
fn a_local_comdat_that_code_outside_units_reaches_is_pinned() {
    let d = caller(b"h", |o, _, h| {
        // A function outside COMDATs (no /Gy) calling it too.
        let g = o.push_section(Section::new(
            Name::new(b".text$mn".to_vec()),
            CODE,
            SectionData::Bytes(Cow::Owned(vec![0xE8, 0, 0, 0, 0, 0xC3])),
        ));
        reloc(o, g, 1, h);
    });
    let f = find_with(&object(&d), COPY).unwrap();
    assert!(f.units[1].pinned);
    // It can still be copied.
    assert_eq!(unit_named(&f, b"f").unfoldable, None);
}

#[test]
fn registration_sections_are_never_copied() {
    let d = build(|o| {
        let (text, _) = comdat(o, b"f", true, vec![0x48, 0x8B, 0x05, 0, 0, 0, 0, 0xC3]);
        let id = o.push_section(Section::new(
            Name::new(b".rtc$IMZ".to_vec()),
            RDATA | COMDAT,
            SectionData::Bytes(Cow::Owned(vec![0; 8])),
        ));
        let mut s = Symbol::new(
            Name::new(b".rtc$IMZ".to_vec()),
            0,
            SectionNumber::Section(id),
            0,
            storage_class::STATIC,
        );
        s.aux = Aux::Section(SectionDefinition {
            selection: comdat_select::ANY,
            ..Default::default()
        });
        o.push_symbol(s);
        let r = o.push_symbol(Symbol::new(
            Name::new(b"_RTC_InitBase.rtc$IMZ".to_vec()),
            0,
            SectionNumber::Section(id),
            0,
            storage_class::STATIC,
        ));
        reloc(o, text, 3, r);
    });
    let f = find_with(&object(&d), COPY).unwrap();
    let rtc = f.units.iter().find(|u| u.pure_local).unwrap();
    assert_eq!(
        rtc.blocked.as_ref().map(|b| b.0),
        Some(Unfoldable::LocalReference)
    );
    assert_eq!(
        unit_named(&f, b"f").unfoldable,
        Some(Unfoldable::LocalReference),
        "it reaches what can't be copied"
    );
}

#[test]
fn debug_information_that_names_a_local_outside_holds_a_unit_back() {
    let d = build(|o| {
        let (text, _) = comdat(o, b"f", true, vec![0xC3]);
        let s = data(o, b".rdata", RDATA, b"$SG1", 0, b"x\0", 0);
        let debug = o.push_section(Section::new(
            Name::new(b".debug$S".to_vec()),
            0x4210_0040 | COMDAT,
            SectionData::Bytes(Cow::Owned(vec![4, 0, 0, 0, 0, 0, 0, 0])),
        ));
        let mut ds = Symbol::new(
            Name::new(b".debug$S".to_vec()),
            0,
            SectionNumber::Section(debug),
            0,
            storage_class::STATIC,
        );
        ds.aux = Aux::Section(SectionDefinition {
            selection: comdat_select::ASSOCIATIVE,
            number: Some(text),
            ..Default::default()
        });
        o.push_symbol(ds);
        o.sections[debug.index()].relocations.push(Relocation {
            offset: 4,
            symbol: s,
            kind: 0xB,
        });
    });
    let f = find_with(&object(&d), COPY).unwrap();
    assert_eq!(f.units[0].unfoldable, Some(Unfoldable::LocalDebug));
}

#[test]
fn a_local_of_a_unit_with_external_symbols_is_never_copied() {
    let d = build(|o| {
        let (text, _) = comdat(o, b"f", true, vec![0xE8, 0, 0, 0, 0, 0xC3]);
        let (gtext, _) = comdat(o, b"g", true, vec![0x90, 0x90, 0xC3]);
        let label = o.push_symbol(Symbol::new(
            Name::new(b"$LN2".to_vec()),
            1,
            SectionNumber::Section(gtext),
            0,
            storage_class::LABEL,
        ));
        reloc(o, text, 1, label);
    });
    let f = find_with(&object(&d), COPY).unwrap();
    assert_eq!(
        unit_named(&f, b"f").unfoldable,
        Some(Unfoldable::LocalReference)
    );
    assert_eq!(
        unit_named(&f, b"g").unfoldable,
        Some(Unfoldable::ReferencedLocally)
    );
}

#[test]
fn references_name_what_a_copy_needs_resolved() {
    let make = |h_refers: bool| {
        build(move |o| {
            let (text, _) = comdat(o, b"f", true, vec![0xE8, 0, 0, 0, 0, 0xE8, 0, 0, 0, 0]);
            let x = o.push_symbol(Symbol::new(
                Name::new(b"x".to_vec()),
                0,
                SectionNumber::Undefined,
                0x20,
                storage_class::EXTERNAL,
            ));
            reloc(o, text, 1, x);
            let (htext, h) = comdat(o, b"h", false, vec![0xE8, 0, 0, 0, 0]);
            reloc(o, text, 6, h);
            if h_refers {
                let y = o.push_symbol(Symbol::new(
                    Name::new(b"y".to_vec()),
                    0,
                    SectionNumber::Undefined,
                    0x20,
                    storage_class::EXTERNAL,
                ));
                reloc(o, htext, 1, y);
            }
        })
    };
    let (d, e) = (make(true), make(false));
    let (od, oe) = (object(&d), object(&e));
    let (fd, fe) = (find_with(&od, COPY).unwrap(), find_with(&oe, COPY).unwrap());
    assert_eq!(fd.units[0].externals, [b"f".to_vec()]);
    // The local unit's reference to y counts.
    assert_ne!(references(&od, &fd, 0), references(&oe, &fe, 0));
}

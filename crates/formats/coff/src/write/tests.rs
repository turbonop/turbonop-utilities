extern crate std;

use alloc::borrow::Cow;
use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::object::{Relocation, Section, Symbol, comdat_select, storage_class};
use crate::testutil::{ids, round_trip, same_content, sample, testdata};

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

#[test]
fn every_part_round_trips_in_both_formats() {
    for format in [Format::Regular, Format::BigObj] {
        let object = sample(format);
        let bytes = round_trip(&object);
        let again = Object::parse(&bytes).unwrap();
        // Every typed form is read back typed.
        let kinds: Vec<_> = again
            .symbols
            .iter()
            .map(|s| core::mem::discriminant(&s.aux))
            .collect();
        let expected: Vec<_> = object
            .symbols
            .iter()
            .map(|s| core::mem::discriminant(&s.aux))
            .collect();
        assert_eq!(kinds, expected);
        assert_eq!(again.style, Style::Msvc);
    }
}

#[test]
fn a_fresh_msvc_layout() {
    let object = sample(Format::Regular);
    let b = object.write().unwrap();
    // The header.
    assert_eq!(u16_at(&b, 0), 0x8664);
    assert_eq!(u16_at(&b, 2), 7);
    assert_eq!(u32_at(&b, 4), 0x1234_5678);
    let symbols = u32_at(&b, 8) as usize;
    assert_eq!(u32_at(&b, 12), 45);
    // Each section's data, relocations, and line numbers, packed, from the
    // end of the section table; no pointers for what a section lacks.
    let header = |i: usize, field: usize| u32_at(&b, 20 + i * 40 + field);
    let table_end = 20 + 7 * 40;
    assert_eq!(header(0, 20), table_end);
    assert_eq!(header(0, 24), table_end + 32);
    assert_eq!(header(0, 28), table_end + 32 + 30);
    assert_eq!(u16_at(&b, 20 + 32), 3);
    assert_eq!(u16_at(&b, 20 + 34), 3);
    assert_eq!(header(1, 20), table_end + 32 + 30 + 18);
    assert_eq!((header(1, 24), header(1, 28)), (0, 0));
    // .bss: a size but no data.
    assert_eq!((header(2, 16), header(2, 20)), (0x400, 0));
    assert_eq!(header(3, 20), table_end + 80 + 16);
    assert_eq!(header(4, 20), table_end + 80 + 20);
    assert_eq!(header(5, 20), table_end + 80 + 21);
    assert_eq!(header(5, 24), table_end + 80 + 25);
    // An empty section has no data pointer.
    assert_eq!((header(6, 16), header(6, 20)), (0, 0));
    assert_eq!(symbols as u32, table_end + 80 + 25 + 10);
    // main's function definition points at its line numbers.
    let main = symbols + 19 * 18;
    assert_eq!(&b[main..main + 4], b"main");
    assert_eq!(u32_at(&b, main + 18 + 8), header(0, 28));
    // The string table: symbols' names in order, then sections'; the long
    // section name is there twice (its symbol's and its header's).
    let strings = symbols + 45 * 18;
    let table = &b[strings..];
    assert_eq!(u32_at(table, 0) as usize, table.len());
    let names: Vec<&[u8]> = table[4..]
        .split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(
        names,
        [
            &b".a_long_section_name"[..],
            b"comdat_leader",
            b"undefined_function_name",
            b"weak_nolibrary",
            b"weak_library",
            b"weak_alias",
            b"#weak_anti_dependency",
            b"clr_token",
            b"common_symbol_name",
            b"$unwind$leader",
            b".a_long_section_name",
        ]
    );
    let field = &b[20 + 3 * 40..20 + 3 * 40 + 8];
    let offset = table.len() - b".a_long_section_name\0".len();
    assert_eq!(field, crate::strings::section_name_field(offset as u32));
    // An 8-byte name is inline, with no NUL; the empty name is all zeros.
    let exactly8 = symbols + (45 - 4) * 18;
    assert_eq!(&b[exactly8..exactly8 + 8], b"exactly8");
    assert_eq!(&b[exactly8 + 18..exactly8 + 26], &[0; 8]);
}

#[test]
fn a_fresh_llvm_layout() {
    let mut object = sample(Format::Regular);
    object.style = Style::Llvm;
    let b = round_trip(&object);
    let again = Object::parse(&b).unwrap();
    assert_eq!(again.style, Style::Llvm);
    let header = |i: usize, field: usize| u32_at(&b, 20 + i * 40 + field);
    // The empty .drectve points where its data would be (the symbol
    // table); .bss still has no pointer.
    assert_eq!(header(6, 20), u32_at(&b, 8));
    assert_eq!(header(2, 20), 0);
    // Each name once.
    let table = &b[u32_at(&b, 8) as usize + 45 * 18..];
    let count = table[4..]
        .split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .count();
    assert_eq!(count, 10);
}

fn with_relocations(format: Format, count: usize) -> Object<'static> {
    let mut o = Object::new(format, 0x8664);
    let mut s = Section::new(
        ".data",
        0xC050_0040,
        SectionData::Bytes(Cow::Owned(vec![0; count * 8])),
    );
    s.relocations = (0..count as u32)
        .map(|i| Relocation {
            offset: i * 8,
            symbol: SymbolId(0),
            kind: 1,
        })
        .collect();
    o.sections.push(s);
    o.symbols.push(Symbol::new(
        b"x",
        0,
        SectionNumber::Undefined,
        0,
        storage_class::EXTERNAL,
    ));
    o
}

#[test]
fn extended_relocation_counts() {
    for format in [Format::Regular, Format::BigObj] {
        let table = format.header_size();
        // 0xFFFE relocations fit the count; 0xFFFF (as MSVC and LLVM
        // decide) and more need the extended count, which counts itself.
        for (count, extended) in [(0xFFFE, false), (0xFFFF, true), (70_000, true)] {
            let object = with_relocations(format, count);
            assert_eq!(object.sections[0].needs_extended_relocations(), extended);
            let b = round_trip(&object);
            let characteristics = u32_at(&b, table + 36);
            assert_eq!(characteristics & scn::LNK_NRELOC_OVFL != 0, extended);
            let relocations = u32_at(&b, table + 24) as usize;
            if extended {
                assert_eq!(u16_at(&b, table + 32), 0xFFFF);
                assert_eq!(u32_at(&b, relocations), count as u32 + 1);
                assert_eq!(&b[relocations + 4..relocations + 10], &[0; 6]);
                assert_eq!(u32_at(&b, relocations + 20), 8);
            } else {
                assert_eq!(usize::from(u16_at(&b, table + 32)), count);
            }
            let symbols = u32_at(&b, if format == Format::BigObj { 48 } else { 8 }) as usize;
            assert_eq!(symbols, relocations + (count + usize::from(extended)) * 10);
            // Back below the limit, the flag goes.
            let mut fewer = Object::parse(&b).unwrap().into_owned();
            fewer.sections[0].relocations.truncate(10);
            let b = fewer.write().unwrap();
            assert_eq!(u32_at(&b, table + 36) & scn::LNK_NRELOC_OVFL, 0);
            assert_eq!(u16_at(&b, table + 32), 10);
        }
    }
}

#[test]
fn an_extended_count_is_kept_even_when_it_needn_t_be() {
    // A file that uses the extended count for 3 relocations.
    let object = with_relocations(Format::Regular, 3);
    let b = object.write().unwrap();
    let relocations = u32_at(&b, 20 + 24) as usize;
    let mut d = b[..relocations].to_vec();
    d.extend_from_slice(&4u32.to_le_bytes());
    d.extend_from_slice(&[0; 6]);
    d.extend_from_slice(&b[relocations..]);
    d[20 + 32..20 + 34].copy_from_slice(&0xFFFFu16.to_le_bytes());
    let c = u32_at(&d, 20 + 36) | scn::LNK_NRELOC_OVFL;
    d[20 + 36..20 + 40].copy_from_slice(&c.to_le_bytes());
    let symbols = u32_at(&d, 8) + 10;
    d[8..12].copy_from_slice(&symbols.to_le_bytes());
    let read = Object::parse(&d).unwrap();
    assert_eq!(read.sections[0].relocations, object.sections[0].relocations);
    assert_eq!(read.write().unwrap(), d);
}

#[test]
fn converting_between_formats() {
    let regular = sample(Format::Regular);
    let mut big = regular.clone();
    big.set_format(Format::BigObj).unwrap();
    // A raw record gets two zero bytes at its end.
    let Aux::Raw(raw) = &big.symbols[ids::RAW.index()].aux else {
        panic!()
    };
    assert_eq!(raw.len(), 40);
    assert_eq!(&raw[18..20], &[0, 0]);
    assert_eq!(&raw[38..40], &[0, 0]);
    let b = round_trip(&big);
    let mut back = Object::parse(&b).unwrap();
    back.set_format(Format::Regular).unwrap();
    assert!(!back.has_recorded_layout());
    assert!(same_content(&back, &regular));
    assert_eq!(back.write().unwrap(), regular.write().unwrap());
    // Not when a raw record's last bytes hold something.
    let mut lossy = sample(Format::BigObj);
    let before = lossy.clone();
    assert_eq!(
        lossy.set_format(Format::Regular).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    assert_eq!(lossy, before);
    // Setting the format an object has changes nothing.
    let mut same = Object::parse(&b).unwrap();
    same.set_format(Format::BigObj).unwrap();
    assert!(same.has_recorded_layout());
    // A big-obj header has no characteristics or optional header.
    let mut with = regular.clone();
    with.characteristics = 0x100;
    assert_eq!(
        with.clone().set_format(Format::BigObj).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    with.format = Format::BigObj;
    assert_eq!(with.write().unwrap_err().kind, ErrorKind::Unsupported);
    let mut with = regular;
    with.optional_header = Cow::Owned(vec![1, 2]);
    round_trip(&with);
    with.format = Format::BigObj;
    assert_eq!(with.write().unwrap_err().kind, ErrorKind::Unsupported);
}

#[test]
fn more_sections_than_a_regular_object_holds() {
    // 70,000 sections, each with its symbol; the last associative to
    // section 66,000, which only big-obj's high number can name.
    let mut o = Object::new(Format::BigObj, 0x8664);
    for i in 0..70_000u32 {
        o.sections.push(Section::new(
            ".text$mn",
            0x6050_1020,
            SectionData::Bytes(Cow::Owned(i.to_le_bytes().to_vec())),
        ));
        let mut s = Symbol::new(
            b".text$mn",
            0,
            SectionNumber::Section(SectionId(i)),
            0,
            storage_class::STATIC,
        );
        s.aux = Aux::Section(crate::object::SectionDefinition {
            length: 4,
            selection: comdat_select::ASSOCIATIVE,
            number: Some(SectionId(65_999)),
            ..Default::default()
        });
        o.symbols.push(s);
    }
    let b = round_trip(&o);
    assert_eq!(u32_at(&b, 44), 70_000);
    let last = u32_at(&b, 48) as usize + (2 * 69_999 + 1) * 20;
    assert_eq!(u16_at(&b, last + 12), (66_000u32 & 0xFFFF) as u16);
    assert_eq!(u16_at(&b, last + 16), (66_000u32 >> 16) as u16);
    let last_symbol = &b[last - 20..last];
    assert_eq!(u32_at(last_symbol, 12), 70_000);
    // The most a regular object holds is 65,279.
    o.format = Format::Regular;
    assert_eq!(o.write().unwrap_err().kind, ErrorKind::Overflow);
    o.sections.truncate(0xFEFF);
    o.symbols.truncate(0xFEFF);
    for s in &mut o.symbols {
        s.aux = Aux::None;
    }
    let b = round_trip(&o);
    let last_symbol = &b[u32_at(&b, 8) as usize + 0xFEFE * 18..];
    assert_eq!(u16_at(last_symbol, 12), 0xFEFF);
}

#[test]
fn what_cannot_be_written() {
    let base = sample(Format::Regular);
    let kind = |o: &Object<'_>| o.write().unwrap_err().kind;
    let mut o = base.clone();
    o.symbols[3].name.set(&b"nul\0name"[..]);
    assert_eq!(kind(&o), ErrorKind::Malformed);
    let mut o = base.clone();
    o.sections[0].name.set(&b"a\0"[..]);
    assert_eq!(kind(&o), ErrorKind::Malformed);
    let mut o = base.clone();
    o.sections[0].relocations[0].symbol = SymbolId(999);
    assert_eq!(kind(&o), ErrorKind::Malformed);
    let mut o = base.clone();
    o.symbols[ids::MAIN.index()].section = SectionNumber::Section(SectionId(7));
    assert_eq!(kind(&o), ErrorKind::Malformed);
    let mut o = base.clone();
    o.symbols[ids::SECTION_SYMBOLS as usize + 5]
        .section_definition_mut()
        .unwrap()
        .number = Some(SectionId(70));
    assert_eq!(kind(&o), ErrorKind::Malformed);
    let mut o = base.clone();
    let Aux::Function(f) = &mut o.symbols[ids::MAIN.index()].aux else {
        panic!()
    };
    f.line_numbers.as_mut().unwrap().index = 3;
    assert_eq!(kind(&o), ErrorKind::Malformed);
    let mut o = base.clone();
    o.sections[0].line_numbers[0] = crate::object::LineNumber::Function(SymbolId(45));
    assert_eq!(kind(&o), ErrorKind::Malformed);
    let mut o = base.clone();
    o.symbols[ids::RAW.index()].aux = Aux::Raw(Cow::Owned(vec![0; 17]));
    assert_eq!(kind(&o), ErrorKind::Malformed);
    let mut o = base.clone();
    o.symbols[ids::FILE.index()].aux = Aux::file(&b"a\0b"[..]);
    assert_eq!(kind(&o), ErrorKind::Malformed);
    let mut o = base.clone();
    o.symbols[ids::FILE.index()].aux = Aux::Raw(Cow::Owned(vec![0; 18 * 256]));
    assert_eq!(kind(&o), ErrorKind::Overflow);
    // Special section numbers: -256..=-1 fit a regular object; others
    // only big-obj.
    let mut o = base.clone();
    o.symbols[1].section = SectionNumber::Other(-256);
    round_trip(&o);
    o.symbols[1].section = SectionNumber::Other(-257);
    assert_eq!(kind(&o), ErrorKind::Malformed);
    let mut o = sample(Format::BigObj);
    o.symbols[1].section = SectionNumber::Other(-257);
    round_trip(&o);
    let mut o = base;
    o.symbols[1].section = SectionNumber::Other(100);
    round_trip(&o);
}

#[test]
fn same_size_changes_keep_the_layout() {
    let data = testdata("objects.obj");
    let mut object = Object::parse(&data).unwrap();
    // A byte of data, and an inline name of the same length.
    let i = object.sections.iter().position(|s| s.size() > 4).unwrap();
    let SectionData::Bytes(bytes) = &mut object.sections[i].data else {
        panic!()
    };
    bytes.to_mut()[2] ^= 0xFF;
    let short = object
        .symbols
        .iter()
        .position(|s| s.name.as_bytes() == b"@feat.00")
        .unwrap();
    object.symbols[short].name.set(&b"@feat.01"[..]);
    let out = object.write().unwrap();
    let differences: Vec<usize> = (0..data.len()).filter(|&k| out[k] != data[k]).collect();
    assert_eq!(out.len(), data.len());
    assert_eq!(differences.len(), 2, "{differences:?}");
}

#[test]
fn a_new_long_name_is_appended_to_the_table_read() {
    let data = testdata("objects.obj");
    let mut object = Object::parse(&data).unwrap();
    let long = object
        .symbols
        .iter()
        .position(|s| s.name.as_bytes().len() > 8)
        .unwrap();
    object.symbols[long].name.set(&b"a_name_no_object_has"[..]);
    let out = object.write().unwrap();
    // Everything up to the string table is as it was; the table grew by
    // the name.
    let strings = u32_at(&data, 8) as usize + u32_at(&data, 12) as usize * 18;
    assert_eq!(out.len(), data.len() + b"a_name_no_object_has\0".len());
    let record = u32_at(&data, 8) as usize
        + object.symbol_table_index(SymbolId(long as u32)).unwrap() as usize * 18;
    let changed: Vec<usize> = (0..strings).filter(|&k| out[k] != data[k]).collect();
    assert!(!changed.is_empty());
    assert!(
        changed
            .iter()
            .all(|&k| (record + 4..record + 8).contains(&k)),
        "only the name's offset"
    );
    assert_eq!(&out[data.len()..], b"a_name_no_object_has\0");
    let again = Object::parse(&out).unwrap();
    assert_eq!(again.symbols[long].name, b"a_name_no_object_has");
    assert!(same_content(&again, &object));
}

/// `sample`'s bytes with `pad` bytes of `fill` after the section table
/// (every pointer past it moved), and `trailer` after the string table.
fn padded(pad: usize, fill: u8, trailer: &[u8]) -> Vec<u8> {
    let b = sample(Format::Regular).write().unwrap();
    let table_end = 20 + 7 * 40;
    let mut d = b[..table_end].to_vec();
    d.resize(table_end + pad, fill);
    d.extend_from_slice(&b[table_end..]);
    d.extend_from_slice(trailer);
    let moved = |d: &mut Vec<u8>, at: usize| {
        let v = u32_at(d, at);
        if v != 0 {
            d[at..at + 4].copy_from_slice(&(v + pad as u32).to_le_bytes());
        }
    };
    moved(&mut d, 8);
    for i in 0..7 {
        for field in [20, 24, 28] {
            moved(&mut d, 20 + i * 40 + field);
        }
    }
    // main's line number pointer.
    let main = u32_at(&d, 8) as usize + 19 * 18;
    moved(&mut d, main + 18 + 8);
    d
}

#[test]
fn padding_and_trailing_bytes_are_kept() {
    for (pad, fill, trailer) in [
        (3, 0xCC, &b""[..]),
        (0, 0, b"TRAILER"),
        (5, 0, b"\0\0\0"),
        (2, 0x90, b"x"),
    ] {
        let d = padded(pad, fill, trailer);
        let object = Object::parse(&d).unwrap();
        assert!(
            same_content(&object, &sample(Format::Regular)),
            "{pad}: {}",
            crate::testutil::difference(&object, &sample(Format::Regular))
        );
        assert_eq!(object.write().unwrap(), d);
        // Afresh, the padding goes.
        let mut fresh = object.clone();
        fresh.forget_layout();
        assert_eq!(
            fresh.write().unwrap(),
            sample(Format::Regular).write().unwrap()
        );
    }
}

#[test]
fn names_stored_unusually_are_kept() {
    let b = sample(Format::Regular).write().unwrap();
    let symbols = u32_at(&b, 8) as usize;
    let strings = symbols + 45 * 18;
    let mut d = b.clone();
    // A short name in the string table: "main", appended.
    let main = symbols + 19 * 18;
    let at = d.len() - strings;
    d.extend_from_slice(b"main\0");
    d[main..main + 8].copy_from_slice(&[0, 0, 0, 0, at as u8, (at >> 8) as u8, 0, 0]);
    // Bytes after an inline name's NUL.
    let lf = symbols + 23 * 18;
    assert_eq!(&d[lf..lf + 4], b".lf\0");
    d[lf + 5] = b'!';
    // A section name with leading zeros, and one in the table that fits
    // inline.
    let long = 20 + 3 * 40;
    let offset =
        crate::strings::section_name_offset(d[long..long + 8].try_into().unwrap()).unwrap();
    let field = alloc::format!("/{offset:07}");
    d[long..long + 8].copy_from_slice(field.as_bytes());
    let data = 20 + 40;
    let at = d.len() - strings;
    d.extend_from_slice(b".data\0");
    let field = alloc::format!("/{at}");
    d[data..data + 8].fill(0);
    d[data..data + field.len()].copy_from_slice(field.as_bytes());
    let size = (d.len() - strings) as u32;
    d[strings..strings + 4].copy_from_slice(&size.to_le_bytes());
    let object = Object::parse(&d).unwrap();
    assert!(same_content(&object, &sample(Format::Regular)));
    assert_eq!(object.write().unwrap(), d);
    // A changed name is stored as the writer chooses.
    let mut changed = object.clone();
    changed.symbols[ids::MAIN.index()].name.set(&b"main"[..]);
    let out = changed.write().unwrap();
    assert_eq!(&out[main..main + 8], b"main\0\0\0\0");
}

#[test]
fn a_section_named_like_a_long_name() {
    // "/4" as a section's own name is stored in the string table, so that
    // it isn't read as offset 4.
    let mut o = sample(Format::Regular);
    o.sections[1].name.set(&b"/4"[..]);
    let b = round_trip(&o);
    assert_ne!(&b[60..62], b"/4");
}

#[test]
fn objects_that_own_their_bytes() {
    let data = testdata("objects-big.obj");
    let owned = Object::parse(&data).unwrap().into_owned();
    drop(data);
    let data = testdata("objects-big.obj");
    assert_eq!(owned.write().unwrap(), data);
    let mut out = vec![1, 2, 3];
    owned.write_to(&mut out).unwrap();
    assert_eq!(&out[..3], &[1, 2, 3]);
    assert_eq!(&out[3..], data);
}

#[test]
fn a_skeleton_filled_is_the_file() {
    use crate::{File, Object};
    for name in [
        "objects.obj",
        "objects-big.obj",
        "model/clang-x64.obj",
        "model/msvc-x86.obj",
    ] {
        let data = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../testdata/coff")
                .join(name),
        )
        .unwrap();
        let File::Object(o) = File::parse(&data).unwrap() else {
            panic!("{name}: not a COFF object")
        };
        let mut edited: Object<'_> = o.clone();
        // A structural edit, so the object is laid out afresh too.
        edited.forget_layout();
        for object in [&o, &edited] {
            let whole = object.write().unwrap();
            for min in [0, 1, 64, 1 << 20] {
                let s = object.write_skeleton(min).unwrap();
                assert_eq!(s.len(), whole.len());
                for (at, piece) in &s.holes {
                    assert!(piece.len() >= min.max(1));
                    assert_eq!(&whole[*at..*at + piece.len()], *piece);
                }
                assert!(s.holes.windows(2).all(|w| w[0].0 + w[0].1.len() <= w[1].0));
                assert_eq!(s.to_file(), whole, "{name}, min {min}");
                // The symbol table (and what follows it) where the
                // skeleton says.
                let field = if object.format == Format::BigObj {
                    48
                } else {
                    8
                };
                let ptr = u32::from_le_bytes(whole[field..field + 4].try_into().unwrap()) as usize;
                if ptr != 0 {
                    let tail = &s.bytes[s.symbol_table as usize..];
                    assert_eq!(tail, &whole[ptr..ptr + tail.len()], "{name}, min {min}");
                }
            }
        }
    }
}

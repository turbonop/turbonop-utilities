extern crate std;

use alloc::vec::Vec;

use turbonop_pe::PeFile;

use super::*;
use crate::kind::File;
use crate::object::{Aux, comdat_select};
use crate::testutil::{same_content, testdata};

/// Checks that `data` is read and written back as it was, and that the
/// model agrees with `turbonop-pe`'s reader.
fn check_round_trip(data: &[u8]) -> Object<'_> {
    let object = Object::parse(data).unwrap();
    assert_eq!(object.write().unwrap(), data);
    let pe = PeFile::parse_object(data).unwrap();
    assert_eq!(object.sections.len(), pe.sections().len());
    for (s, h) in object.sections.iter().zip(pe.sections()) {
        assert_eq!(s.name.as_bytes(), pe.section_name(h));
        assert_eq!(s.size(), h.size_of_raw_data.get());
        assert_eq!(s.characteristics, h.characteristics.get());
    }
    let table = pe.symbols().unwrap().unwrap();
    let primaries: Vec<_> = table.iter().collect();
    assert_eq!(object.symbols.len(), primaries.len());
    let indices = object.symbol_table_indices();
    for ((s, p), &index) in object.symbols.iter().zip(&primaries).zip(&indices) {
        assert_eq!(index as usize, p.index);
        assert_eq!(s.name.as_bytes(), table.name(p).unwrap());
        assert_eq!(s.value, p.value);
        assert_eq!(s.storage_class, p.storage_class);
        assert_eq!(s.section.raw(), p.section);
        if let (Some(d), Some(e)) = (s.section_definition(), p.section_definition()) {
            assert_eq!(d.length, e.length);
            assert_eq!(d.selection, e.selection);
            assert_eq!(d.number.map_or(0, SectionId::number), e.number);
        }
    }
    // Laid out afresh, an MSVC object is what MSVC wrote.
    let mut fresh = object.clone();
    fresh.forget_layout();
    assert_eq!(fresh.write().unwrap(), data);
    object
}

#[test]
fn msvc_objects_round_trip() {
    for name in ["objects.obj", "objects-zi.obj"] {
        let data = testdata(name);
        let object = check_round_trip(&data);
        assert_eq!(object.format, Format::Regular);
        assert_eq!(object.style, Style::Msvc);
    }
}

#[test]
fn bigobj_round_trips() {
    let data = testdata("objects-big.obj");
    let object = check_round_trip(&data);
    assert_eq!(object.format, Format::BigObj);
    assert_eq!(object.bigobj, BigObjHeader::default());
    // To a regular object and back.
    let mut regular = object.clone();
    regular.format = Format::Regular;
    regular.forget_layout();
    let bytes = regular.write().unwrap();
    assert_eq!(kind(&bytes), Some(Kind::Regular));
    let mut back = Object::parse(&bytes).unwrap();
    back.format = Format::BigObj;
    assert!(same_content(&back, &object));
    back.forget_layout();
    assert_eq!(back.write().unwrap(), data);
}

#[test]
fn comdats_and_associative_sections_are_typed() {
    let data = testdata("objects.obj");
    let object = Object::parse(&data).unwrap();
    let mut associative = 0;
    for s in &object.symbols {
        if let Aux::Section(d) = &s.aux {
            let id = s.section.id().unwrap();
            assert_eq!(d.length, object.sections[id.index()].size());
            if d.selection == comdat_select::ASSOCIATIVE {
                let leader = d.number.unwrap();
                assert!(leader < id, "MSVC numbers a leader before its associates");
                associative += 1;
            }
        }
        assert!(!matches!(s.aux, Aux::Raw(_)), "{s:?}");
    }
    assert!(associative > 0);
    // Every COMDAT's checksum without relocations is checksum()'s.
    for s in &object.symbols {
        let (Some(d), Some(id)) = (s.section_definition(), s.section.id()) else {
            continue;
        };
        let section = &object.sections[id.index()];
        if d.selection != 0 && section.relocations.is_empty() && d.checksum != 0 {
            assert_eq!(d.checksum, crate::checksum(section.bytes()), "{s:?}");
        }
    }
}

#[test]
fn library_members_round_trip_or_pass_through() {
    use turbonop_archive::Archive;
    let mut kinds = Vec::new();
    for name in ["objects.lib", "imports.lib", "ltcg.lib"] {
        let data = testdata(name);
        let archive = Archive::parse(&data).unwrap();
        for member in archive.files() {
            let file = File::parse(member.data).unwrap();
            assert_eq!(file.write().unwrap(), member.data);
            kinds.push(match file {
                File::Object(o) if o.format == Format::BigObj => Kind::BigObj,
                File::Object(_) => Kind::Regular,
                File::Opaque { kind, .. } => kind,
            });
        }
    }
    for kind in [Kind::Regular, Kind::BigObj, Kind::Import, Kind::Ltcg] {
        assert!(kinds.contains(&kind), "{kind:?} in {kinds:?}");
    }
}

#[test]
fn anonymous_objects_are_not_coff_objects() {
    for name in ["ltcg.obj", "ltcg-x86.obj"] {
        let data = testdata(name);
        assert_eq!(kind(&data), Some(Kind::Ltcg));
        assert_eq!(
            Object::parse(&data).unwrap_err().kind,
            ErrorKind::Unsupported
        );
        assert_eq!(
            File::parse(&data).unwrap(),
            File::Opaque {
                kind: Kind::Ltcg,
                data: &data
            }
        );
    }
    // An import object, and an anonymous header of another class.
    let mut import = alloc::vec![0, 0, 0xFF, 0xFF, 0, 0, 0x64, 0x86];
    import.resize(32, 0);
    assert_eq!(kind(&import), Some(Kind::Import));
    assert_eq!(
        Object::parse(&import).unwrap_err().kind,
        ErrorKind::Unsupported
    );
    let mut other = import.clone();
    other[4] = 2;
    other[12] = 1;
    assert_eq!(kind(&other), Some(Kind::Anonymous));
    // A version-1 header with the big-obj class is not big-obj.
    let mut v1 = alloc::vec![0, 0, 0xFF, 0xFF, 1, 0, 0x64, 0x86, 0, 0, 0, 0];
    v1.extend_from_slice(&crate::BIGOBJ_CLASS_ID);
    v1.resize(56, 0);
    assert_eq!(kind(&v1), Some(Kind::Anonymous));
    v1[4] = 2;
    assert_eq!(kind(&v1), Some(Kind::BigObj));
    assert_eq!(kind(&v1[..40]), Some(Kind::Anonymous));
    assert_eq!(kind(&[0; 19]), None);
    assert_eq!(kind(&[0; 20]), Some(Kind::Regular));
}

#[test]
fn an_empty_object_round_trips() {
    // A header and nothing else: no sections, no symbol table.
    let mut data = alloc::vec![0u8; 20];
    data[..2].copy_from_slice(&0x8664u16.to_le_bytes());
    let object = Object::parse(&data).unwrap();
    assert!(object.sections.is_empty() && object.symbols.is_empty());
    assert_eq!(object.write().unwrap(), data);
}

#[test]
fn malformed_objects_are_errors() {
    let data = testdata("objects.obj");
    let object = Object::parse(&data).unwrap();
    let symbols = u32_at(&data, 8) as usize;
    let count = u32_at(&data, 12) as usize;
    let with = |at: usize, bytes: &[u8]| {
        let mut d = data.clone();
        d[at..at + bytes.len()].copy_from_slice(bytes);
        d
    };
    let kind = |d: &[u8]| Object::parse(d).unwrap_err().kind;
    // A symbol table past the end.
    assert_eq!(
        kind(&with(8, &u32::MAX.to_le_bytes())),
        ErrorKind::Truncated
    );
    // Symbols but no symbol table.
    assert_eq!(kind(&with(8, &[0; 4])), ErrorKind::Malformed);
    // The last symbol claims auxiliary records past the table.
    let last = object
        .symbol_table_index(SymbolId(object.symbols.len() as u32 - 1))
        .unwrap() as usize;
    let claim = (count - last) as u8;
    assert_eq!(
        kind(&with(symbols + last * 18 + 17, &[claim])),
        ErrorKind::Malformed
    );
    // A relocation to an auxiliary record.
    let (i, s) = object
        .sections
        .iter()
        .enumerate()
        .find(|(_, s)| !s.relocations.is_empty())
        .unwrap();
    let header = 20 + i * 40;
    let relocations = u32_at(&data, header + 24) as usize;
    let aux = object.symbol_table_index(s.relocations[0].symbol).unwrap();
    let aux = (0..count as u32)
        .find(|&r| r > aux && object.symbol_at_table_index(r).is_none())
        .unwrap();
    assert_eq!(
        kind(&with(relocations + 4, &aux.to_le_bytes())),
        ErrorKind::Malformed
    );
    // A long name past the string table.
    let long = object
        .symbols
        .iter()
        .position(|s| s.name.as_bytes().len() > 8)
        .unwrap();
    let at = symbols + object.symbol_table_index(SymbolId(long as u32)).unwrap() as usize * 18;
    assert_eq!(
        kind(&with(at + 4, &u32::MAX.to_le_bytes())),
        ErrorKind::Malformed
    );
    // Section data past the end.
    assert_eq!(
        kind(&with(20 + 16, &u32::MAX.to_le_bytes())),
        ErrorKind::Truncated
    );
    // An extended relocation count of 0 (which counts itself).
    let mut d = with(header + 32, &0xFFFFu16.to_le_bytes());
    let c = u32_at(&d, header + 36) | 0x0100_0000;
    d[header + 36..header + 40].copy_from_slice(&c.to_le_bytes());
    d[relocations..relocations + 4].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(kind(&d), ErrorKind::Malformed);
    // Too short for a header.
    assert_eq!(kind(&data[..19]), ErrorKind::Truncated);
}

/// A deterministic stream of numbers.
fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn damaged_objects_never_panic() {
    // Truncated at every length, and with bytes changed at random: either
    // an error, or an object that writes (and writes again the same after
    // reading it back).
    for name in ["objects.obj", "objects-big.obj"] {
        let data = testdata(name);
        for len in (0..data.len()).step_by(7) {
            if let Ok(o) = Object::parse(&data[..len]) {
                let _ = o.write();
            }
        }
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..2000 {
            let mut d = data.clone();
            for _ in 0..1 + xorshift(&mut state) % 4 {
                let at = (xorshift(&mut state) % d.len() as u64) as usize;
                d[at] = xorshift(&mut state) as u8;
            }
            if let Ok(o) = Object::parse(&d)
                && let Ok(bytes) = o.write()
            {
                assert_eq!(bytes, d, "an object read writes back as it was");
            }
        }
    }
}

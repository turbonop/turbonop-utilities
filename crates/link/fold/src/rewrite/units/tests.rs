use super::*;
use std::borrow::Cow;
use std::collections::HashMap;
use turbonop_coff::{File, Name, Section, Symbol};

fn fixture(rel: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/coff")
            .join(rel),
    )
    .unwrap()
}

fn object(data: &[u8]) -> Object<'_> {
    match File::parse(data).unwrap() {
        File::Object(o) => o,
        File::Opaque { .. } => panic!("not a COFF object"),
    }
}

/// Each unit's first external name, and its identity.
fn identities(f: &Facts) -> HashMap<Vec<u8>, u128> {
    f.units
        .iter()
        .filter_map(|u| u.externals.first().map(|n| (n.clone(), u.identity)))
        .collect()
}

#[test]
fn units_of_a_gy_object() {
    let data = fixture("objects.obj");
    let o = object(&data);
    let f = find(&o).unwrap();
    assert!(!f.units.is_empty());
    let mut seen = vec![false; o.sections.len()];
    for u in &f.units {
        assert_eq!(u.sections[0], u.leader, "the leader first");
        assert!(matches!(
            u.selection,
            comdat_select::ANY
                | comdat_select::SAME_SIZE
                | comdat_select::EXACT_MATCH
                | comdat_select::LARGEST
        ));
        for s in &u.sections {
            assert!(!seen[s.index()], "a section in two units");
            seen[s.index()] = true;
        }
        // Every symbol it lists is defined in one of its sections.
        for id in &u.symbols {
            let sec = o.symbols[id.index()].section.id().unwrap();
            assert!(u.sections.contains(&sec));
        }
    }
    // A function's .pdata and .xdata are in its unit.
    let with_pdata = f
        .units
        .iter()
        .filter(|u| {
            u.sections
                .iter()
                .any(|s| o.sections[s.index()].name.as_bytes() == b".pdata")
        })
        .count();
    assert!(with_pdata > 0);
    // /Gy functions with external linkage are NODUPLICATES: never units.
    for (i, s) in o.symbols.iter().enumerate() {
        if let Aux::Section(d) = &s.aux
            && d.selection == comdat_select::NODUPLICATES
        {
            let id = s.section.id().unwrap();
            assert!(
                !seen[id.index()],
                "symbol {i}'s NODUPLICATES section is in a unit"
            );
        }
    }
}

#[test]
fn the_same_code_has_the_same_identity_in_either_format() {
    let a = fixture("objects.obj");
    let b = fixture("objects-big.obj");
    let (oa, ob) = (object(&a), object(&b));
    assert_ne!(oa.format, ob.format);
    let (fa, fb) = (find(&oa).unwrap(), find(&ob).unwrap());
    let (ia, ib) = (identities(&fa), identities(&fb));
    let common: Vec<_> = ia.keys().filter(|k| ib.contains_key(*k)).collect();
    assert!(common.len() > 5);
    for k in common {
        assert_eq!(ia[k], ib[k], "{}", String::from_utf8_lossy(k));
    }
}

#[test]
fn weak_externals_with_external_defaults_fold() {
    // objects.obj's vftables refer to ??_E, a weak external whose default
    // is ??_G: neither the vftable nor ??_G is held back by it.
    let data = fixture("objects.obj");
    let o = object(&data);
    let f = find(&o).unwrap();
    for u in &f.units {
        assert_ne!(u.unfoldable, Some(Unfoldable::WeakExternalReference));
        if u.externals.iter().any(|n| n.starts_with(b"??_G")) {
            assert_ne!(
                u.unfoldable,
                Some(Unfoldable::ReferencedLocally),
                "{:?}",
                u.detail
            );
        }
    }
}

const COMDAT: u32 = 0x1000;
const CODE: u32 = 0x6050_0020;

/// An object with a COMDAT function `f` (selection `ANY`) and whatever
/// `extra` adds.
fn with_comdat(extra: impl FnOnce(&mut Object<'static>, SectionId, SymbolId)) -> Vec<u8> {
    let mut o = Object::new(turbonop_coff::Format::Regular, 0x8664);
    let text = o.push_section(Section::new(
        Name::new(b".text$mn".to_vec()),
        CODE | COMDAT,
        SectionData::Bytes(Cow::Owned(vec![0xC3; 16])),
    ));
    let mut ss = Symbol::new(
        Name::new(b".text$mn".to_vec()),
        0,
        SectionNumber::Section(text),
        0,
        storage_class::STATIC,
    );
    ss.aux = Aux::Section(turbonop_coff::SectionDefinition {
        selection: comdat_select::ANY,
        ..Default::default()
    });
    o.push_symbol(ss);
    let f = o.push_symbol(Symbol::new(
        Name::new(b"f".to_vec()),
        0,
        SectionNumber::Section(text),
        0x20,
        storage_class::EXTERNAL,
    ));
    extra(&mut o, text, f);
    o.sync_section_definitions();
    o.write().unwrap()
}

#[test]
fn a_local_symbol_referred_to_from_outside_holds_a_unit_back() {
    let data = with_comdat(|o, text, _| {
        let label = o.push_symbol(Symbol::new(
            Name::new(b"$LN1".to_vec()),
            4,
            SectionNumber::Section(text),
            0,
            storage_class::LABEL,
        ));
        let mut data = Section::new(
            Name::new(b".data".to_vec()),
            0xC030_0040,
            SectionData::Bytes(Cow::Owned(vec![0; 8])),
        );
        data.relocations.push(turbonop_coff::Relocation {
            offset: 0,
            symbol: label,
            kind: 1,
        });
        o.push_section(data);
    });
    let o = object(&data);
    let f = find(&o).unwrap();
    assert_eq!(f.units.len(), 1);
    assert_eq!(f.units[0].unfoldable, Some(Unfoldable::ReferencedLocally));
}

#[test]
fn a_reference_to_a_local_outside_holds_a_unit_back() {
    let data = with_comdat(|o, text, _| {
        let rdata = o.push_section(Section::new(
            Name::new(b".rdata".to_vec()),
            0x4030_0040,
            SectionData::Bytes(Cow::Owned(vec![1; 8])),
        ));
        let local = o.push_symbol(Symbol::new(
            Name::new(b"$SG1".to_vec()),
            0,
            SectionNumber::Section(rdata),
            0,
            storage_class::STATIC,
        ));
        o.sections[text.index()]
            .relocations
            .push(turbonop_coff::Relocation {
                offset: 0,
                symbol: local,
                kind: 4,
            });
    });
    let f = find(&object(&data)).unwrap();
    assert_eq!(f.units[0].unfoldable, Some(Unfoldable::LocalReference));
}

#[test]
fn a_local_comdat_is_internal_linkage() {
    let data = with_comdat(|o, _, f| {
        o.symbols[f.index()].storage_class = storage_class::STATIC;
    });
    let f = find(&object(&data)).unwrap();
    assert_eq!(f.units[0].unfoldable, Some(Unfoldable::InternalLinkage));
}

#[test]
fn unknown_sections_pass_the_member_through() {
    let data = with_comdat(|o, _, _| {
        o.push_section(Section::new(
            Name::new(b".weird".to_vec()),
            0x4000_0040,
            SectionData::Bytes(Cow::Owned(vec![0; 4])),
        ));
    });
    assert_eq!(
        find(&object(&data)).unwrap_err(),
        Unfoldable::UnknownSection
    );
}

#[test]
fn identity_ignores_local_names_but_not_code() {
    let label = |name: &'static [u8]| {
        move |o: &mut Object<'static>, text: SectionId, _| {
            o.push_symbol(Symbol::new(
                Name::new(name.to_vec()),
                4,
                SectionNumber::Section(text),
                0,
                storage_class::LABEL,
            ));
        }
    };
    let a = with_comdat(label(b"$LN5"));
    let b = with_comdat(label(b"$LN7"));
    let ia = find(&object(&a)).unwrap().units[0].identity;
    let ib = find(&object(&b)).unwrap().units[0].identity;
    assert_eq!(ia, ib, "labels named differently are the same code");
    let c = with_comdat(|o, text, _| {
        if let SectionData::Bytes(b) = &mut o.sections[text.index()].data {
            b.to_mut()[0] = 0x90;
        }
    });
    let ic = find(&object(&c)).unwrap().units[0].identity;
    assert_ne!(ia, ic);
}

#[test]
fn side_effects_and_directives() {
    let data = with_comdat(|o, _, _| {
        o.push_section(Section::new(
            Name::new(b".CRT$XCU".to_vec()),
            0x4030_0040,
            SectionData::Bytes(Cow::Owned(vec![0; 8])),
        ));
        o.push_section(Section::new(
            Name::new(b".drectve".to_vec()),
            0x0010_0A00,
            SectionData::Bytes(Cow::Owned(
                b" /FAILIFMISMATCH:\"_MSC_VER=1900\" /INCLUDE:__@@_PchSym_x".to_vec(),
            )),
        ));
    });
    let f = find(&object(&data)).unwrap();
    assert!(f.side_effects, "a non-COMDAT .CRT$XCU");
    assert_eq!(
        f.mismatches,
        [b"/FAILIFMISMATCH:\"_MSC_VER=1900\"".to_vec()]
    );
    // The PCH symbol's /INCLUDE alone isn't a side effect.
    let data = with_comdat(|o, _, _| {
        o.push_section(Section::new(
            Name::new(b".drectve".to_vec()),
            0x0010_0A00,
            SectionData::Bytes(Cow::Owned(b" /INCLUDE:__@@_PchSym_x".to_vec())),
        ));
    });
    assert!(!find(&object(&data)).unwrap().side_effects);
}

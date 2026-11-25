extern crate std;

use alloc::borrow::Cow;
use alloc::vec::Vec;

use super::*;
use crate::object::{Format, SectionData, comdat_select, storage_class};
use crate::testutil::{ids, round_trip, sample, testdata};

/// Each relocation's target, by name, section by section.
fn targets(o: &Object<'_>) -> Vec<Vec<Vec<u8>>> {
    o.sections
        .iter()
        .map(|s| {
            s.relocations
                .iter()
                .map(|r| o.symbols[r.symbol.index()].name.as_bytes().to_vec())
                .collect()
        })
        .collect()
}

/// Makes `symbol` an undefined external, as a folded COMDAT's leader
/// becomes.
fn undefine(o: &mut Object<'_>, symbol: SymbolId) {
    let s = &mut o.symbols[symbol.index()];
    s.section = SectionNumber::Undefined;
    s.value = 0;
    s.aux = Aux::None;
}

#[test]
fn removing_a_comdat_and_its_associative_section() {
    let mut o = sample(Format::Regular);
    let before = targets(&o);
    undefine(&mut o, ids::LEADER);
    let r = o
        .remove_sections(|id, _| id == SectionId(4) || id == SectionId(5))
        .unwrap();
    assert_eq!(o.sections.len(), 5);
    // The two sections' symbols and the label in .xdata went.
    assert_eq!(o.symbols.len(), 22);
    for gone in [6, 7, ids::UNWIND_LABEL.0] {
        assert_eq!(r.symbol(SymbolId(gone)), None);
    }
    assert_eq!(r.symbol(ids::MAIN), Some(SymbolId(7)));
    assert_eq!(r.symbol(ids::LEADER), Some(SymbolId(11)));
    assert_eq!(r.section(SectionId(6)), Some(SectionId(4)));
    assert_eq!(r.section(SectionId(4)), None);
    // Relocations and records follow their symbols.
    let after = targets(&o);
    assert_eq!(after[..4], before[..4]);
    assert_eq!(after[4], before[6]);
    let Aux::BeginEnd(bf) = &o.symbols[8].aux else {
        panic!("{:?}", o.symbols[8])
    };
    assert_eq!(bf.next, Some(SymbolId(11)));
    let Aux::Function(main) = &o.symbols[7].aux else {
        panic!()
    };
    assert_eq!(main.tag, Some(SymbolId(8)));
    for s in &o.symbols[13..17] {
        let Aux::WeakExternal(w) = &s.aux else {
            panic!()
        };
        assert_eq!(w.tag, SymbolId(7));
    }
    let Aux::ClrToken(c) = &o.symbols[ids::CLR_TOKEN.index() - 2].aux else {
        panic!()
    };
    assert_eq!(c.symbol, SymbolId(7));
    assert_eq!(o.symbols[6].section, SectionNumber::Section(SectionId(4)));
    assert_eq!(o.symbols[6].name, b".drectve");
    assert_eq!(
        o.sections[0].line_numbers[0],
        LineNumber::Function(SymbolId(7))
    );
    // Table indices: main was at 19 and is at 15 (two section symbols of
    // two records each went before it); a removed symbol and an auxiliary
    // record map to nothing.
    assert_eq!(r.table_index(19), Some(15));
    assert_eq!(r.table_index(0), Some(0));
    assert_eq!(r.table_index(13), None);
    assert_eq!(r.table_index(20), None);
    assert_eq!(r.table_index(44), None);
    round_trip(&o);
}

type Edit = dyn Fn(&mut Object<'_>) -> Result<Renumbering, EditError>;

/// Checks that `edit` is refused and changes nothing.
fn refused(o: &mut Object<'_>, edit: &Edit) -> EditError {
    let before = o.clone();
    let e = edit(o).unwrap_err();
    assert_eq!(*o, before, "an edit refused changes nothing");
    e
}

#[test]
fn edits_that_would_leave_references_dangling() {
    let mut o = sample(Format::Regular);
    // The leader goes with its section, but a relocation still uses it.
    assert_eq!(
        refused(&mut o, &|o| o.remove_sections(|id, _| id == SectionId(4))),
        EditError::SectionRefersToRemovedSymbol {
            section: SectionId(0),
            symbol: ids::LEADER,
        }
    );
    // A section goes and its symbols stay.
    assert_eq!(
        refused(&mut o, &|o| o
            .retain(|id, _| id != SectionId(4), |_, _| true)),
        EditError::SymbolInRemovedSection {
            symbol: SymbolId(6),
            section: SectionId(4),
        }
    );
    // A line number's function, and a .bf that main's record names.
    assert_eq!(
        refused(&mut o, &|o| o.remove_symbols(|id, _| id == ids::MAIN)),
        EditError::SectionRefersToRemovedSymbol {
            section: SectionId(0),
            symbol: ids::MAIN,
        }
    );
    assert_eq!(
        refused(&mut o, &|o| o.remove_symbols(|id, _| id == ids::BF)),
        EditError::AuxRefersToRemovedSymbol {
            symbol: ids::MAIN,
            target: ids::BF,
        }
    );
    // An associative section's leader goes, and it stays.
    undefine(&mut o, ids::LEADER);
    let e = refused(&mut o, &|o| o.remove_sections(|id, _| id == SectionId(4)));
    assert_eq!(
        e,
        EditError::AuxRefersToRemovedSection {
            symbol: SymbolId(7),
            section: SectionId(4),
        }
    );
    assert_eq!(
        std::format!("{e}"),
        "symbol 7's auxiliary record names section 5, which is removed"
    );
}

#[test]
fn inserting_a_symbol_renumbers_those_after_it() {
    let mut o = sample(Format::Regular);
    let before = targets(&o);
    let at = o.insert_symbol(
        SymbolId(2),
        Symbol::new(
            b"@feat.00",
            0x11,
            SectionNumber::Absolute,
            0,
            storage_class::STATIC,
        ),
    );
    assert_eq!(at, SymbolId(2));
    assert_eq!(targets(&o), before);
    let Aux::Function(main) = &o.symbols[ids::MAIN.index() + 1].aux else {
        panic!()
    };
    assert_eq!(main.tag, Some(SymbolId(ids::BF.0 + 1)));
    assert_eq!(
        o.sections[0].line_numbers[0],
        LineNumber::Function(SymbolId(ids::MAIN.0 + 1))
    );
    round_trip(&o);
    // At the end, and past it.
    let end = o.insert_symbol(
        SymbolId(999),
        Symbol::new(
            b"last",
            0,
            SectionNumber::Undefined,
            0,
            storage_class::EXTERNAL,
        ),
    );
    assert_eq!(end, SymbolId(o.symbols.len() as u32 - 1));
    assert_eq!(targets(&o), before);
}

#[test]
fn inserting_a_section_renumbers_those_after_it() {
    let mut o = sample(Format::Regular);
    let at = o.insert_section(
        SectionId(0),
        crate::Section::new(
            ".drectve",
            0xA00,
            SectionData::Bytes(Cow::Borrowed(b" /merge:a=b")),
        ),
    );
    assert_eq!(at, SectionId(0));
    assert_eq!(o.sections.len(), 8);
    // Every section number moved up: the symbols', the associative
    // reference, and main's line numbers.
    let definition = o.symbols[ids::SECTION_SYMBOLS as usize + 5]
        .section_definition()
        .unwrap();
    assert_eq!(definition.number, Some(SectionId(5)));
    assert_eq!(
        o.symbols[ids::LEADER.index()].section,
        SectionNumber::Section(SectionId(5))
    );
    let Aux::Function(main) = &o.symbols[ids::MAIN.index()].aux else {
        panic!()
    };
    assert_eq!(main.line_numbers.unwrap().section, SectionId(1));
    round_trip(&o);
}

#[test]
fn structural_edits_forget_the_layout() {
    let data = testdata("objects.obj");
    let section = || crate::Section::new(".x", 0, SectionData::Uninitialized(4));
    let symbol = || Symbol::new(b"anchor", 0, SectionNumber::Undefined, 0, 2);
    let edits: [&dyn Fn(&mut Object<'_>); 5] = [
        &|o| {
            o.push_section(section());
        },
        &|o| {
            o.push_symbol(symbol());
        },
        &|o| {
            o.insert_symbol(SymbolId(0), symbol());
        },
        &|o| {
            o.insert_section(SectionId(0), section());
        },
        &|o| {
            o.retain(|_, _| true, |_, _| true).unwrap();
        },
    ];
    for edit in edits {
        let mut o = Object::parse(&data).unwrap();
        assert!(o.has_recorded_layout());
        edit(&mut o);
        assert!(!o.has_recorded_layout());
        round_trip(&o);
    }
    // Keeping everything changes nothing but the layout, which an MSVC
    // object doesn't need.
    let mut o = Object::parse(&data).unwrap();
    o.retain(|_, _| true, |_, _| true).unwrap();
    assert_eq!(o.write().unwrap(), data);
}

#[test]
fn table_indices_and_section_symbols() {
    let o = sample(Format::Regular);
    assert_eq!(o.symbol_table_index(SymbolId(0)), Some(0));
    assert_eq!(o.symbol_table_index(SymbolId(1)), Some(4));
    assert_eq!(o.symbol_table_index(ids::MAIN), Some(19));
    assert_eq!(o.symbol_table_index(SymbolId(25)), None);
    let all = o.symbol_table_indices();
    assert_eq!(all.len(), o.symbols.len());
    for (i, &index) in all.iter().enumerate() {
        assert_eq!(o.symbol_at_table_index(index), Some(SymbolId(i as u32)));
        assert_eq!(o.symbol_table_index(SymbolId(i as u32)), Some(index));
    }
    assert_eq!(o.symbol_at_table_index(1), None);
    assert_eq!(o.symbol_at_table_index(45), None);
    // In big-obj, the .file name takes 2 records, not 3.
    let big = sample(Format::BigObj);
    assert_eq!(big.symbol_table_index(SymbolId(1)), Some(3));
    for i in 0..7 {
        assert_eq!(
            o.section_symbol(SectionId(i)),
            Some(SymbolId(ids::SECTION_SYMBOLS + i))
        );
    }
    assert_eq!(o.section_symbol(SectionId(7)), None);
}

#[test]
fn syncing_section_definitions() {
    let mut o = sample(Format::Regular);
    o.sections[1].data = SectionData::Bytes(Cow::Owned(alloc::vec![0; 100]));
    o.sections[5].relocations = alloc::vec![o.sections[5].relocations[0]; 70_000];
    o.sync_section_definitions();
    let d = |o: &Object<'_>, i: u32| {
        *o.symbols[(ids::SECTION_SYMBOLS + i) as usize]
            .section_definition()
            .unwrap()
    };
    assert_eq!(d(&o, 1).length, 100);
    assert_eq!(d(&o, 5).relocations, 0xFFFF);
    assert_eq!(d(&o, 0).line_numbers, 3);
    assert_eq!(d(&o, 2).length, 0x400);
    // What else they hold stays.
    assert_eq!(d(&o, 5).selection, comdat_select::ASSOCIATIVE);
    assert_eq!(d(&o, 5).number, Some(SectionId(4)));
    // An object's own definitions already agree.
    let data = testdata("objects.obj");
    let read = Object::parse(&data).unwrap();
    let mut synced = read.clone();
    synced.sync_section_definitions();
    assert_eq!(synced, read);
}

#[test]
fn folding_every_any_comdat_of_a_real_object() {
    // As turbolib's rewrite does: each COMDAT leader becomes an undefined
    // external, and its section goes with its associative sections.
    let data = testdata("objects.obj");
    let mut o = Object::parse(&data).unwrap();
    let names_before = targets(&o);
    let mut gone = alloc::vec![false; o.sections.len()];
    let mut leaders = Vec::new();
    for s in &o.symbols {
        if let (Some(d), Some(id)) = (s.section_definition(), s.section.id())
            && d.selection == comdat_select::ANY
        {
            gone[id.index()] = true;
            leaders.push(id);
        }
    }
    assert!(!leaders.is_empty());
    // Associative sections, transitively.
    loop {
        let mut more = false;
        for s in &o.symbols {
            if let (Some(d), Some(id)) = (s.section_definition(), s.section.id())
                && d.selection == comdat_select::ASSOCIATIVE
                && d.number.is_some_and(|n| gone[n.index()])
                && !gone[id.index()]
            {
                gone[id.index()] = true;
                more = true;
            }
        }
        if !more {
            break;
        }
    }
    // The leaders: the first symbol in each section after its definition.
    for &id in &leaders {
        let definition = o.section_symbol(id).unwrap().index();
        let leader = o.symbols[definition + 1..]
            .iter()
            .position(|s| s.section == SectionNumber::Section(id))
            .unwrap();
        undefine(&mut o, SymbolId((definition + 1 + leader) as u32));
    }
    let r = o
        .retain(
            |id, _| !gone[id.index()],
            |_, s| !s.section.id().is_some_and(|id| gone[id.index()]),
        )
        .unwrap();
    assert!(o.sections.len() < gone.len());
    // The relocations of the sections that stayed name what they did.
    let kept: Vec<_> = (0..gone.len())
        .filter(|&i| !gone[i])
        .map(|i| names_before[i].clone())
        .collect();
    assert_eq!(targets(&o), kept);
    for (i, g) in gone.iter().enumerate() {
        assert_eq!(r.section(SectionId(i as u32)).is_none(), *g);
    }
    round_trip(&o);
}

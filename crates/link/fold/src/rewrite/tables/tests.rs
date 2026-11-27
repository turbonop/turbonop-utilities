use super::super::{Input, Options, Pull, gathered as rewrite, units};
use super::*;
use std::collections::BTreeSet;
use turbonop_coff::{Aux, File, SectionNumber};

/// The configurations `tools/coff/tables.cmd` builds, each as
/// `<name>-one.obj` and `<name>-two.obj`.
const CONFIGURATIONS: &[&str] = &[
    "msvc-x64-cf",
    "msvc-x64-cf-nogy",
    "clang-x64-cf",
    "msvc-arm64-cf",
    "msvc-x86",
    "msvc-x86-nogy",
    "clang-x86",
    "msvc-x64-voltbl",
    "msvc-x64-voltbl-nogy",
    "msvc-x64-retpoline",
];

const RELAXED: Options = Options {
    pull: Pull::Relaxed,
    weak_externals: false,
    fold: crate::rewrite::Fold::Identical,
    locals: crate::rewrite::Locals::Keep,
    odr: false,
    precomp: crate::rewrite::Precomp::None,
};
const PRESERVE: Options = Options {
    pull: Pull::Preserve,
    weak_externals: false,
    fold: crate::rewrite::Fold::Identical,
    locals: crate::rewrite::Locals::Keep,
    odr: false,
    precomp: crate::rewrite::Precomp::None,
};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/coff/tables")
            .join(name),
    )
    .unwrap()
}

fn object(data: &[u8]) -> Object<'_> {
    match File::parse(data).unwrap() {
        File::Object(o) => o,
        File::Opaque { .. } => panic!("not a COFF object"),
    }
}

fn pair(config: &str) -> (Vec<u8>, Vec<u8>) {
    (
        fixture(&format!("{config}-one.obj")),
        fixture(&format!("{config}-two.obj")),
    )
}

/// What a section is, in words that survive a move: the first external
/// symbol defined in it (a COMDAT's own symbol), or its first symbol's
/// name.
fn section_name(o: &Object<'_>, id: SectionId) -> String {
    let in_it = || {
        o.symbols
            .iter()
            .filter(move |s| s.section == SectionNumber::Section(id))
    };
    in_it()
        .find(|s| s.is_external())
        .or_else(|| in_it().find(|s| s.section_definition().is_none()))
        .map_or_else(
            || show(o.sections[id.index()].name.as_bytes()),
            |s| show(s.name.as_bytes()),
        )
}

/// What a symbol is, in words that survive a move: an external's name, or
/// a local's name, section, and offset.
fn symbol_name(o: &Object<'_>, id: SymbolId) -> String {
    let s = &o.symbols[id.index()];
    match s.section.id() {
        Some(sec) if !s.is_external() => format!(
            "{} in {} +{:#x}",
            show(s.name.as_bytes()),
            section_name(o, sec),
            s.value
        ),
        _ => show(s.name.as_bytes()),
    }
}

/// One entry, in words: the table's name, what the entry names, and its
/// other bytes.
type Described = (String, String, Vec<u8>);

/// Every entry of every table of a member, described, in table and entry
/// order.
fn described(data: &[u8]) -> Vec<Described> {
    let o = object(data);
    let tables = find(&o).unwrap();
    let mut out = Vec::new();
    for t in &tables.tables {
        let sec = &o.sections[t.section.index()];
        let bytes = sec.bytes();
        for e in &t.entries {
            let mut other = bytes[e.start as usize..e.end as usize].to_vec();
            if let Some(f) = t.kind.field() {
                other[f..f + 4].fill(0);
            }
            if t.kind == Kind::Addrsig {
                other.clear();
            }
            let target = match e.target {
                Some(Target::Symbol(s)) => symbol_name(&o, s),
                Some(Target::Section(s)) => format!("section {}", section_name(&o, s)),
                None => String::new(),
            };
            out.push((show(sec.name.as_bytes()), target, other));
        }
    }
    out
}

/// Each COMDAT `.voltbl`'s bytes, with what it is associated with.
fn volatile_bytes(data: &[u8]) -> Vec<(String, Vec<u8>)> {
    let o = object(data);
    let mut out = Vec::new();
    for s in &o.symbols {
        if let (SectionNumber::Section(id), Aux::Section(d)) = (s.section, &s.aux)
            && o.sections[id.index()].name.as_bytes() == b".voltbl"
            && let Some(with) = d.number
        {
            out.push((
                section_name(&o, with),
                o.sections[id.index()].bytes().to_vec(),
            ));
        }
    }
    out
}

/// The absolute symbols named `@...` (`@feat.00`, `@comp.id`, `@vol.md`),
/// with their values.
fn feat00(data: &[u8]) -> Vec<(Vec<u8>, u32)> {
    object(data)
        .symbols
        .iter()
        .filter(|s| s.name.as_bytes().starts_with(b"@") && s.section == SectionNumber::Absolute)
        .map(|s| (s.name.as_bytes().to_vec(), s.value))
        .collect()
}

#[test]
fn every_fixture_s_tables_read() {
    for config in CONFIGURATIONS {
        let (a, b) = pair(config);
        for data in [&a, &b] {
            let o = object(data);
            let tables = find(&o).unwrap_or_else(|e| panic!("{config}: {e}"));
            let facts = units::find(&o).unwrap();
            assert!(
                facts
                    .units
                    .iter()
                    .all(|u| u.unfoldable != Some(Unfoldable::SideTables)),
                "{config}"
            );
            let kinds: BTreeSet<&[u8]> = tables
                .tables
                .iter()
                .map(|t| o.sections[t.section.index()].name.as_bytes())
                .collect();
            let expected: &[&[u8]] = match *config {
                "msvc-x64-cf" | "msvc-x64-cf-nogy" | "msvc-arm64-cf" => {
                    &[b".gfids$y", b".giats$y", b".gljmp$y", b".gehcont$y"]
                }
                "clang-x64-cf" => &[
                    b".gfids$y",
                    b".giats$y",
                    b".gljmp$y",
                    b".gehcont$y",
                    b".llvm_addrsig",
                ],
                "msvc-x86" | "msvc-x86-nogy" => &[b".gfids$y", b".sxdata", b".debug$F"],
                "clang-x86" => &[b".gfids$y", b".sxdata", b".llvm_addrsig"],
                "msvc-x64-voltbl" => &[],
                "msvc-x64-voltbl-nogy" => &[b".voltbl"],
                "msvc-x64-retpoline" => &[b".retplne"],
                _ => unreachable!(),
            };
            for e in expected {
                assert!(kinds.contains(e), "{config}: no {}", show(e));
            }
        }
    }
}

/// Folds a configuration's pair, and checks what every fold must keep:
/// every table still reads; every entry there was is still somewhere, and
/// none is made up; a set's entry for an external stays in its member;
/// each member keeps its `@feat.00` (and `@comp.id` and `@vol.md`), and a
/// synthesized member has the same. Gives the output members: the two rewritten ones, then the
/// synthesized ones.
fn check_folded(config: &str, options: Options) -> Vec<Vec<u8>> {
    let (a, b) = pair(config);
    let out = rewrite(
        &[
            Input {
                name: b"one.obj",
                data: &a,
            },
            Input {
                name: b"two.obj",
                data: &b,
            },
        ],
        options,
    );
    let r = &out.report;
    assert!(r.groups > 0, "{config}: {r:?}");
    assert!(
        !r.not_folded.contains_key(&Unfoldable::SideTables),
        "{config}: {r:?}"
    );
    assert!(out.members.len() > 2, "{config}: a synthesized member");
    // The rewritten members (in input order), then the synthesized ones,
    // wherever the output puts them.
    let datas: Vec<Vec<u8>> = (out.members.iter().filter(|m| !m.synthesized))
        .chain(out.members.iter().filter(|m| m.synthesized))
        .map(|m| m.data.to_vec())
        .collect();
    assert!(out.members.iter().filter(|m| !m.synthesized).count() == 2);
    for d in &datas {
        let o = object(d);
        find(&o).unwrap_or_else(|e| panic!("{config}: {e}"));
        assert!(
            units::find(&o)
                .unwrap()
                .units
                .iter()
                .all(|u| u.unfoldable != Some(Unfoldable::SideTables))
        );
    }

    let before: BTreeSet<Described> = [&a, &b].iter().flat_map(|d| described(d)).collect();
    let after: BTreeSet<Described> = datas.iter().flat_map(|d| described(d)).collect();
    let lost: Vec<_> = before.difference(&after).collect();
    let made: Vec<_> = after.difference(&before).collect();
    assert!(lost.is_empty(), "{config}: lost {lost:#?}");
    assert!(made.is_empty(), "{config}: made {made:#?}");

    let volatile_before: BTreeSet<_> = [&a, &b].iter().flat_map(|d| volatile_bytes(d)).collect();
    let volatile_after: BTreeSet<_> = datas.iter().flat_map(|d| volatile_bytes(d)).collect();
    assert_eq!(volatile_before, volatile_after, "{config}");

    for (i, input) in [&a, &b].into_iter().enumerate() {
        let o = object(input);
        let kept: BTreeSet<Described> = described(&datas[i]).into_iter().collect();
        let mut was = described(input).into_iter();
        for t in &find(&o).unwrap().tables {
            for e in &t.entries {
                let d = was.next().unwrap();
                if t.kind == Kind::Indices
                    && !t.comdat
                    && let Some(Target::Symbol(s)) = e.target
                    && o.symbols[s.index()].is_external()
                {
                    assert!(kept.contains(&d), "{config}: member {i} lost {d:?}");
                }
            }
        }
        assert_eq!(feat00(&datas[i]), feat00(input), "{config}");
    }
    for d in &datas[2..] {
        assert_eq!(feat00(d), feat00(&a), "{config}: synthesized");
    }
    datas
}

#[test]
fn folding_keeps_every_entry() {
    for config in CONFIGURATIONS {
        check_folded(config, RELAXED);
        check_folded(config, PRESERVE);
    }
}

/// The entries of the synthesized members (those after the two rewritten
/// ones), as (table, target).
fn synthesized_entries(datas: &[Vec<u8>]) -> BTreeSet<(String, String)> {
    datas[2..]
        .iter()
        .flat_map(|d| described(d))
        .map(|(t, n, _)| (t, n))
        .collect()
}

fn names(data: &[u8]) -> BTreeSet<(String, String)> {
    described(data)
        .into_iter()
        .map(|(t, n, _)| (t, n))
        .collect()
}

fn entry(table: &str, target: &str) -> (String, String) {
    (table.into(), target.into())
}

#[test]
fn msvc_entries_move_with_their_functions() {
    for config in ["msvc-x64-cf", "msvc-x64-cf-nogy"] {
        let datas = check_folded(config, RELAXED);
        let s = synthesized_entries(&datas);
        for e in [
            entry(".gfids$y", "?add@@YAHHH@Z"),
            entry(".gfids$y", "??$twice@H@@YAHH@Z"),
            entry(".giats$y", "__imp_GetTickCount"),
            entry(".gljmp$y", "$LN8 in ?jumps@@YAHH@Z +0x1c"),
            entry(".gehcont$y", "$LN8 in ?throws@@YAHH@Z +0xf"),
            entry(".gfids$y", "?area@square@@UEBAHXZ"),
            entry(".gfids$y", "??_Esquare@@UEAAPEAXI@Z"),
        ] {
            assert!(s.contains(&e), "{config}: {e:?} in {s:#?}");
        }
        // The rewritten members keep the entries of their own functions,
        // and none for a folded function's labels.
        let one = names(&datas[0]);
        assert!(
            one.iter()
                .any(|(t, n)| t == ".gfids$y" && n.starts_with("?seven@@YAHH@Z in")),
            "{config}: {one:#?}"
        );
        assert!(!one.iter().any(|(_, n)| n.contains("?jumps@@YAHH@Z")));
    }
    // With /Gy, the synthesized member's tables are COMDATs associated
    // with their functions, as they were.
    let datas = check_folded("msvc-x64-cf", RELAXED);
    let o = object(&datas[2]);
    assert!(find(&o).unwrap().tables.iter().all(|t| t.comdat));
}

#[test]
fn one_table_per_object_is_split() {
    // clang-cl: the labels go with their functions; the address-taken
    // externals stay, and are copied to the synthesized member for the
    // functions that refer to them.
    let datas = check_folded("clang-x64-cf", RELAXED);
    let s = synthesized_entries(&datas);
    let one = names(&datas[0]);
    let labels = |set: &BTreeSet<(String, String)>, table: &str| {
        set.iter()
            .filter(|(t, n)| t == table && n.starts_with('$'))
            .count()
    };
    assert!(labels(&s, ".gljmp$y") > 0, "{s:#?}");
    assert!(labels(&s, ".gehcont$y") > 0, "{s:#?}");
    assert_eq!(labels(&one, ".gljmp$y"), 0, "{one:#?}");
    assert_eq!(labels(&one, ".gehcont$y"), 0, "{one:#?}");
    for e in [
        entry(".gfids$y", "?add@@YAHHH@Z"),
        entry(".gfids$y", "??$twice@H@@YAHH@Z"),
        entry(".gfids$y", "GetTickCount"),
        entry(".giats$y", "__imp_GetTickCount"),
    ] {
        assert!(s.contains(&e), "{e:?} in {s:#?}");
        assert!(one.contains(&e), "{e:?} kept");
    }
    // The synthesized member's tables are outside COMDATs, as clang's are.
    let o = object(&datas[2]);
    assert!(find(&o).unwrap().tables.iter().all(|t| !t.comdat));
}

#[test]
fn x86_handlers_and_fpo_move_with_their_functions() {
    for config in ["msvc-x86", "msvc-x86-nogy", "clang-x86"] {
        let datas = check_folded(config, RELAXED);
        let s = synthesized_entries(&datas);
        assert!(
            s.iter()
                .any(|(t, n)| t == ".sxdata" && n.contains("__ehhandler$?throws")),
            "{config}: {s:#?}"
        );
        let one = names(&datas[0]);
        assert!(
            !one.iter().any(|(_, n)| n.contains("__ehhandler$?throws")),
            "{config}"
        );
        if config.starts_with("msvc") {
            assert!(
                s.contains(&entry(".sxdata", "__except_handler4")),
                "{config}: {s:#?}"
            );
            // FPO for each folded function, and none left behind for it.
            assert!(
                s.contains(&entry(".debug$F", "?pick@@YAHH@Z")),
                "{config}: {s:#?}"
            );
            assert!(!one.contains(&entry(".debug$F", "?pick@@YAHH@Z")));
            assert!(one.contains(&entry(".debug$F", "?one@@YAHH@Z")));
        }
    }
}

#[test]
fn volatile_and_retpoline_records_move_with_their_code() {
    let datas = check_folded("msvc-x64-retpoline", RELAXED);
    let s = synthesized_entries(&datas);
    assert!(
        s.contains(&entry(".retplne", "section ?pick@@YAHH@Z")),
        "{s:#?}"
    );
    assert!(s.contains(&entry(".retplne", "section ?call@@YAHP6AHH@ZH@Z")));
    let one = names(&datas[0]);
    assert!(one.contains(&entry(".retplne", "section ?one@@YAHH@Z")));
    assert!(!one.contains(&entry(".retplne", "section ?pick@@YAHH@Z")));
    let o = object(&datas[2]);
    let t = &find(&o).unwrap().tables[0];
    assert_eq!(
        &o.sections[t.section.index()].bytes()[..RETPOLINE_MAGIC.len()],
        RETPOLINE_MAGIC
    );

    check_folded("msvc-x64-voltbl", RELAXED);
    let datas = check_folded("msvc-x64-voltbl-nogy", RELAXED);
    assert!(names(&datas[0]).contains(&entry(".voltbl", "?one@@YAHH@Z")));
}

#[test]
fn identical_tables_hash_alike() {
    // Each copy names its targets by its own member's indices, and the
    // copies are still the same.
    let (a, b) = pair("msvc-x64-cf");
    let (oa, ob) = (object(&a), object(&b));
    let (fa, fb) = (units::find(&oa).unwrap(), units::find(&ob).unwrap());
    let unit = |f: &units::Facts, name: &[u8]| {
        let u = f
            .units
            .iter()
            .find(|u| u.externals.iter().any(|e| e == name))
            .unwrap();
        assert!(
            u.sections.iter().any(|&s| f.tables.get(s).is_some()),
            "{} has a table",
            show(name)
        );
        assert_eq!(u.unfoldable, None, "{}", show(name));
        u.identity
    };
    for name in [
        &b"?add_address@@YAP6AHHH@ZXZ"[..],
        b"?jumps@@YAHH@Z",
        b"?throws@@YAHH@Z",
        b"??_7square@@6B@",
    ] {
        assert_eq!(unit(&fa, name), unit(&fb, name), "{}", show(name));
    }
}

/// `data` with the first section named `name` given `bytes`.
fn with_bytes(data: &[u8], name: &[u8], bytes: Vec<u8>) -> Vec<u8> {
    let mut o = object(data);
    let s = o
        .sections
        .iter_mut()
        .find(|s| s.name.as_bytes() == name)
        .unwrap();
    s.data = SectionData::Bytes(Cow::Owned(bytes));
    o.sync_section_definitions();
    o.write().unwrap()
}

/// Whether a member's tables are refused, so that it folds nothing.
fn passes_through(data: &[u8]) -> bool {
    let o = object(data);
    assert!(find(&o).is_err());
    let facts = units::find(&o).unwrap();
    !facts.units.is_empty()
        && facts
            .units
            .iter()
            .all(|u| u.unfoldable == Some(Unfoldable::SideTables) && u.detail.is_some())
}

#[test]
fn tables_turbolib_can_t_read_make_the_member_pass_through() {
    let (a, _) = pair("msvc-x64-cf");
    // An index naming an auxiliary record (the .drectve's section
    // definition), one past the symbol table, and a partial index.
    assert!(passes_through(&with_bytes(
        &a,
        b".gfids$y",
        3u32.to_le_bytes().to_vec()
    )));
    assert!(passes_through(&with_bytes(
        &a,
        b".gfids$y",
        u32::MAX.to_le_bytes().to_vec()
    )));
    assert!(passes_through(&with_bytes(&a, b".gfids$y", vec![0; 3])));

    let (r, _) = pair("msvc-x64-retpoline");
    assert!(passes_through(&with_bytes(
        &r,
        b".retplne",
        b"RetpolineV2\0".to_vec()
    )));
    let mut bad = RETPOLINE_MAGIC.to_vec();
    bad.extend_from_slice(&[6, 0, 0, 0, 1, 0, 0, 0]);
    assert!(passes_through(&with_bytes(&r, b".retplne", bad)));
    let mut bad = RETPOLINE_MAGIC.to_vec();
    bad.extend_from_slice(&[16, 0, 0, 0, 0xFF, 0xFF, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0]);
    assert!(passes_through(&with_bytes(&r, b".retplne", bad)));

    let (v, _) = pair("msvc-x64-voltbl-nogy");
    let mut o = object(&v);
    let t = o
        .sections
        .iter_mut()
        .find(|s| s.name.as_bytes() == b".voltbl" && s.characteristics & IMAGE_SCN_LNK_COMDAT == 0)
        .unwrap();
    t.data = SectionData::Bytes(Cow::Owned(vec![1, 2, 3, 4]));
    o.sync_section_definitions();
    assert!(passes_through(&o.write().unwrap()));

    // FPO with a relocation that isn't at a record's start.
    let (x, _) = pair("msvc-x86");
    let mut o = object(&x);
    let f = o
        .sections
        .iter_mut()
        .find(|s| s.name.as_bytes() == b".debug$F")
        .unwrap();
    f.relocations[0].offset = 4;
    assert!(passes_through(&o.write().unwrap()));

    // A name that starts as a table's does but isn't one.
    let mut o = object(&a);
    let g = o
        .sections
        .iter_mut()
        .find(|s| s.name.as_bytes() == b".gfids$y")
        .unwrap();
    g.name = turbonop_coff::Name::new(b".gfids$yy".to_vec());
    assert!(passes_through(&o.write().unwrap()));
}

#[test]
fn a_table_of_code_that_stays_keeps_a_unit_s_label_from_folding() {
    // jumps's .gljmp$y names its $LN8 label. Associated with one() (which
    // stays) instead, it can't give the entry away, so jumps can't fold.
    let (a, _) = pair("msvc-x64-cf");
    let mut o = object(&a);
    let one = o
        .symbols
        .iter()
        .find(|s| s.name.as_bytes() == b"?one@@YAHH@Z")
        .and_then(|s| s.section.id())
        .unwrap();
    let table = o
        .sections
        .iter()
        .position(|s| s.name.as_bytes() == b".gljmp$y")
        .unwrap();
    let def = o
        .symbols
        .iter_mut()
        .find(|s| s.section == SectionNumber::Section(SectionId(table as u32)))
        .and_then(|s| s.section_definition_mut())
        .unwrap();
    def.number = Some(one);
    let data = o.write().unwrap();
    let jumps = |data: &[u8]| {
        units::find(&object(data))
            .unwrap()
            .units
            .into_iter()
            .find(|u| u.externals.iter().any(|e| e == b"?jumps@@YAHH@Z"))
            .unwrap()
            .unfoldable
    };
    assert_eq!(jumps(&data), Some(Unfoldable::ReferencedLocally));

    // Outside COMDATs, the entry goes with jumps, which folds.
    let mut o = object(&data);
    o.sections[table].characteristics &= !IMAGE_SCN_LNK_COMDAT;
    let def = o
        .symbols
        .iter_mut()
        .find(|s| s.section == SectionNumber::Section(SectionId(table as u32)))
        .and_then(|s| s.section_definition_mut())
        .unwrap();
    def.selection = 0;
    def.number = None;
    assert_eq!(jumps(&o.write().unwrap()), None);
}

//! Phase 4's less conservative folding: `--fold=select-any` with `--pick`,
//! `--locals=copy` and `merge`, and `--report odr`, on the fixtures that
//! `tools/turbolib/loose/build.cmd` builds (`testdata/coff/loose`) and on
//! objects made here.

use super::locals::tests::{build, comdat, data, reloc};
use super::*;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/coff/loose")
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

fn options(fold: Fold, locals: Locals, pull: Pull) -> Options {
    Options {
        pull,
        weak_externals: false,
        fold,
        locals,
        precomp: Precomp::None,
        odr: false,
    }
}

const SELECT_ANY: Fold = Fold::SelectAny(Pick::First);

/// The three objects of a build of tools/turbolib/loose, as members.
fn loose(prefix: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    ["a", "b", "c"]
        .iter()
        .map(|m| {
            (
                format!("{m}.obj").into_bytes(),
                fixture(&format!("{prefix}-{m}.obj")),
            )
        })
        .collect()
}

fn run(members: &[(Vec<u8>, Vec<u8>)], o: Options) -> Output<'_> {
    let inputs: Vec<Input<'_>> = members
        .iter()
        .map(|(name, data)| Input { name, data })
        .collect();
    rewrite(&inputs, o)
}

/// The output members (by position) that define `name`.
fn definers(out: &Output<'_>, name: &[u8]) -> Vec<usize> {
    out.members
        .iter()
        .enumerate()
        .filter(|(_, m)| {
            object(&m.data)
                .symbols
                .iter()
                .any(|s| s.is_external() && s.section.id().is_some() && s.name == name)
        })
        .map(|(i, _)| i)
        .collect()
}

fn synthesized(out: &Output<'_>) -> Vec<usize> {
    (0..out.members.len())
        .filter(|&i| out.members[i].synthesized)
        .collect()
}

/// Every output member is an object turbolib reads back.
fn readable(out: &Output<'_>) {
    for m in &out.members {
        let o = object(&m.data);
        units::find(&o).unwrap();
    }
}

const VARIANT: &[u8] = b"?variant@@YAHXZ";
const REFERS: &[u8] = b"?refers@@YAHXZ";

#[test]
fn select_any_folds_copies_whose_code_differs() {
    let members = loose("msvc-od");
    // Identical: b's and c's variant() fold, a's (VARIANT 1) stays.
    let out = run(
        &members,
        options(Fold::Identical, Locals::Keep, Pull::Relaxed),
    );
    let d = definers(&out, VARIANT);
    assert_eq!(d.len(), 2, "{d:?}");
    assert!(d.iter().any(|&i| !out.members[i].synthesized));
    // Select-any: one copy, in a synthesized member.
    let out = run(&members, options(SELECT_ANY, Locals::Keep, Pull::Relaxed));
    let d = definers(&out, VARIANT);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(out.members[d[0]].synthesized);
    assert!(
        out.report.differing_groups >= 3,
        "variant, selected, refers"
    );
    assert_eq!(definers(&out, REFERS).len(), 1);
    readable(&out);
}

#[test]
fn select_any_under_preserve_keeps_copies_that_refer_to_other_names() {
    let members = loose("msvc-od");
    let out = run(&members, options(SELECT_ANY, Locals::Keep, Pull::Preserve));
    // variant() refers to nothing else: it folds.
    let d = definers(&out, VARIANT);
    assert_eq!(d.len(), 1);
    assert!(out.members[d[0]].synthesized);
    // refers() calls ext_one in a and ext_two in b and c: keeping one
    // copy for all could load other members, so none moves.
    let d = definers(&out, REFERS);
    assert_eq!(d.len(), 3, "{d:?}");
    assert!(d.iter().all(|&i| !out.members[i].synthesized));
    assert!(
        out.report
            .not_folded
            .contains_key(&Unfoldable::DifferingReferences),
        "{:?}",
        out.report.not_folded
    );
    readable(&out);
}

/// An object with one COMDAT `f` of `selection` holding `code`, with
/// `checksum` in its definition.
fn selected(selection: u8, code: Vec<u8>, checksum: u32) -> Vec<u8> {
    build(|o| {
        let (text, _) = comdat(o, b"f", true, code);
        for s in &mut o.symbols {
            if s.section == SectionNumber::Section(text)
                && let Aux::Section(d) = &mut s.aux
            {
                d.selection = selection;
                d.checksum = checksum;
            }
        }
    })
}

fn pair(a: Vec<u8>, b: Vec<u8>) -> Vec<(Vec<u8>, Vec<u8>)> {
    vec![(b"x.obj".to_vec(), a), (b"y.obj".to_vec(), b)]
}

/// The size of `f`'s section in the member that defines it last.
fn kept_size(out: &Output<'_>) -> u32 {
    let d = definers(out, b"f");
    let o = object(&out.members[*d.last().unwrap()].data);
    let s = o
        .symbols
        .iter()
        .find(|s| s.name == b"f" && s.section.id().is_some())
        .unwrap();
    o.sections[s.section.id().unwrap().index()].size()
}

#[test]
fn each_selection_by_its_rule() {
    let relaxed = |fold| options(fold, Locals::Keep, Pull::Relaxed);
    // ANY: the first, or the largest.
    let any = pair(
        selected(comdat_select::ANY, vec![0x90, 0xC3], 0),
        selected(comdat_select::ANY, vec![0x90, 0x90, 0x90, 0xC3], 0),
    );
    let out = run(&any, relaxed(SELECT_ANY));
    assert_eq!(out.report.groups, 1);
    assert_eq!(definers(&out, b"f").len(), 1);
    assert_eq!(kept_size(&out), 2);
    let out = run(&any, relaxed(Fold::SelectAny(Pick::Largest)));
    assert_eq!(kept_size(&out), 4);
    // Identical folding leaves them.
    assert_eq!(run(&any, relaxed(Fold::Identical)).report.groups, 0);
    // LARGEST: the largest, whatever the pick.
    let largest = pair(
        selected(comdat_select::LARGEST, vec![0x90, 0xC3], 0),
        selected(comdat_select::LARGEST, vec![0x90, 0x90, 0x90, 0xC3], 0),
    );
    let out = run(&largest, relaxed(SELECT_ANY));
    assert_eq!(out.report.groups, 1);
    assert_eq!(kept_size(&out), 4);
    // SAME_SIZE: folded when the sizes agree, else left and reported.
    let same = pair(
        selected(comdat_select::SAME_SIZE, vec![0x90, 0xC3], 0),
        selected(comdat_select::SAME_SIZE, vec![0xCC, 0xC3], 0),
    );
    let out = run(&same, relaxed(SELECT_ANY));
    assert_eq!((out.report.groups, out.report.mismatches.len()), (1, 0));
    let other = pair(
        selected(comdat_select::SAME_SIZE, vec![0x90, 0xC3], 0),
        selected(comdat_select::SAME_SIZE, vec![0x90, 0x90, 0xC3], 0),
    );
    let out = run(&other, relaxed(SELECT_ANY));
    assert_eq!((out.report.groups, out.report.mismatches.len()), (0, 1));
    assert_eq!(definers(&out, b"f").len(), 2);
    assert_eq!(out.report.mismatches[0].names, [b"f".to_vec()]);
    // EXACT_MATCH: folded when the checksums (and sizes) agree.
    let exact = pair(
        selected(comdat_select::EXACT_MATCH, vec![0x90, 0xC3], 7),
        selected(comdat_select::EXACT_MATCH, vec![0xCC, 0xC3], 7),
    );
    assert_eq!(run(&exact, relaxed(SELECT_ANY)).report.groups, 1);
    let differ = pair(
        selected(comdat_select::EXACT_MATCH, vec![0x90, 0xC3], 7),
        selected(comdat_select::EXACT_MATCH, vec![0xCC, 0xC3], 8),
    );
    let out = run(&differ, relaxed(SELECT_ANY));
    assert_eq!((out.report.groups, out.report.mismatches.len()), (0, 1));
    // Copies that disagree leave identical ones to fold as identical.
    let mut three = other.clone();
    three.push((
        b"z.obj".to_vec(),
        selected(comdat_select::SAME_SIZE, vec![0x90, 0xC3], 0),
    ));
    let out = run(&three, relaxed(SELECT_ANY));
    assert_eq!((out.report.groups, out.report.mismatches.len()), (1, 1));
    assert_eq!(definers(&out, b"f").len(), 2, "y's, and x's and z's one");
    // Other selections don't fold together.
    let mixed = pair(
        selected(comdat_select::ANY, vec![0x90, 0xC3], 0),
        selected(comdat_select::LARGEST, vec![0x90, 0x90, 0xC3], 0),
    );
    assert_eq!(run(&mixed, relaxed(SELECT_ANY)).report.groups, 0);
    // Under preserve, a name defined elsewhere by something other than a
    // copy of its kind stays.
    let out = run(&mixed, options(SELECT_ANY, Locals::Keep, Pull::Preserve));
    assert_eq!(out.report.groups, 0);
}

const LOOKUP: &[u8] = b"?lookup@@YAHH@Z";
const DOUBLED: &[u8] = b"?doubled@@YAHH@Z";
const TRIPLED: &[u8] = b"?tripled@@YAHH@Z";
const BUMP: &[u8] = b"?bump@@YAHXZ";
const TWICE: &[u8] = b"?twice@@YAHH@Z";

/// The static symbols of an output member named `name`.
fn statics(out: &Output<'_>, i: usize, name: &[u8]) -> usize {
    object(&out.members[i].data)
        .symbols
        .iter()
        .filter(|s| !s.is_external() && s.name == name)
        .count()
}

#[test]
fn locals_copy_folds_units_that_reach_locals() {
    let members = loose("msvc-od");
    for pull in [Pull::Relaxed, Pull::Preserve] {
        // Kept, units that reach locals stay.
        let keep = run(&members, options(Fold::Identical, Locals::Keep, pull));
        for name in [LOOKUP, DOUBLED, TRIPLED, BUMP] {
            assert_eq!(definers(&keep, name).len(), 3);
        }
        for locals in [Locals::Copy, Locals::Merge] {
            let out = run(&members, options(Fold::Identical, locals, pull));
            readable(&out);
            for name in [LOOKUP, DOUBLED, TRIPLED] {
                let d = definers(&out, name);
                assert_eq!(d.len(), 1, "{}", String::from_utf8_lossy(name));
                assert!(out.members[d[0]].synthesized);
            }
            // What reaches writable data stays.
            let d = definers(&out, BUMP);
            assert_eq!(d.len(), 3);
            // The static inline function went with what reached it: the
            // members have none of it left, and a synthesized member has a
            // copy that doubled() calls.
            let r = &out.report.locals;
            assert!(r.absorbed >= 6, "twice and thrice in each: {r:?}");
            assert!(r.units_copied >= 2 && r.pieces_copied >= 1, "{r:?}");
            for (i, m) in out.members.iter().enumerate() {
                if !m.synthesized {
                    assert_eq!(statics(&out, i, TWICE), 0, "{i}");
                }
            }
            let s = definers(&out, DOUBLED)[0];
            assert_eq!(statics(&out, s, TWICE), 1);
            let o = object(&out.members[s].data);
            let doubled = o
                .symbols
                .iter()
                .find(|s| s.name == DOUBLED)
                .unwrap()
                .section
                .id()
                .unwrap();
            let called = &o.symbols[o.sections[doubled.index()].relocations[0].symbol.index()];
            assert_eq!(called.name, TWICE);
            assert!(called.section.id().is_some(), "the copy, defined here");
            // kTable's bytes, copied for lookup().
            let s = definers(&out, LOOKUP)[0];
            let o = object(&out.members[s].data);
            let table: Vec<u8> = [2u32, 3, 5, 7, 11, 13]
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect();
            assert!(
                o.sections
                    .iter()
                    .any(|x| x.name == b".rdata" && x.bytes() == table.as_slice()),
                "kTable copied"
            );
        }
    }
}

#[test]
fn locals_copy_without_gy_leaves_static_functions() {
    // Without /Gy, static functions are code outside COMDATs: what calls
    // them stays; lookup()'s table is still copied.
    let members = loose("msvc-odnogy");
    let out = run(
        &members,
        options(Fold::Identical, Locals::Copy, Pull::Relaxed),
    );
    readable(&out);
    assert_eq!(definers(&out, DOUBLED).len(), 3);
    assert_eq!(definers(&out, LOOKUP).len(), 1);
}

/// A member with `f1` and `f2`, each reading the same string, `f2` with
/// `extra` more bytes of code.
fn two_readers(extra: usize) -> Vec<u8> {
    build(|o| {
        let s = data(o, b".rdata", 0x4030_0040, b"$SG5", 0, b"shared\0", 1);
        let (t1, _) = comdat(o, b"f1", true, vec![0x48, 0x8D, 0x05, 0, 0, 0, 0, 0xC3]);
        reloc(o, t1, 3, s);
        let mut code = vec![0x48, 0x8D, 0x05, 0, 0, 0, 0];
        code.extend(std::iter::repeat_n(0x90, extra));
        code.push(0xC3);
        let (t2, _) = comdat(o, b"f2", true, code);
        reloc(o, t2, 3, s);
    })
}

/// The sections of `out`'s synthesized members named `.rdata`.
fn copied_pieces(out: &Output<'_>) -> usize {
    synthesized(out)
        .iter()
        .map(|&i| {
            object(&out.members[i].data)
                .sections
                .iter()
                .filter(|s| s.name == b".rdata")
                .count()
        })
        .sum()
}

#[test]
fn merge_makes_one_copy_of_equal_locals() {
    // f1's survivor is x's, f2's (the largest) y's: one synthesized
    // member holds both, each reaching its own member's string.
    let members = pair(two_readers(0), two_readers(3));
    let largest = Fold::SelectAny(Pick::Largest);
    let copy = run(&members, options(largest, Locals::Copy, Pull::Relaxed));
    assert_eq!(synthesized(&copy).len(), 1);
    assert_eq!(copied_pieces(&copy), 2);
    let merge = run(&members, options(largest, Locals::Merge, Pull::Relaxed));
    assert_eq!(copied_pieces(&merge), 1);
    readable(&merge);
    // Both reach the one copy.
    let o = object(&merge.members[synthesized(&merge)[0]].data);
    let targets: Vec<&[u8]> = o
        .sections
        .iter()
        .filter(|s| s.name == b".text$mn")
        .map(|s| o.symbols[s.relocations[0].symbol.index()].name.as_bytes())
        .collect();
    assert_eq!(targets, [b"$SG5", b"$SG5"]);
    // In one member, the same string is one copy either way.
    let one = pair(two_readers(0), two_readers(0));
    let out = run(&one, options(Fold::Identical, Locals::Copy, Pull::Relaxed));
    assert_eq!(copied_pieces(&out), 1);
}

#[test]
fn a_pinned_local_unit_stays_and_is_copied() {
    let member = |n: &'static [u8]| {
        super::locals::tests::caller(n, |o, _, h| {
            let g = o.push_section(turbonop_coff::Section::new(
                Name::new(b".text$mn".to_vec()),
                0x6050_0020,
                SectionData::Bytes(Cow::Owned(vec![0xE8, 0, 0, 0, 0, 0xC3])),
            ));
            reloc(o, g, 1, h);
        })
    };
    let members = pair(member(b"?h@?A0x1@@YAXXZ"), member(b"?h@?A0x2@@YAXXZ"));
    let out = run(
        &members,
        options(Fold::Identical, Locals::Copy, Pull::Relaxed),
    );
    readable(&out);
    let d = definers(&out, b"f");
    assert_eq!(d.len(), 1);
    assert_eq!(out.report.locals.absorbed, 0);
    assert_eq!(out.report.locals.units_copied, 1);
    let at = |name: &[u8]| {
        out.members
            .iter()
            .position(|m| m.name.as_ref() == name)
            .unwrap()
    };
    assert_eq!(statics(&out, at(b"x.obj"), b"?h@?A0x1@@YAXXZ"), 1);
    assert_eq!(statics(&out, at(b"y.obj"), b"?h@?A0x2@@YAXXZ"), 1);
    assert_eq!(statics(&out, d[0], b"?h@?A0x1@@YAXXZ"), 1, "x's, copied");
}

#[test]
fn report_odr_lists_the_names_whose_definitions_differ() {
    let members = loose("msvc-od");
    let mut o = options(Fold::None, Locals::Keep, Pull::Preserve);
    o.odr = true;
    let out = run(&members, o);
    let names: Vec<&[u8]> = out
        .report
        .odr
        .iter()
        .map(|f| f.names[0].as_slice())
        .collect();
    for want in [VARIANT, REFERS, b"?selected@@3HA".as_slice()] {
        assert!(names.contains(&want), "{}", String::from_utf8_lossy(want));
    }
    // Units that differ only in which translation unit's locals they name
    // are the same.
    for same in [LOOKUP, DOUBLED, TRIPLED] {
        assert!(!names.contains(&same), "{}", String::from_utf8_lossy(same));
    }
    let v = out
        .report
        .odr
        .iter()
        .find(|f| f.names[0] == VARIANT)
        .unwrap();
    assert_eq!(v.variants.len(), 2);
    assert_eq!(v.variants[0].members, [b"a.obj".to_vec()]);
    assert_eq!(
        v.variants[1].members,
        [b"b.obj".to_vec(), b"c.obj".to_vec()]
    );
    assert!(v.same_environment);
    // Nothing folded, nothing changed.
    assert_eq!(out.report.groups, 0);
    for (m, (_, data)) in out.members.iter().zip(&members) {
        assert_eq!(m.data.as_ref(), data.as_slice());
    }
}

#[test]
fn the_output_is_the_same_every_time() {
    let members = loose("msvc-od");
    for (fold, locals) in [
        (SELECT_ANY, Locals::Keep),
        (Fold::Identical, Locals::Copy),
        (Fold::SelectAny(Pick::Largest), Locals::Merge),
    ] {
        let x = run(&members, options(fold, locals, Pull::Preserve));
        let y = run(&members, options(fold, locals, Pull::Preserve));
        assert_eq!(x.members.len(), y.members.len());
        for (m, n) in x.members.iter().zip(&y.members) {
            assert_eq!(m.data, n.data);
            assert_eq!(m.holes, n.holes);
        }
    }
}

#[test]
fn the_other_compilers_fixtures_fold() {
    for prefix in ["msvc-o2", "clang-od"] {
        let members = loose(prefix);
        for (fold, locals) in [
            (SELECT_ANY, Locals::Copy),
            (Fold::SelectAny(Pick::Largest), Locals::Merge),
        ] {
            let out = run(&members, options(fold, locals, Pull::Relaxed));
            readable(&out);
            assert_eq!(definers(&out, VARIANT).len(), 1, "{prefix}");
        }
    }
}

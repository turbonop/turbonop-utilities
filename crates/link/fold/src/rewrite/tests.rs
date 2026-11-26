use super::*;
use std::collections::{HashMap, HashSet};

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

/// External definitions in each of `datas`, by name.
fn defined(datas: &[&[u8]]) -> HashMap<Vec<u8>, Vec<usize>> {
    let mut out: HashMap<Vec<u8>, Vec<usize>> = HashMap::new();
    for (i, d) in datas.iter().enumerate() {
        for s in &object(d).symbols {
            if s.is_external() && s.section != SectionNumber::Undefined {
                out.entry(s.name.as_bytes().to_vec()).or_default().push(i);
            }
        }
    }
    out
}

fn undefined(data: &[u8]) -> HashSet<Vec<u8>> {
    object(data)
        .symbols
        .iter()
        .filter(|s| s.section == SectionNumber::Undefined && s.is_external())
        .map(|s| s.name.as_bytes().to_vec())
        .collect()
}

/// Two members with the same code (objects.obj in both formats).
fn pair() -> (Vec<u8>, Vec<u8>) {
    (fixture("objects.obj"), fixture("objects-big.obj"))
}

fn inputs<'a>(members: &[(&'a [u8], &'a [u8])]) -> Vec<Input<'a>> {
    members
        .iter()
        .map(|&(name, data)| Input { name, data })
        .collect()
}

const RELAXED: Options = Options {
    pull: Pull::Relaxed,
    weak_externals: false,
};
const PRESERVE: Options = Options {
    pull: Pull::Preserve,
    weak_externals: false,
};

#[test]
fn identical_copies_fold_into_one_synthesized_member() {
    let (a, b) = pair();
    let out = rewrite(&inputs(&[(b"a.obj", &a), (b"b.obj", &b)]), PRESERVE);
    let r = &out.report;
    assert!(r.groups > 0, "{r:?}");
    assert_eq!(r.copies_removed, 2 * r.groups);
    assert_eq!(r.rewritten, 2);
    assert_eq!(r.synthesized, 1, "one member set: both members");
    assert!(r.bytes_out < r.bytes_in);
    assert_eq!(out.members.len(), 3);
    assert!(out.members[0].synthesized, "synthesized members first");
    assert_eq!(&*out.members[1].name, b"a.obj");
    assert_eq!(&*out.members[2].name, b"b.obj");

    // Every name defined before is defined after, and a folded one only in
    // the synthesized member.
    let before = defined(&[&a, &b]);
    let datas: Vec<&[u8]> = out.members.iter().map(|m| &*m.data).collect();
    let after = defined(&datas);
    for name in before.keys() {
        assert!(
            after.contains_key(name),
            "{}",
            String::from_utf8_lossy(name)
        );
    }
    for (name, members) in &after {
        if members.contains(&0) {
            assert_eq!(
                members,
                &[0],
                "{} folded once",
                String::from_utf8_lossy(name)
            );
        }
    }
    // Each rewritten member refers to the synthesized member's anchor.
    let synth = object(&out.members[0].data);
    let anchor: Vec<u8> = synth
        .symbols
        .iter()
        .find(|s| s.name.as_bytes().starts_with(b"__turbolib$anchor$"))
        .map(|s| s.name.as_bytes().to_vec())
        .expect("an anchor");
    for m in &out.members[1..] {
        assert!(undefined(&m.data).contains(&anchor));
    }
    for m in &out.members {
        assert_eq!(object(&m.data).machine, 0x8664);
    }
}

#[test]
fn the_output_is_the_same_every_time() {
    let (a, b) = pair();
    let i = inputs(&[(b"a.obj", &a), (b"b.obj", &b)]);
    let x = rewrite(&i, PRESERVE);
    let y = rewrite(&i, PRESERVE);
    assert_eq!(x.members.len(), y.members.len());
    for (m, n) in x.members.iter().zip(&y.members) {
        assert_eq!(m.data, n.data);
    }
}

#[test]
fn a_single_member_is_left_alone() {
    let a = fixture("objects.obj");
    let out = rewrite(&inputs(&[(b"a.obj", &a)]), PRESERVE);
    assert_eq!(out.report.groups, 0);
    assert_eq!(out.members.len(), 1);
    assert!(matches!(out.members[0].data, Cow::Borrowed(_)));
}

/// `data` with a non-COMDAT `.CRT$XCU` section: a member with side effects.
fn with_initializer(data: &[u8]) -> Vec<u8> {
    let mut o = object(data).into_owned();
    o.push_section(Section::new(
        Name::new(b".CRT$XCU".to_vec()),
        0x4030_0040,
        SectionData::Bytes(Cow::Owned(vec![0; 8])),
    ));
    o.write().unwrap()
}

#[test]
fn preserve_holds_back_groups_whose_first_definer_has_side_effects() {
    let (a, b) = pair();
    let a = with_initializer(&a);
    let first = inputs(&[(b"a.obj", &a), (b"b.obj", &b)]);
    let out = rewrite(&first, PRESERVE);
    assert_eq!(out.report.groups, 0);
    assert!(out.report.not_folded.contains_key(&Unfoldable::SideEffects));
    // With the side effects second, the first definer has none.
    let second = inputs(&[(b"b.obj", &b), (b"a.obj", &a)]);
    assert!(rewrite(&second, PRESERVE).report.groups > 0);
    // Relaxed folds regardless.
    assert!(rewrite(&first, RELAXED).report.groups > 0);
}

#[test]
fn preserve_holds_back_names_defined_differently_elsewhere() {
    let (a, b) = pair();
    let two = rewrite(&inputs(&[(b"a.obj", &a), (b"b.obj", &b)]), PRESERVE)
        .report
        .groups;
    // A third member defining one of the folded names differently: a copy
    // of a.obj with one COMDAT's first code byte changed.
    let mut o = object(&a).into_owned();
    let text = o
        .sections
        .iter()
        .position(|s| {
            s.name.as_bytes().starts_with(b".text")
                && s.characteristics & 0x1000 != 0
                && s.size() > 0
        })
        .unwrap();
    if let SectionData::Bytes(bytes) = &mut o.sections[text].data {
        bytes.to_mut()[0] ^= 0xFF;
    }
    let c = o.write().unwrap();
    // Wherever the different copy is, its name stays: which copy a link
    // keeps depends on which members it loads, and in what order.
    for order in [
        [(&b"a.obj"[..], &a), (b"b.obj", &b), (b"c.obj", &c)],
        [(&b"c.obj"[..], &c), (b"a.obj", &a), (b"b.obj", &b)],
    ] {
        let three = inputs(&order.map(|(n, d)| (n, d.as_slice())));
        let preserve = rewrite(&three, PRESERVE);
        assert!(
            preserve
                .report
                .not_folded
                .contains_key(&Unfoldable::OtherDefinitions)
        );
        assert_eq!(preserve.report.groups, two - 1);
        // Relaxed folds the identical copies anyway.
        assert_eq!(rewrite(&three, RELAXED).report.groups, two);
    }
}

#[test]
fn preserve_folds_copies_whose_environments_alone_differ() {
    let (a, b) = pair();
    let two = rewrite(&inputs(&[(b"a.obj", &a), (b"b.obj", &b)]), PRESERVE)
        .report
        .groups;
    // A third member with the same code but another /FAILIFMISMATCH: its
    // copies are another environment's, alone there, but the same code, so
    // the other two still fold.
    let mut o = object(&a).into_owned();
    o.push_section(Section::new(
        Name::new(b".drectve".to_vec()),
        0x0010_0A00,
        SectionData::Bytes(Cow::Owned(b" /FAILIFMISMATCH:\"x=1\"".to_vec())),
    ));
    let c = o.write().unwrap();
    let three = inputs(&[(b"c.obj", &c), (b"a.obj", &a), (b"b.obj", &b)]);
    assert_eq!(rewrite(&three, PRESERVE).report.groups, two);
}

#[test]
fn passed_through_members_are_unchanged() {
    let (a, b) = pair();
    let imports = fixture("imports.lib");
    let archive = turbonop_archive::Archive::parse(&imports).unwrap();
    let import = archive
        .files()
        .find(|m| turbonop_pe::import_object::ImportObject::is_import_object(m.data))
        .unwrap();
    let ltcg = fixture("ltcg.obj");
    let out = rewrite(
        &inputs(&[
            (b"i.obj", import.data),
            (b"a.obj", &a),
            (b"l.obj", &ltcg),
            (b"b.obj", &b),
        ]),
        PRESERVE,
    );
    assert_eq!(out.report.passed_through[&Unfoldable::Import], 1);
    assert_eq!(out.report.passed_through[&Unfoldable::Ltcg], 1);
    let n = out.report.synthesized as usize;
    assert_eq!(&*out.members[n].data, import.data);
    assert_eq!(&*out.members[n + 2].data, &ltcg[..]);
    assert!(out.report.groups > 0);
}

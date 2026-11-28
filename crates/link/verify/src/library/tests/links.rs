//! `Depth::Links` (turbolib's `verify=links`) against `Depth::Full`, on the
//! fixture's rewritings of the parent module: every library the full check
//! passes, links passes; every break that isn't of contents, links fails
//! in the same categories as full; and the breaks of contents that links
//! deliberately leaves out (a moved COMDAT, or what a member keeps, with
//! other bytes of the same size), full fails and links passes.

use super::*;

fn verify_at(original: &[u8], rewritten: &[u8], pull: Pull, depth: Depth) -> Report {
    super::super::verify(
        original,
        rewritten,
        &Options {
            pull,
            depth,
            ..Options::default()
        },
    )
    .unwrap()
}

fn failures(report: &Report) -> Vec<String> {
    report
        .findings()
        .filter(|(s, _, _)| *s == crate::report::Severity::Failure)
        .map(|(_, c, _)| c.to_string())
        .collect()
}

const CONTENTS_UNCHECKED: &str = "contents of COMDATs and of sections outside them (verify=links: their names, characteristics, sizes, and recorded checksums compared, not their bytes or relocations)";

/// Checks `rewritten` at both depths: each fails exactly `want` (and
/// counts what it checked the same), and links says what it left out.
fn both(original: &[u8], rewritten: &[u8], pull: Pull, want: &[&str]) {
    let full = verify_at(original, rewritten, pull, Depth::Full);
    let links = verify_at(original, rewritten, pull, Depth::Links);
    fails(&full, want);
    fails(&links, want);
    for what in [
        "original members",
        "synthesized members",
        "COMDATs moved out of original members",
        "COMDATs in synthesized members",
        "original index entries",
        "rewritten index entries",
        "side-table entries",
    ] {
        assert_eq!(
            full.checked_count(what),
            links.checked_count(what),
            "{what}: {}\n{}",
            full.text(),
            links.text()
        );
    }
    assert!(
        links
            .finding(crate::report::Severity::Unchecked, CONTENTS_UNCHECKED)
            .is_some(),
        "{}",
        links.text()
    );
    assert!(
        full.finding(crate::report::Severity::Unchecked, CONTENTS_UNCHECKED)
            .is_none(),
        "{}",
        full.text()
    );
}

#[test]
fn links_passes_what_full_passes() {
    let lib = fixture("verify.lib");
    both(&lib, &lib, Pull::Preserve, &[]);
    both(&lib, &archive(&members(), None), Pull::Preserve, &[]);
    for pull in [Pull::Relaxed, Pull::Preserve] {
        both(&lib, &archive(&folded(), None), pull, &[]);
    }
    both(
        &lib,
        &archive(&folded_preserving(), None),
        Pull::Preserve,
        &[],
    );
    // The engine's aliases, of the new default and of the old (a library
    // folded before), and the old renewed.
    for default in [alias_default(0x8664), LEGACY_ALIAS_DEFAULT] {
        let m = folded_with_aliases(default);
        both(
            &lib,
            &archive(&m, Some(engine_index(&m))),
            Pull::Preserve,
            &[],
        );
    }
    let old = folded_with_aliases(LEGACY_ALIAS_DEFAULT);
    let new: Vec<(Vec<u8>, Vec<u8>)> = old.iter().map(|(n, d)| (n.clone(), renewed(d))).collect();
    both(
        &lib,
        &archive(&new, Some(engine_index(&new))),
        Pull::Preserve,
        &[],
    );
    let mut m = folded();
    let s = m.pop().unwrap();
    m.insert(0, s);
    both(&lib, &archive(&m, None), Pull::Preserve, &[]);
}

#[test]
fn links_catches_a_missing_definition() {
    let lib = fixture("verify.lib");
    // The fold without its synthesized member: what moved is defined
    // nowhere.
    let mut m = folded();
    m.pop();
    both(
        &lib,
        &archive(&m, None),
        Pull::Relaxed,
        &[
            "moved COMDATs that no synthesized member holds (in their environment)",
            "moved symbols that no member defines",
            "symbols the original defines that no member defines",
            "undefined references added to original members (not moved symbols or anchors)",
        ],
    );
    // A strong definition moved.
    let mut m = members();
    let b = find(&m, b"b.obj");
    let compute: &[u8] = b"?compute@@YAHXZ";
    let s = synthesized(&m[b].1, &[compute], false, ANCHOR);
    m[b].1 = without(&m[b].1, &[compute], false, Some(ANCHOR));
    m.push((b"turbolib-0.obj".to_vec(), s));
    both(
        &lib,
        &archive(&m, None),
        Pull::Relaxed,
        &[
            "strong (NODUPLICATES) COMDATs moved",
            "definitions that left an original member without their COMDAT moving",
        ],
    );
}

#[test]
fn links_catches_a_wrong_index() {
    let lib = fixture("verify.lib");
    let m = members();
    let c = find(&m, b"c.obj");
    let b = find(&m, b"b.obj");
    let mut index: Vec<(Vec<u8>, usize)> = first_definers(&m)
        .into_iter()
        .filter(|(n, _)| n != b"?compute@@YAHXZ")
        .map(|(n, i)| {
            if n == SHARED_ADD && i == c {
                (n, b)
            } else {
                (n, i)
            }
        })
        .collect();
    index.push((b"?gl_value@@YAHH@Z".to_vec(), find(&m, b"gl.obj")));
    both(
        &lib,
        &archive(&m, Some(index)),
        Pull::Relaxed,
        &[
            "rewritten definitions missing from the index",
            "rewritten index entries naming a member other than the first definer",
        ],
    );
}

#[test]
fn links_catches_a_changed_load_order() {
    // The index sending shared_add from a definer with side effects (a.obj,
    // first in this original) to a synthesized member.
    let mut m = members();
    let a = find(&m, b"a.obj");
    let x = m.remove(a);
    let first = find(&m, b"c.obj");
    m.insert(first, x);
    let original = archive(&m, None);
    let mut f = m.clone();
    let b = find(&f, b"b.obj");
    let s = synthesized(&f[b].1, &[SHARED_ADD], false, ANCHOR);
    for name in [&b"a.obj"[..], b"b.obj", b"c.obj"] {
        let i = find(&f, name);
        f[i].1 = without(&f[i].1, &[SHARED_ADD], false, Some(ANCHOR));
    }
    f.insert(0, (b"turbolib-0.obj".to_vec(), s));
    let ours = archive(&f, None);
    both(&original, &ours, Pull::Relaxed, &[]);
    both(
        &original,
        &ours,
        Pull::Preserve,
        &["symbols the index sends to another member (--pull=preserve)"],
    );
    // Members out of order: what a link loads first changes.
    let lib = fixture("verify.lib");
    let mut m = members();
    let (a, b) = (find(&m, b"a.obj"), find(&m, b"b.obj"));
    m.swap(a, b);
    let full = verify_at(&lib, &archive(&m, None), Pull::Preserve, Depth::Full);
    let links = verify_at(&lib, &archive(&m, None), Pull::Preserve, Depth::Links);
    assert!(
        links.failed("original members out of order"),
        "{}",
        links.text()
    );
    assert_eq!(failures(&full), failures(&links), "{}", links.text());
    // A member anchoring no synthesized member: its moved COMDATs wouldn't
    // be loaded with it.
    let mut m = folded();
    let a = find(&m, b"a.obj");
    let orig = members();
    m[a].1 = without(&orig[a].1, &[SHARED_ADD, TWICE], false, None);
    both(
        &lib,
        &archive(&m, None),
        Pull::Relaxed,
        &["moved COMDATs whose synthesized member the original member doesn't anchor"],
    );
}

#[test]
fn links_catches_the_other_breaks_of_names_and_kinds() {
    let lib = fixture("verify.lib");
    // A copy in two synthesized members.
    let mut m = folded();
    let b = find(&m, b"b.obj");
    let orig = members();
    let again = synthesized(&orig[b].1, &[TWICE], false, b"__turbolib$anchor$1");
    m.push((b"turbolib-1.obj".to_vec(), again));
    both(
        &lib,
        &archive(&m, None),
        Pull::Relaxed,
        &["COMDATs held by more than one synthesized member"],
    );
    // Another selection: a unit's shape includes it.
    let mut m = folded();
    let s = m.len() - 1;
    let mut o = Object::parse(&m[s].1).unwrap().into_owned();
    for d in o
        .symbols
        .iter_mut()
        .filter_map(|s| s.section_definition_mut())
    {
        if d.selection == turbonop_coff::comdat_select::ANY {
            d.selection = turbonop_coff::comdat_select::LARGEST;
        }
    }
    m[s].1 = o.write().unwrap();
    both(
        &lib,
        &archive(&m, None),
        Pull::Relaxed,
        &[
            "moved COMDATs that no synthesized member holds (in their environment)",
            "symbols no longer defined with their kind",
            "synthesized COMDATs that match no original copy (in their environment)",
        ],
    );
    // A weak external of the wrong form.
    let mut m = folded_preserving();
    let a = find(&m, b"a.obj");
    let mut o = Object::parse(&m[a].1).unwrap().into_owned();
    for s in &mut o.symbols {
        if let Aux::WeakExternal(w) = &mut s.aux {
            w.search = weak_search::LIBRARY;
        }
    }
    m[a].1 = o.write().unwrap();
    both(
        &lib,
        &archive(&m, None),
        Pull::Preserve,
        &["weak externals that aren't NOLIBRARY with the renamed copy as default"],
    );
    // An alias with another default.
    let m = folded_with_aliases(b"__turbolib$other");
    both(
        &lib,
        &archive(&m, Some(engine_index(&m))),
        Pull::Preserve,
        &["alias weak externals whose default isn't __ImageBase (or the old __turbolib$alias)"],
    );
    // Aliases of both defaults in one library (link.exe's LNK1227).
    let mut m = folded_with_aliases(alias_default(0x8664));
    let c = find(&m, b"c.obj");
    let orig = members();
    let c0 = find(&orig, b"c.obj");
    m[c].1 = aliased(&orig[c0].1, &[SHARED_ADD], LEGACY_ALIAS_DEFAULT, ANCHOR);
    both(
        &lib,
        &archive(&m, Some(engine_index(&m))),
        Pull::Preserve,
        &["aliases of both defaults, __turbolib$alias and __ImageBase (LNK1227)"],
    );
    // Aliases no anchored synthesized member defines.
    let mut m = folded_with_aliases(alias_default(0x8664));
    let a = find(&m, b"a.obj");
    let a0 = find(&orig, b"a.obj");
    m[a].1 = aliased(
        &orig[a0].1,
        &[SHARED_ADD, TWICE],
        alias_default(0x8664),
        b"__turbolib$anchor$9",
    );
    let rewritten = archive(&m, Some(engine_index(&m)));
    let full = verify_at(&lib, &rewritten, Pull::Preserve, Depth::Full);
    let links = verify_at(&lib, &rewritten, Pull::Preserve, Depth::Links);
    assert!(
        links.failed("aliases of moved symbols that no anchored synthesized member defines"),
        "{}",
        links.text()
    );
    assert_eq!(failures(&full), failures(&links), "{}", links.text());
    // A lost side-table entry.
    let mut m = members();
    let b = find(&m, b"b.obj");
    let vftable: &[u8] = b"??_7Square@@6B@";
    let s = synthesized(&m[b].1, &[vftable], false, ANCHOR);
    m[b].1 = without(&m[b].1, &[vftable], false, Some(ANCHOR));
    m.push((b"turbolib-0.obj".to_vec(), s));
    both(&lib, &archive(&m, None), Pull::Relaxed, &[]);
    let s = m.len() - 1;
    let mut o = Object::parse(&m[s].1).unwrap().into_owned();
    o.remove_sections(|_, x| x.name == b".gfids$y").unwrap();
    m[s].1 = o.write().unwrap();
    let r = verify_at(&lib, &archive(&m, None), Pull::Relaxed, Depth::Links);
    assert!(
        r.failed(
            "side-table entries neither in their member nor in a synthesized member it anchors"
        ),
        "{}",
        r.text()
    );
    // An original member's directives, references, and a section's
    // characteristics.
    let mut m = members();
    let a = find(&m, b"a.obj");
    let mut o = Object::parse(&m[a].1).unwrap().into_owned();
    o.push_symbol(Symbol::new(
        &b"surprise"[..],
        0,
        SectionNumber::Undefined,
        0,
        storage_class::EXTERNAL,
    ));
    let drectve = o
        .sections
        .iter_mut()
        .find(|s| s.name == b".drectve")
        .unwrap();
    drectve.data = turbonop_coff::SectionData::Bytes(Cow::Owned(b"/DEFAULTLIB:other ".to_vec()));
    let data = o
        .sections
        .iter_mut()
        .find(|s| {
            (s.name == b".bss" || s.name == b".data")
                && s.characteristics & turbonop_pe::consts::section_characteristics::LNK_COMDAT == 0
        })
        .unwrap();
    data.characteristics ^= 0x0040_0000;
    m[a].1 = o.write().unwrap();
    both(
        &lib,
        &archive(&m, None),
        Pull::Relaxed,
        &[
            "original members whose directives changed",
            "original members whose sections outside COMDATs changed",
            "undefined references added to original members (not moved symbols or anchors)",
        ],
    );
}

/// `section`'s first byte (of the first section `pick` accepts) flipped,
/// its size and checksum field left as they were.
fn flip_first_byte(data: &[u8], pick: impl Fn(&turbonop_coff::Section<'_>) -> bool) -> Vec<u8> {
    let mut o = Object::parse(data).unwrap().into_owned();
    let s = o.sections.iter_mut().find(|s| pick(s)).unwrap();
    let mut bytes = s.bytes().to_vec();
    bytes[0] ^= 0xFF;
    s.data = turbonop_coff::SectionData::Bytes(Cow::Owned(bytes));
    o.write().unwrap()
}

#[test]
fn full_catches_other_bytes_that_links_leaves_out() {
    let lib = fixture("verify.lib");
    let comdat = turbonop_pe::consts::section_characteristics::LNK_COMDAT;
    // A moved COMDAT whose synthesized copy has other bytes, of the same
    // size and with the same recorded checksum.
    let mut m = folded();
    let s = m.len() - 1;
    m[s].1 = flip_first_byte(&m[s].1, |x| x.name.as_bytes().starts_with(b".text"));
    let rewritten = archive(&m, None);
    let full = verify_at(&lib, &rewritten, Pull::Relaxed, Depth::Full);
    fails(
        &full,
        &[
            "moved COMDATs that no synthesized member holds (in their environment)",
            "synthesized COMDATs that match no original copy (in their environment)",
        ],
    );
    let links = verify_at(&lib, &rewritten, Pull::Relaxed, Depth::Links);
    fails(&links, &[]);
    // A COMDAT an original member keeps, changed in place.
    let mut m = members();
    let c = find(&m, b"c.obj");
    m[c].1 = flip_first_byte(&m[c].1, |x| {
        x.name.as_bytes().starts_with(b".text") && x.characteristics & comdat != 0
    });
    let rewritten = archive(&m, None);
    let full = verify_at(&lib, &rewritten, Pull::Preserve, Depth::Full);
    assert!(
        full.failed("COMDATs of original members changed or added"),
        "{}",
        full.text()
    );
    fails(
        &verify_at(&lib, &rewritten, Pull::Preserve, Depth::Links),
        &[],
    );
    // A section outside COMDATs, changed in place.
    let mut m = members();
    let a = find(&m, b"a.obj");
    m[a].1 = flip_first_byte(&m[a].1, |x| {
        x.characteristics & comdat == 0
            && !x.bytes().is_empty()
            && member::INDEX_TABLES
                .iter()
                .chain(&member::RELOCATION_TABLES)
                .all(|t| x.name.as_bytes() != *t)
            && ![&b".drectve"[..], b".chks64"].contains(&x.name.as_bytes())
            && !x.name.as_bytes().starts_with(b".debug$")
    });
    let rewritten = archive(&m, None);
    fails(
        &verify_at(&lib, &rewritten, Pull::Preserve, Depth::Full),
        &["original members whose sections outside COMDATs changed"],
    );
    fails(
        &verify_at(&lib, &rewritten, Pull::Preserve, Depth::Links),
        &[],
    );
    // The same change of size is caught by both (a section's shape).
    let mut m = folded();
    let s = m.len() - 1;
    let mut o = Object::parse(&m[s].1).unwrap().into_owned();
    let text = o
        .sections
        .iter_mut()
        .find(|s| s.name.as_bytes().starts_with(b".text"))
        .unwrap();
    let mut bytes = text.bytes().to_vec();
    bytes.push(0xCC);
    text.data = turbonop_coff::SectionData::Bytes(Cow::Owned(bytes));
    m[s].1 = o.write().unwrap();
    both(
        &lib,
        &archive(&m, None),
        Pull::Relaxed,
        &[
            "moved COMDATs that no synthesized member holds (in their environment)",
            "synthesized COMDATs that match no original copy (in their environment)",
        ],
    );
    // And a changed recorded checksum.
    let mut m = folded();
    let s = m.len() - 1;
    let mut o = Object::parse(&m[s].1).unwrap().into_owned();
    let d = o
        .symbols
        .iter_mut()
        .filter_map(|s| s.section_definition_mut())
        .find(|d| d.selection == turbonop_coff::comdat_select::ANY)
        .unwrap();
    d.checksum ^= 1;
    m[s].1 = o.write().unwrap();
    let r = verify_at(&lib, &archive(&m, None), Pull::Relaxed, Depth::Links);
    fails(
        &r,
        &[
            "moved COMDATs that no synthesized member holds (in their environment)",
            "synthesized COMDATs that match no original copy (in their environment)",
        ],
    );
}

/// Under `--locals=copy` or `merge`, whose units are keyed by content, the
/// check is full whatever the depth.
#[test]
fn links_with_locals_is_full() {
    let o = Options {
        locals: true,
        depth: Depth::Links,
        ..Options::default()
    };
    assert!(o.contents());
    assert!(
        !Options {
            depth: Depth::Links,
            ..Options::default()
        }
        .contents()
    );
    let lib = fixture("verify.lib");
    let mut m = folded();
    let s = m.len() - 1;
    m[s].1 = flip_first_byte(&m[s].1, |x| x.name.as_bytes().starts_with(b".text"));
    let r = super::super::verify(&lib, &archive(&m, None), &o).unwrap();
    assert!(
        r.failed("synthesized COMDATs that match no original copy (in their environment)"),
        "{}",
        r.text()
    );
    assert!(
        r.finding(crate::report::Severity::Unchecked, CONTENTS_UNCHECKED)
            .is_none()
    );
}

//! `verify` on libraries made from `testdata/turbolib/verify.lib`'s
//! members: the library as `lib.exe` wrote it, rewritten as turbolib's
//! folds would (correctly, and broken in each way the checks look for),
//! and archived again by a small Microsoft-dialect writer here.

use super::*;
use std::borrow::Cow;
use turbonop_coff::{Aux, Object, SectionNumber, Symbol, SymbolId, storage_class, weak_search};

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/turbolib")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The fixture library's members, as (name, data).
fn members() -> Vec<(Vec<u8>, Vec<u8>)> {
    let lib = fixture("verify.lib");
    let a = Archive::parse(&lib).unwrap();
    a.files()
        .map(|m| (m.name.to_vec(), m.data.to_vec()))
        .collect()
}

/// Each member's indexed names, sent to the first definer.
fn first_definers(members: &[(Vec<u8>, Vec<u8>)]) -> Vec<(Vec<u8>, usize)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (i, (_, data)) in members.iter().enumerate() {
        let names: Vec<Vec<u8>> = match member::read(data) {
            Body::Coff(c) => c.definitions.iter().map(|d| d.name.to_vec()).collect(),
            Body::Import(n) => n.iter().map(|n| n.to_vec()).collect(),
            _ => Vec::new(),
        };
        for n in names {
            if seen.insert(n.clone()) {
                out.push((n, i));
            }
        }
    }
    out
}

/// An archive of `members` in the Microsoft dialect: both linker members
/// (from `index`, as (name, member number), or else
/// [`first_definers`]), the long names, and the members. A `/GL`
/// member's names come from the fixture's own index.
fn archive(members: &[(Vec<u8>, Vec<u8>)], index: Option<Vec<(Vec<u8>, usize)>>) -> Vec<u8> {
    let index = index.unwrap_or_else(|| {
        let mut index = first_definers(members);
        if let Some(gl) = members.iter().position(|(n, _)| n == b"gl.obj") {
            index.push((b"?gl_value@@YAHH@Z".to_vec(), gl));
        }
        index
    });
    let header = |name: &[u8], size: usize| -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(format!("{:<16}", String::from_utf8_lossy(name)).as_bytes());
        h.extend_from_slice(
            format!("{:<12}{:<6}{:<6}{:<8}{:<10}`\n", -1, "", "", 0, size).as_bytes(),
        );
        assert_eq!(h.len(), 60);
        h
    };
    let pad = |n: usize| n + (n & 1);
    let names_len: usize = index.iter().map(|(n, _)| n.len() + 1).sum();
    let first_size = 4 + 4 * index.len() + names_len;
    let second_size = 4 + 4 * members.len() + 4 + 2 * index.len() + names_len;
    let mut long = Vec::new();
    let mut fields = Vec::new();
    for (name, _) in members {
        if name.len() > 15 {
            fields.push(format!("/{}", long.len()).into_bytes());
            long.extend_from_slice(name);
            long.push(0);
        } else {
            fields.push([&name[..], b"/"].concat());
        }
    }
    let mut at = 8 + 60 + pad(first_size) + 60 + pad(second_size);
    if !long.is_empty() {
        at += 60 + pad(long.len());
    }
    let mut offsets = Vec::new();
    for (_, data) in members {
        offsets.push(at as u32);
        at += 60 + pad(data.len());
    }
    let even = |out: &mut Vec<u8>| {
        if out.len() & 1 == 1 {
            out.push(b'\n');
        }
    };
    let mut out = b"!<arch>\n".to_vec();
    out.extend(header(b"/", first_size));
    out.extend((index.len() as u32).to_be_bytes());
    for (_, m) in &index {
        out.extend(offsets[*m].to_be_bytes());
    }
    for (n, _) in &index {
        out.extend_from_slice(n);
        out.push(0);
    }
    even(&mut out);
    let mut sorted: Vec<&(Vec<u8>, usize)> = index.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    out.extend(header(b"/", second_size));
    out.extend((members.len() as u32).to_le_bytes());
    for o in &offsets {
        out.extend(o.to_le_bytes());
    }
    out.extend((index.len() as u32).to_le_bytes());
    for (_, m) in &sorted {
        out.extend((*m as u16 + 1).to_le_bytes());
    }
    for (n, _) in &sorted {
        out.extend_from_slice(n);
        out.push(0);
    }
    even(&mut out);
    if !long.is_empty() {
        out.extend(header(b"//", long.len()));
        out.extend_from_slice(&long);
        even(&mut out);
    }
    for (k, (field, (_, data))) in fields.iter().zip(members).enumerate() {
        assert_eq!(offsets[k] as usize, out.len());
        out.extend(header(field, data.len()));
        out.extend_from_slice(data);
        even(&mut out);
    }
    out
}

fn verify_with(original: &[u8], rewritten: &[u8], pull: Pull) -> Report {
    super::verify(original, rewritten, &Options { pull }).unwrap()
}

/// Asserts that the report fails exactly the categories `want`.
fn fails(report: &Report, want: &[&str]) {
    let got: Vec<&str> = report
        .findings()
        .filter(|(s, _, _)| *s == crate::report::Severity::Failure)
        .map(|(_, c, _)| c)
        .collect();
    let mut want: Vec<&str> = want.to_vec();
    want.sort_unstable();
    assert_eq!(got, want, "{}", report.text());
}

const SHARED_ADD: &[u8] = b"?shared_add@@YAHHH@Z";
const TWICE: &[u8] = b"??$twice@H@@YAHH@Z";
const ANCHOR: &[u8] = b"__turbolib$anchor$0";

/// The sections of `leader`'s unit in `o`: its section, then its
/// associative sections (transitively), in section order.
fn unit_sections(o: &Object<'_>, leader: &[u8]) -> Vec<usize> {
    let s = o
        .symbols
        .iter()
        .find(|s| s.name == leader && s.section_definition().is_none())
        .and_then(|s| s.section.id())
        .unwrap()
        .index();
    let mut out = vec![s];
    loop {
        let before = out.len();
        for sym in &o.symbols {
            if let (Some(id), Some(d)) = (sym.section.id(), sym.section_definition())
                && d.selection == turbonop_coff::comdat_select::ASSOCIATIVE
                && d.number.is_some_and(|n| out.contains(&n.index()))
                && !out.contains(&id.index())
            {
                out.push(id.index());
            }
        }
        if out.len() == before {
            break;
        }
    }
    out.sort_unstable();
    out
}

/// Rewrites `o`'s index tables (`.gfids$y`, ...), whose entries are
/// symbol table indices, for the edits since `before` (each symbol's
/// index then) that `renumbering` (if any symbols went) describes.
fn renumber_tables(
    o: &mut Object<'_>,
    before: &[u32],
    renumbering: Option<&turbonop_coff::Renumbering>,
) {
    let after = o.symbol_table_indices();
    for s in &mut o.sections {
        if !member::INDEX_TABLES.contains(&s.name.as_bytes()) {
            continue;
        }
        let mut bytes = s.bytes().to_vec();
        for e in bytes.as_chunks_mut::<4>().0 {
            let old = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
            let id = SymbolId(before.binary_search(&old).unwrap() as u32);
            let id = renumbering.map_or(Some(id), |r| r.symbol(id)).unwrap();
            e.copy_from_slice(&after[id.index()].to_le_bytes());
        }
        s.data = turbonop_coff::SectionData::Bytes(Cow::Owned(bytes));
    }
}

/// `data` without the units of `leaders`, each leader left an undefined
/// external (or, with `weak`, a NOLIBRARY weak external whose default is
/// its renamed copy), and with an undefined reference to `anchor`.
fn without(data: &[u8], leaders: &[&[u8]], weak: bool, anchor: Option<&[u8]>) -> Vec<u8> {
    let mut o = Object::parse(data).unwrap();
    let before = o.symbol_table_indices();
    let mut gone = Vec::new();
    for l in leaders {
        gone.extend(unit_sections(&o, l));
    }
    for l in leaders {
        let i = o.symbols.iter().position(|s| s.name == *l).unwrap();
        if weak {
            let alt = o.push_symbol(Symbol::new(
                [RENAMED, l].concat(),
                0,
                SectionNumber::Undefined,
                0,
                storage_class::EXTERNAL,
            ));
            let s = &mut o.symbols[i];
            s.section = SectionNumber::Undefined;
            s.value = 0;
            s.storage_class = storage_class::WEAK_EXTERNAL;
            s.aux = Aux::WeakExternal(turbonop_coff::WeakExternal {
                tag: alt,
                search: weak_search::NOLIBRARY,
            });
        } else {
            let s = &mut o.symbols[i];
            s.section = SectionNumber::Undefined;
            s.value = 0;
            s.aux = Aux::None;
        }
    }
    let renumbering = o
        .remove_sections(|id, _| gone.contains(&id.index()))
        .unwrap();
    renumber_tables(&mut o, &before, Some(&renumbering));
    if let Some(a) = anchor {
        o.push_symbol(Symbol::new(
            a.to_vec(),
            0,
            SectionNumber::Undefined,
            0,
            storage_class::EXTERNAL,
        ));
    }
    o.write().unwrap()
}

/// A synthesized member: the units of `leaders` copied out of `data`
/// (with `.drectve` and `@feat.00`), each leader renamed if `rename`, and
/// a defined anchor.
fn synthesized(data: &[u8], leaders: &[&[u8]], rename: bool, anchor: &[u8]) -> Vec<u8> {
    let mut o = Object::parse(data).unwrap();
    let before = o.symbol_table_indices();
    let mut keep = Vec::new();
    for l in leaders {
        keep.extend(unit_sections(&o, l));
    }
    for (i, s) in o.sections.iter().enumerate() {
        if s.name == b".drectve" {
            keep.push(i);
        }
    }
    // Symbols the kept sections refer to, defined in sections that go,
    // become undefined externals.
    let referenced: std::collections::HashSet<SymbolId> = keep
        .iter()
        .flat_map(|&s| o.sections[s].relocations.iter().map(|r| r.symbol))
        .collect();
    for id in &referenced {
        let s = &mut o.symbols[id.index()];
        if let Some(sec) = s.section.id()
            && !keep.contains(&sec.index())
        {
            assert!(s.is_external(), "a unit reaching a static");
            s.section = SectionNumber::Undefined;
            s.value = 0;
            s.aux = Aux::None;
        }
    }
    let renumbering = o
        .retain(
            |id, _| keep.contains(&id.index()),
            |id, s| match s.section {
                SectionNumber::Section(sec) => keep.contains(&sec.index()),
                SectionNumber::Absolute => s.name == b"@feat.00",
                _ => referenced.contains(&id),
            },
        )
        .unwrap();
    renumber_tables(&mut o, &before, Some(&renumbering));
    if rename {
        for l in leaders {
            let s = o.symbols.iter_mut().find(|s| s.name == *l).unwrap();
            s.name.set([RENAMED, l].concat());
        }
    }
    o.push_symbol(Symbol::new(
        anchor.to_vec(),
        0,
        SectionNumber::Absolute,
        0,
        storage_class::EXTERNAL,
    ));
    o.write().unwrap()
}

fn find(members: &[(Vec<u8>, Vec<u8>)], name: &[u8]) -> usize {
    members.iter().position(|(n, _)| n == name).unwrap()
}

/// The fixture's members with `shared_add` and `twice<int>` folded out of
/// a.obj, b.obj, and c.obj into one synthesized member, every leader left
/// undefined (`--pull=relaxed`'s rewriting).
fn folded() -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut m = members();
    let c = find(&m, b"c.obj");
    let b = find(&m, b"b.obj");
    let s = synthesized(&m[b].1, &[SHARED_ADD, TWICE], false, ANCHOR);
    for name in [&b"a.obj"[..], b"b.obj"] {
        let i = find(&m, name);
        m[i].1 = without(&m[i].1, &[SHARED_ADD, TWICE], false, Some(ANCHOR));
    }
    m[c].1 = without(&m[c].1, &[SHARED_ADD], false, Some(ANCHOR));
    m.push((b"turbolib-0.obj".to_vec(), s));
    m
}

/// The same fold under `--pull=preserve`: c.obj, the definer the index
/// names for `shared_add`, keeps its copy, and so does b.obj, the
/// definer of `twice<int>`; the others get weak externals whose default
/// is the synthesized member's renamed copy.
fn folded_preserving() -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut m = members();
    let b = find(&m, b"b.obj");
    let s = synthesized(&m[b].1, &[SHARED_ADD, TWICE], true, ANCHOR);
    let a = find(&m, b"a.obj");
    m[a].1 = without(&m[a].1, &[SHARED_ADD, TWICE], true, Some(ANCHOR));
    m[b].1 = without(&m[b].1, &[SHARED_ADD], true, Some(ANCHOR));
    m.push((b"turbolib-0.obj".to_vec(), s));
    m
}

#[test]
fn the_fixture_is_equivalent_to_itself_and_to_our_archive_of_it() {
    let lib = fixture("verify.lib");
    let r = verify_with(&lib, &lib, Pull::Preserve);
    fails(&r, &[]);
    assert_eq!(r.checked_count("original members"), 9);
    assert_eq!(r.checked_count("members passed through"), 3);
    assert_eq!(r.checked_count("original index entries"), 26);
    // Our writer's archive of the same members.
    let ours = archive(&members(), None);
    fails(&verify_with(&lib, &ours, Pull::Preserve), &[]);
}

#[test]
fn a_correct_relaxed_fold_passes_relaxed_and_fails_preserve() {
    let lib = fixture("verify.lib");
    let ours = archive(&folded(), None);
    let r = verify_with(&lib, &ours, Pull::Relaxed);
    fails(&r, &[]);
    assert_eq!(r.checked_count("synthesized members"), 1);
    assert_eq!(r.checked_count("COMDATs moved out of original members"), 5);
    assert!(
        r.finding(
            crate::report::Severity::Expected,
            "symbols the index sends to another member (--pull=relaxed allows it)"
        )
        .is_some()
    );
    // Under preserve, the index now sends both to the synthesized member.
    let r = verify_with(&lib, &ours, Pull::Preserve);
    let category = "symbols the index sends to another member (--pull=preserve)";
    fails(&r, &[category]);
    let f = r
        .finding(crate::report::Severity::Failure, category)
        .unwrap();
    assert_eq!(f.count, 2);
}

#[test]
fn a_correct_preserving_fold_passes() {
    let lib = fixture("verify.lib");
    let ours = archive(&folded_preserving(), None);
    let r = verify_with(&lib, &ours, Pull::Preserve);
    fails(&r, &[]);
    assert_eq!(r.checked_count("COMDATs moved out of original members"), 3);
    assert_eq!(r.checked_count("COMDATs in synthesized members"), 2);
}

#[test]
fn a_changed_copy_is_caught() {
    let lib = fixture("verify.lib");
    let mut m = folded();
    let s = m.len() - 1;
    let mut o = Object::parse(&m[s].1).unwrap().into_owned();
    let text = o
        .sections
        .iter_mut()
        .find(|s| s.name.as_bytes().starts_with(b".text"))
        .unwrap();
    let mut bytes = text.bytes().to_vec();
    bytes[0] ^= 0xFF;
    text.data = turbonop_coff::SectionData::Bytes(Cow::Owned(bytes));
    m[s].1 = o.write().unwrap();
    let r = verify_with(&lib, &archive(&m, None), Pull::Relaxed);
    fails(
        &r,
        &[
            "moved COMDATs that no synthesized member holds (in their environment)",
            "synthesized COMDATs that match no original copy (in their environment)",
        ],
    );
}

#[test]
fn a_missing_anchor_reference_is_caught() {
    let lib = fixture("verify.lib");
    let mut m = folded();
    let a = find(&m, b"a.obj");
    let orig = members();
    m[a].1 = without(&orig[a].1, &[SHARED_ADD, TWICE], false, None);
    let r = verify_with(&lib, &archive(&m, None), Pull::Relaxed);
    fails(
        &r,
        &["moved COMDATs whose synthesized member the original member doesn't anchor"],
    );
}

#[test]
fn moving_a_strong_definition_is_caught() {
    let lib = fixture("verify.lib");
    let mut m = members();
    let b = find(&m, b"b.obj");
    let compute: &[u8] = b"?compute@@YAHXZ";
    let s = synthesized(&m[b].1, &[compute], false, ANCHOR);
    m[b].1 = without(&m[b].1, &[compute], false, Some(ANCHOR));
    m.push((b"turbolib-0.obj".to_vec(), s));
    let r = verify_with(&lib, &archive(&m, None), Pull::Relaxed);
    fails(
        &r,
        &[
            "strong (NODUPLICATES) COMDATs moved",
            "definitions that left an original member without their COMDAT moving",
        ],
    );
}

#[test]
fn a_copy_in_two_synthesized_members_is_caught() {
    let lib = fixture("verify.lib");
    let mut m = folded();
    let b = find(&m, b"b.obj");
    let orig = members();
    let again = synthesized(&orig[b].1, &[TWICE], false, b"__turbolib$anchor$1");
    m.push((b"turbolib-1.obj".to_vec(), again));
    let r = verify_with(&lib, &archive(&m, None), Pull::Relaxed);
    fails(&r, &["COMDATs held by more than one synthesized member"]);
}

#[test]
fn another_selection_is_caught() {
    let lib = fixture("verify.lib");
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
    let r = verify_with(&lib, &archive(&m, None), Pull::Relaxed);
    fails(
        &r,
        &[
            "moved COMDATs that no synthesized member holds (in their environment)",
            "symbols no longer defined with their kind",
            "synthesized COMDATs that match no original copy (in their environment)",
        ],
    );
}

#[test]
fn a_changed_import_object_is_caught() {
    let lib = fixture("verify.lib");
    let mut m = members();
    let i = m
        .iter()
        .position(|(_, d)| matches!(member::read(d), Body::Import(_)))
        .unwrap();
    let last = m[i].1.len() - 2;
    m[i].1[last] ^= 0x20;
    let r = verify_with(&lib, &archive(&m, None), Pull::Preserve);
    assert!(r.failed("passed-through members changed"), "{}", r.text());
}

#[test]
fn a_wrong_or_incomplete_index_is_caught() {
    let lib = fixture("verify.lib");
    let m = members();
    let c = find(&m, b"c.obj");
    let b = find(&m, b"b.obj");
    // shared_add sent to b.obj rather than c.obj (its first definer), and
    // compute left out.
    let mut index: Vec<(Vec<u8>, usize)> = first_definers(&m)
        .into_iter()
        .filter(|(n, _)| n != b"?compute@@YAHXZ")
        .map(|(n, i)| {
            if n == SHARED_ADD {
                assert_eq!(i, c);
                (n, b)
            } else {
                (n, i)
            }
        })
        .collect();
    index.push((b"?gl_value@@YAHH@Z".to_vec(), find(&m, b"gl.obj")));
    let r = verify_with(&lib, &archive(&m, Some(index)), Pull::Relaxed);
    fails(
        &r,
        &[
            "rewritten definitions missing from the index",
            "rewritten index entries naming a member other than the first definer",
        ],
    );
}

#[test]
fn members_out_of_order_missing_or_added_are_caught() {
    let lib = fixture("verify.lib");
    let mut m = members();
    let (a, b) = (find(&m, b"a.obj"), find(&m, b"b.obj"));
    m.swap(a, b);
    let r = verify_with(&lib, &archive(&m, None), Pull::Relaxed);
    assert!(r.failed("original members out of order"), "{}", r.text());

    let mut m = members();
    let gl = find(&m, b"gl.obj");
    let extra = m[gl].1.clone();
    m.push((b"extra.obj".to_vec(), extra));
    m.remove(gl);
    let r = verify_with(&lib, &archive(&m, None), Pull::Relaxed);
    assert!(r.failed("original members missing"), "{}", r.text());
    assert!(
        r.failed("added members that aren't synthesized (no anchor)"),
        "{}",
        r.text()
    );
}

#[test]
fn a_lost_side_table_entry_is_caught() {
    let lib = fixture("verify.lib");
    // The vftables' units carry .gfids$y entries; moving Square's keeps
    // them, and dropping the synthesized copy's table loses them.
    let mut m = members();
    let b = find(&m, b"b.obj");
    let vftable: &[u8] = b"??_7Square@@6B@";
    let s = synthesized(&m[b].1, &[vftable], false, ANCHOR);
    m[b].1 = without(&m[b].1, &[vftable], false, Some(ANCHOR));
    m.push((b"turbolib-0.obj".to_vec(), s));
    let r = verify_with(&lib, &archive(&m, None), Pull::Relaxed);
    fails(&r, &[]);
    let s = m.len() - 1;
    let mut o = Object::parse(&m[s].1).unwrap().into_owned();
    o.remove_sections(|_, x| x.name == b".gfids$y").unwrap();
    m[s].1 = o.write().unwrap();
    let r = verify_with(&lib, &archive(&m, None), Pull::Relaxed);
    assert!(
        r.failed(
            "side-table entries neither in their member nor in a synthesized member it anchors"
        ),
        "{}",
        r.text()
    );
}

#[test]
fn changes_to_an_original_members_other_parts_are_caught() {
    let lib = fixture("verify.lib");
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
    let r = verify_with(&lib, &archive(&m, None), Pull::Relaxed);
    fails(
        &r,
        &[
            "original members whose directives changed",
            "original members whose sections outside COMDATs changed",
            "undefined references added to original members (not moved symbols or anchors)",
        ],
    );
}

#[test]
fn a_weak_external_of_the_wrong_form_is_caught() {
    let lib = fixture("verify.lib");
    let mut m = folded_preserving();
    let a = find(&m, b"a.obj");
    let mut o = Object::parse(&m[a].1).unwrap().into_owned();
    for s in &mut o.symbols {
        if let Aux::WeakExternal(w) = &mut s.aux {
            w.search = weak_search::LIBRARY;
        }
    }
    m[a].1 = o.write().unwrap();
    let r = verify_with(&lib, &archive(&m, None), Pull::Preserve);
    fails(
        &r,
        &["weak externals that aren't NOLIBRARY with the renamed copy as default"],
    );
}

#[test]
fn not_archives_are_errors() {
    assert!(super::verify(b"nope", b"nope", &Options::default()).is_err());
}

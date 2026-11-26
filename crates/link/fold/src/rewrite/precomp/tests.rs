//! Tests of shared types (`--precomp=library`), on the fixtures that
//! `tools/turbolib/debug/build.cmd` builds (`testdata/coff/fold`).
//!
//! The main check reads every `.debug$S` of every member twice, as the
//! member was and as it is written, in the canonical form of the debug
//! tests (every index as the structural hash of the record it names,
//! through whatever PCH the member names): the two must be the same.

use super::super::debug::tests::{
    RELAXED, canonical, fixture, inputs, object, pch_of, pch_streams, stream, tables,
};
use super::super::{Options, Output, OutputMember, Precomp, rewrite};
use super::*;
use turbonop_codeview::symbols::kind as sym;
use turbonop_coff::File;
use turbonop_hash::Set as HashSet;

const LIBRARY: Options = Options {
    fold: false,
    precomp: Precomp::Library { min: 2 },
    ..RELAXED
};

const FOLD_LIBRARY: Options = Options {
    fold: true,
    ..LIBRARY
};

fn run<'a>(names: &'a [&str], datas: &'a [Vec<u8>], options: Options) -> Output<'a> {
    rewrite(&inputs(names, datas), options)
}

fn datas(names: &[&str]) -> Vec<Vec<u8>> {
    names.iter().map(|n| fixture(n)).collect()
}

/// The output's precompiled-types members.
fn pchs<'o>(out: &'o Output<'_>) -> Vec<&'o [u8]> {
    out.members
        .iter()
        .filter(|m| m.synthesized && m.name.starts_with(b"turbolib-types-"))
        .map(|m| &m.data[..])
        .collect()
}

/// The output's members that were inputs, in order.
fn originals<'o, 'a>(out: &'o Output<'a>) -> &'o [OutputMember<'a>] {
    let first = out.members.iter().take_while(|m| m.synthesized).count();
    &out.members[first..]
}

fn section<'o>(o: &'o Object<'_>, name: &[u8]) -> Option<&'o Section<'o>> {
    o.sections.iter().find(|s| s.name.as_bytes() == name)
}

fn first_record(o: &Object<'_>) -> Option<(u16, Vec<u8>)> {
    let t = section(o, b".debug$T")?;
    let (_, r) = remap::type_records(&t.bytes()[4..]).next()?.ok()?;
    Some((r.kind, r.data.to_vec()))
}

/// Each record of a type section (signature first): its kind and bytes.
fn kinds(data: &[u8]) -> Vec<(u16, Vec<u8>)> {
    remap::type_records(&data[4..])
        .map(|r| {
            let (at, r) = r.unwrap();
            (r.kind, data[4 + at..4 + at + 4 + r.data.len()].to_vec())
        })
        .collect()
}

/// A precompiled-types member as phase 0's spike made one, and its
/// records: each names only earlier ones, none is a function id or build
/// information, no two are the same, and `LF_ENDPRECOMP` ends them with
/// the signature that `S_OBJNAME` has too. Its symbol, signature, and
/// number of records.
fn check_pch(data: &[u8], name: &[u8]) -> (Vec<u8>, u32, usize) {
    let o = object(data);
    let p = section(&o, b".debug$P").expect(".debug$P");
    let records = kinds(p.bytes());
    let (last, rest) = records.split_last().unwrap();
    assert_eq!(last.0, leaf::LF_ENDPRECOMP);
    let signature = le32(&last.1, 4);
    let mut seen = HashSet::default();
    let mut refs = Vec::new();
    for (i, (kind, bytes)) in rest.iter().enumerate() {
        assert!(shareable(*kind), "a {kind:#06x} in the PCH");
        refs.clear();
        type_refs::refs(*kind, &bytes[4..], &mut refs).unwrap();
        for r in &refs {
            let v = le32(&bytes[4..], r.offset as usize);
            assert!(v < FIRST + i as u32, "record {i} names {v:#x}");
        }
        assert!(seen.insert(bytes.clone()), "a duplicate");
    }
    let s = section(&o, b".debug$S").unwrap();
    let x = &c13::section_subsections(s.bytes()).unwrap()[0];
    let r = &c13::symbol_records(x.data).unwrap()[0];
    assert_eq!(r.kind, sym::S_OBJNAME);
    assert_eq!(le32(r.data, 0), signature);
    let mut want = b"turbolib:".to_vec();
    want.extend_from_slice(name);
    assert_eq!(&r.data[4..4 + want.len()], &want[..]);
    let symbol = o
        .symbols
        .iter()
        .find(|s| s.is_external())
        .expect("its symbol");
    assert!(symbol.name.as_bytes().starts_with(PCH_SYMBOL_PREFIX));
    assert_ne!(symbol.section, SectionNumber::Undefined);
    (symbol.name.as_bytes().to_vec(), signature, rest.len())
}

/// Shares `names`' types and checks every member: what it was and what
/// it is name the same records everywhere. The number of members that
/// share.
fn check_members(names: &[&str], datas: &[Vec<u8>], pch: Option<&str>, options: Options) -> usize {
    let out = run(names, datas, options);
    let made = pchs(&out);
    assert_eq!(made.len(), 1, "{:?}", out.report.types);
    let pch_name = out
        .members
        .iter()
        .find(|m| m.name.starts_with(b"turbolib-types-"))
        .unwrap()
        .name
        .to_vec();
    let (symbol, signature, count) = check_pch(made[0], &pch_name);
    let pch_data = pch.map(fixture);
    let olds = pch_streams(pch_data.as_deref(), &[]);
    let news = pch_streams(None, &made);
    let shared: HashSet<u128> = news[0].hashes.iter().copied().collect();
    let mut sharing = 0;
    for (k, m) in originals(&out).iter().enumerate() {
        assert_eq!(m.name.as_ref(), names[k].as_bytes());
        let before = object(&datas[k]);
        let after = object(&m.data);
        let shares = first_record(&after)
            .is_some_and(|(kind, body)| kind == leaf::LF_PRECOMP && le32(&body, 8) == signature);
        if !shares {
            // Too little shared to pay for naming the PCH: as it was.
            if !options.fold {
                assert_eq!(m.data.as_ref(), &datas[k][..], "{}", names[k]);
            }
            continue;
        }
        sharing += 1;
        let (_, body) = first_record(&after).unwrap();
        assert_eq!((le32(&body, 0), le32(&body, 4)), (FIRST, count as u32));
        let mut want_name = pch_name.clone();
        want_name.push(0);
        assert_eq!(&body[12..12 + want_name.len()], &want_name[..]);
        assert!(
            after
                .symbols
                .iter()
                .any(|s| s.section == SectionNumber::Undefined && s.name.as_bytes() == symbol),
            "it refers to the PCH's symbol"
        );
        assert!(section(&after, b".debug$H").is_none());
        // Every .debug$S, canonically, the same as one it had, and as long.
        let (sb, tb) = (stream(&before, pch_of(&before, &olds)), tables(&before));
        let (sa, ta) = (stream(&after, pch_of(&after, &news)), tables(&after));
        let debug_s = |o: &Object<'_>| -> Vec<usize> {
            (0..o.sections.len())
                .filter(|&i| o.sections[i].name.as_bytes() == b".debug$S")
                .collect()
        };
        let (db, da) = (debug_s(&before), debug_s(&after));
        if options.fold {
            assert!(da.len() <= db.len());
        } else {
            assert_eq!(da.len(), db.len());
        }
        for &i in &da {
            let sec = &after.sections[i];
            let got = canonical(&after, sec, &sa, pch_of(&after, &news), &ta);
            let found = db.iter().any(|&j| {
                let old = &before.sections[j];
                old.bytes().len() == sec.bytes().len()
                    && canonical(&before, old, &sb, pch_of(&before, &olds), &tb) == got
            });
            assert!(found, "{}: a .debug$S that reads differently", names[k]);
        }
        // Its own records: none a shared one, but what only its build
        // information reaches.
        let t = section(&after, b".debug$T").unwrap();
        let local = kinds(t.bytes());
        for (j, h) in sa.hashes.iter().enumerate() {
            let kind = local[j + 1].0;
            if shared.contains(h) {
                assert!(
                    matches!(kind, leaf::LF_STRING_ID | leaf::LF_SUBSTR_LIST),
                    "{}: a shared {kind:#06x} kept",
                    names[k]
                );
            }
        }
        // Function ids and build information are its own.
        let count_kind = |recs: &[(u16, Vec<u8>)], k: u16| recs.iter().filter(|r| r.0 == k).count();
        let old_t = section(&before, b".debug$T").unwrap();
        for kind in [leaf::LF_FUNC_ID, leaf::LF_MFUNC_ID, leaf::LF_BUILDINFO] {
            let was = count_kind(&kinds(old_t.bytes()), kind);
            let is = count_kind(&local, kind);
            assert_eq!(is, was, "{}: {kind:#06x}", names[k]);
        }
        // Smaller.
        assert!(t.bytes().len() < old_t.bytes().len());
    }
    sharing
}

#[test]
fn msvc_members_read_the_same_through_the_shared_types() {
    let names = [
        "msvc-a.obj",
        "msvc-b.obj",
        "msvc-c.obj",
        "msvc-d.obj",
        "msvc-pch.obj",
        "msvc-u1.obj",
        "msvc-u2.obj",
    ];
    let d = datas(&names);
    for options in [LIBRARY, FOLD_LIBRARY] {
        let n = check_members(&names, &d, Some("msvc-pch.obj"), options);
        // Every member but the PCH object and the /O2 ones, whose few
        // shared records don't pay for naming the PCH.
        assert_eq!(n, 4, "{options:?}");
    }
    let out = run(&names, &d, LIBRARY);
    // The PCH object stays as it was: other libraries' users may name it.
    assert_eq!(originals(&out)[4].data.as_ref(), &d[4][..]);
    assert_eq!(out.report.types.absorbed, 2);
    assert_eq!(out.report.types.sharing, 4);
    assert_eq!(out.report.types.nothing_shared, 2);
    assert!(out.report.types.types_after < out.report.types.types_before);
}

#[test]
fn clang_members_read_the_same_through_the_shared_types() {
    let names = ["clang-a.obj", "clang-b.obj", "clang-c.obj", "clang-d.obj"];
    let d = datas(&names);
    for options in [LIBRARY, FOLD_LIBRARY] {
        assert_eq!(check_members(&names, &d, None, options), 2);
    }
}

#[test]
fn users_of_a_pch_outside_the_library_keep_their_types() {
    let names = ["msvc-u1.obj", "msvc-u2.obj"];
    let d = datas(&names);
    let out = run(&names, &d, LIBRARY);
    assert!(pchs(&out).is_empty());
    for (m, data) in originals(&out).iter().zip(&d) {
        assert_eq!(m.data.as_ref(), &data[..]);
    }
    // With members that share, still.
    let names = ["msvc-u1.obj", "msvc-u2.obj", "msvc-a.obj", "msvc-b.obj"];
    let d = datas(&names);
    let out = run(&names, &d, LIBRARY);
    assert_eq!(pchs(&out).len(), 1);
    assert_eq!(originals(&out)[0].data.as_ref(), &d[0][..]);
    assert_eq!(originals(&out)[1].data.as_ref(), &d[1][..]);
    assert_eq!(out.report.types.sharing, 2);
}

#[test]
fn the_threshold_bounds_the_shared_set() {
    let names = ["msvc-a.obj", "msvc-b.obj", "msvc-c.obj", "msvc-d.obj"];
    let d = datas(&names);
    let shared = |min| {
        let out = run(
            &names,
            &d,
            Options {
                precomp: Precomp::Library { min },
                ..LIBRARY
            },
        );
        out.report.types.shared_records
    };
    let (two, three, four, five) = (shared(2), shared(3), shared(4), shared(5));
    assert!(
        two > three && three >= four && four > 0,
        "{two} {three} {four}"
    );
    assert_eq!(five, 0);
    // With min 1, one member's types are all shared but what can't be.
    let one = run(
        &names[..1],
        &d[..1],
        Options {
            precomp: Precomp::Library { min: 1 },
            ..LIBRARY
        },
    );
    assert_eq!(one.report.types.sharing, 1);
    let o = object(&originals(&one)[0].data);
    for (kind, _) in kinds(section(&o, b".debug$T").unwrap().bytes())
        .iter()
        .skip(1)
    {
        assert!(
            !shareable(*kind) || matches!(*kind, leaf::LF_STRING_ID | leaf::LF_SUBSTR_LIST),
            "{kind:#06x}"
        );
    }
}

#[test]
fn the_output_is_the_same_every_time() {
    let names = [
        "msvc-pch.obj",
        "msvc-u1.obj",
        "msvc-a.obj",
        "msvc-b.obj",
        "msvc-u2.obj",
    ];
    let d = datas(&names);
    let a = run(&names, &d, FOLD_LIBRARY);
    let b = run(&names, &d, FOLD_LIBRARY);
    assert_eq!(a.members.len(), b.members.len());
    for (x, y) in a.members.iter().zip(&b.members) {
        assert_eq!(x.name, y.name);
        assert_eq!(x.data, y.data);
        assert_eq!(x.holes, y.holes);
    }
}

#[test]
fn synthesized_members_use_the_shared_types() {
    let names = [
        "msvc-a.obj",
        "msvc-b.obj",
        "msvc-pch.obj",
        "msvc-u1.obj",
        "msvc-u2.obj",
    ];
    let d = datas(&names);
    let out = run(&names, &d, FOLD_LIBRARY);
    let made = pchs(&out);
    let news = pch_streams(None, &made);
    let shared: HashSet<u128> = news[0].hashes.iter().copied().collect();
    let signature = news[0].signature.unwrap();
    let mut checked = 0;
    for m in out.members.iter().filter(|m| m.synthesized) {
        let o = object(&m.data);
        if section(&o, b".debug$P").is_some() || section(&o, b".debug$T").is_none() {
            continue;
        }
        let (kind, body) = first_record(&o).unwrap();
        assert_eq!(kind, leaf::LF_PRECOMP);
        assert_eq!(le32(&body, 8), signature);
        assert!(
            o.symbols
                .iter()
                .any(|s| s.section == SectionNumber::Undefined
                    && s.name.as_bytes().starts_with(PCH_SYMBOL_PREFIX))
        );
        // Its own records: none is a shared one.
        let s = stream(&o, pch_of(&o, &news));
        assert!(s.hashes.iter().all(|h| !shared.contains(h)));
        checked += 1;
    }
    assert!(checked >= 1, "{checked}");
}

/// A COFF object of `.debug$T` (these records) and a `.debug$S` of
/// `S_OBJNAME` and one `S_UDT` naming `udt`.
fn types_object(records: &[u8], udt: u32) -> Vec<u8> {
    let mut o = Object::new(Format::Regular, 0x8664);
    let mut symbols = Vec::new();
    write::obj_name(0, b"x.obj")
        .write_symbol(&mut symbols, 4)
        .unwrap();
    let mut u = write::RecordBuilder::new(sym::S_UDT);
    u.u32(udt).name(b"t");
    u.write_symbol(&mut symbols, 4).unwrap();
    let mut s = CV_SIGNATURE_C13.to_le_bytes().to_vec();
    c13::push_subsection(&mut s, sub::DEBUG_S_SYMBOLS, &symbols);
    push(&mut o, b".debug$S", DEBUG_CHARACTERISTICS, s);
    let mut t = CV_SIGNATURE_C13.to_le_bytes().to_vec();
    t.extend_from_slice(records);
    push(&mut o, b".debug$T", DEBUG_CHARACTERISTICS, t);
    o.sync_section_definitions();
    o.write().unwrap()
}

#[test]
fn a_record_naming_one_that_cant_be_shared_isnt_shared() {
    // 0x1000 LF_FUNC_ID, 0x1001 LF_MODIFIER of it (no compiler writes
    // that, but nothing here relies on it), 0x1002 LF_POINTER to int.
    let mut r = Vec::new();
    let mut f = write::RecordBuilder::new(leaf::LF_FUNC_ID);
    f.u32(0).u32(0x74).name(b"f");
    f.write_type(&mut r).unwrap();
    let mut m = write::RecordBuilder::new(leaf::LF_MODIFIER);
    m.u32(0x1000).u16(1);
    m.write_type(&mut r).unwrap();
    let mut p = write::RecordBuilder::new(leaf::LF_POINTER);
    p.u32(0x74).u32(0x1000c);
    p.write_type(&mut r).unwrap();
    // 0x1003, enough to pay for naming the PCH.
    let mut n = write::RecordBuilder::new(leaf::LF_STRING_ID);
    n.u32(0).name(&[b'x'; 200]);
    n.write_type(&mut r).unwrap();
    let a = types_object(&r, 0x1001);
    let b = types_object(&r, 0x1002);
    let names = ["a.obj", "b.obj"];
    let d = vec![a, b];
    let out = run(&names, &d, LIBRARY);
    let made = pchs(&out);
    assert_eq!(made.len(), 1);
    let o = object(made[0]);
    let recs = kinds(section(&o, b".debug$P").unwrap().bytes());
    // The pointer and the string, and LF_ENDPRECOMP.
    let k: Vec<u16> = recs.iter().map(|r| r.0).collect();
    assert_eq!(
        k,
        [leaf::LF_POINTER, leaf::LF_STRING_ID, leaf::LF_ENDPRECOMP]
    );
    for (k, m) in originals(&out).iter().enumerate() {
        let after = object(&m.data);
        let t = kinds(section(&after, b".debug$T").unwrap().bytes());
        let kinds: Vec<u16> = t.iter().map(|r| r.0).collect();
        assert_eq!(
            kinds,
            [leaf::LF_PRECOMP, leaf::LF_FUNC_ID, leaf::LF_MODIFIER],
            "{k}"
        );
        // The modifier names its own function id, now 0x1002.
        assert_eq!(le32(&t[2].1, 4), 0x1002);
        // The S_UDT: a's the modifier (0x1003), b's the pointer (0x1000).
        let s = section(&after, b".debug$S").unwrap();
        let x = &c13::section_subsections(s.bytes()).unwrap()[0];
        let udt = &c13::symbol_records(x.data).unwrap()[1];
        assert_eq!(le32(udt.data, 0), [0x1003, 0x1000][k]);
    }
}

fn is_comdat(s: &Section<'_>) -> bool {
    s.characteristics & 0x1000 != 0
}

#[test]
fn a_member_whose_debug_s_cant_be_read_through_keeps_its_types() {
    // b's per-object .debug$S with a symbol of a kind with no known layout.
    let mut o = object(&fixture("msvc-b.obj")).into_owned();
    let i = o
        .sections
        .iter()
        .position(|s| s.name.as_bytes() == b".debug$S" && !is_comdat(s))
        .unwrap();
    let mut bytes = o.sections[i].bytes().to_vec();
    c13::push_subsection(&mut bytes, sub::DEBUG_S_SYMBOLS, &[2, 0, 0x99, 0x11]);
    o.sections[i].data = SectionData::Bytes(Cow::Owned(bytes));
    o.forget_layout();
    o.sync_section_definitions();
    let b = o.write().unwrap();
    let names = ["a.obj", "b.obj", "a2.obj"];
    let d = vec![fixture("msvc-a.obj"), b, fixture("msvc-a.obj")];
    let out = run(&names, &d, LIBRARY);
    assert_eq!(out.report.types.unreadable, 1);
    assert!(
        out.report
            .types
            .first_unreadable
            .as_deref()
            .is_some_and(|w| w.contains(".debug$S")),
        "{:?}",
        out.report.types
    );
    assert_eq!(originals(&out)[1].data.as_ref(), &d[1][..]);
    assert_eq!(out.report.types.sharing, 2);
}

#[test]
fn precomputed_type_hashes_go_with_shared_types() {
    // A .debug$H (as clang's -gcodeview-ghash writes) beside a's types.
    let mut o = object(&fixture("msvc-a.obj")).into_owned();
    let mut h = vec![0x0D, 0x3E, 0xA3, 0xF2, 0, 0, 2, 0];
    h.extend_from_slice(&[0xAB; 16]);
    push(&mut o, b".debug$H", DEBUG_CHARACTERISTICS, h);
    o.sync_section_definitions();
    let a = o.write().unwrap();
    let names = ["a.obj", "b.obj"];
    let d = vec![a, fixture("msvc-b.obj")];
    let out = run(&names, &d, LIBRARY);
    assert_eq!(out.report.types.sharing, 2);
    let after = &originals(&out)[0].data;
    assert!(matches!(File::parse(after), Ok(File::Object(_))));
    assert!(section(&object(after), b".debug$H").is_none());
}

#[test]
fn nothing_shared_changes_nothing() {
    let names = ["msvc-a.obj"];
    let d = datas(&names);
    let out = run(&names, &d, LIBRARY);
    assert!(pchs(&out).is_empty());
    assert_eq!(originals(&out)[0].data.as_ref(), &d[0][..]);
    assert_eq!(out.report.types.nothing_shared, 1);
}

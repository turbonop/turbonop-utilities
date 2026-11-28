//! Tests of synthesized members' CodeView, on the fixtures that
//! `tools/turbolib/debug/build.cmd` builds (`testdata/coff/fold`).
//!
//! The main check reads each folded unit's `.debug$S` twice, in the member
//! the copy came from and in the synthesized member, as a canonical form
//! that doesn't depend on numbering: every type or id index replaced by a
//! structural hash of the record it names (through a PCH's records where
//! it names those), every file checksum offset by the file's name and
//! checksum, every string offset by the string, and every relocation by
//! the record it is in, its place there, and its target's name. The two
//! must be the same.

use super::super::{Input, Options, Pull, gathered as rewrite};
use super::*;
use crate::hash::H128;
use turbonop_coff::File;

pub(crate) fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../testdata/coff/fold")
            .join(name),
    )
    .unwrap()
}

pub(crate) fn object(data: &[u8]) -> Object<'_> {
    match File::parse(data).unwrap() {
        File::Object(o) => o,
        File::Opaque { .. } => panic!("not a COFF object"),
    }
}

pub(crate) const RELAXED: Options = Options {
    pull: Pull::Relaxed,
    weak_externals: false,
    fold: crate::rewrite::Fold::Identical,
    locals: crate::rewrite::Locals::Keep,
    odr: false,
    precomp: crate::rewrite::Precomp::None,
};

/// An object's type stream, hashed structurally.
pub(crate) struct Stream {
    pub(crate) base: u32,
    pub(crate) precomp: Option<(u32, u32)>,
    /// Its `LF_PRECOMP`'s signature, or a PCH object's `LF_ENDPRECOMP`'s.
    pub(crate) signature: Option<u32>,
    pub(crate) hashes: Vec<u128>,
    /// A type server's: indices stand for themselves.
    pub(crate) server: bool,
}

pub(crate) fn index_hash(s: &Stream, pch: Option<&Stream>, v: u32) -> u128 {
    if v < FIRST || s.server {
        return u128::from(v);
    }
    if let Some((start, count)) = s.precomp
        && v >= start
        && v - start < count
    {
        return index_hash(pch.expect("a PCH user's PCH"), None, v);
    }
    s.hashes[(v - s.base) as usize]
}

/// The structural hashes of a `.debug$T`'s (or `.debug$P`'s) records.
pub(crate) fn stream_of(data: &[u8], pch: Option<&Stream>) -> Stream {
    let mut s = Stream {
        base: FIRST,
        precomp: None,
        signature: None,
        hashes: Vec::new(),
        server: false,
    };
    let mut refs = Vec::new();
    for r in remap::type_records(&data[4..]) {
        let (_, r) = r.unwrap();
        if s.hashes.is_empty() && s.precomp.is_none() {
            if r.kind == leaf::LF_TYPESERVER2 {
                s.server = true;
                return s;
            }
            if r.kind == leaf::LF_PRECOMP {
                let (start, count) = (le32(r.data, 0), le32(r.data, 4));
                s.precomp = Some((start, count));
                s.signature = Some(le32(r.data, 8));
                s.base = start + count;
                continue;
            }
        }
        if r.kind == leaf::LF_ENDPRECOMP && s.signature.is_none() {
            s.signature = Some(le32(r.data, 0));
        }
        refs.clear();
        type_refs::refs(r.kind, r.data, &mut refs).unwrap();
        let mut h = H128::new();
        h.u64(u64::from(r.kind));
        let mut from = 0;
        for x in &refs {
            let at = x.offset as usize;
            h.bytes(&r.data[from..at]);
            h.u128(index_hash(&s, pch, le32(r.data, at)));
            from = at + 4;
        }
        h.bytes(&r.data[from..]);
        s.hashes.push(h.finish());
    }
    s
}

pub(crate) fn stream(o: &Object<'_>, pch: Option<&Stream>) -> Stream {
    match o
        .sections
        .iter()
        .find(|s| s.name.as_bytes() == b".debug$T" || s.name.as_bytes() == b".debug$P")
    {
        Some(sec) => stream_of(sec.bytes(), pch),
        None => stream_of(&CV_SIGNATURE_C13.to_le_bytes(), pch),
    }
}

/// An object's per-object checksums and strings.
pub(crate) struct Tables {
    checksums: Vec<u8>,
    strings: Vec<u8>,
}

pub(crate) fn tables(o: &Object<'_>) -> Tables {
    let mut t = Tables {
        checksums: Vec::new(),
        strings: Vec::new(),
    };
    for s in &o.sections {
        if s.name.as_bytes() != b".debug$S" || is_comdat(s) {
            continue;
        }
        for x in c13::section_subsections(s.bytes()).unwrap() {
            match x.kind {
                sub::DEBUG_S_FILECHKSMS => t.checksums = x.data.to_vec(),
                sub::DEBUG_S_STRINGTABLE => t.strings = x.data.to_vec(),
                _ => {}
            }
        }
    }
    t
}

/// The file at checksum offset `v`: its name and checksum.
pub(crate) fn file(t: &Tables, v: u32) -> Vec<u8> {
    let at = v as usize;
    let name = le32(&t.checksums, at);
    let len = usize::from(t.checksums[at + 4]);
    let mut out = string(&t.strings, name).unwrap().to_vec();
    out.push(0);
    out.extend_from_slice(&t.checksums[at + 5..at + 6 + len]);
    out
}

/// A field's value, canonically.
fn canonical_field(
    f: Field,
    data: &[u8],
    s: &Stream,
    pch: Option<&Stream>,
    t: &Tables,
    out: &mut Vec<u8>,
) {
    let v = |at: u32| le32(data, at as usize);
    match f {
        Field::Type(at) | Field::Id(at) => {
            out.extend_from_slice(&index_hash(s, pch, v(at)).to_le_bytes());
        }
        Field::String(at) => out.extend_from_slice(string(&t.strings, v(at)).unwrap()),
        Field::Checksum(at) => out.extend_from_slice(&file(t, v(at))),
        Field::Annotations(at) => {
            for f in annotations::files(&data[at as usize..]).unwrap() {
                out.extend_from_slice(&file(t, f));
            }
            // The rest, with each file as 0, without the padding.
            let mut rest = Vec::new();
            annotations::rewrite_files(&data[at as usize..], &mut rest, |_| 0).unwrap();
            while rest.last() == Some(&0) {
                rest.pop();
            }
            out.extend_from_slice(&rest);
        }
        _ => {}
    }
}

/// A unit's `.debug$S` section, canonically: each record (or other
/// subsection's contents) as its bytes outside fields, then its fields'
/// values, then its relocations (place in it, kind, target's name).
pub(crate) fn canonical(
    o: &Object<'_>,
    sec: &Section<'_>,
    s: &Stream,
    pch: Option<&Stream>,
    t: &Tables,
) -> Vec<Vec<u8>> {
    let data = sec.bytes();
    // Each item: where it is in the section, and its canonical bytes.
    let mut items: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    let mut push = |at: usize, len: usize, kind: u32, k: usize, body: &[u8], fields: &[Field]| {
        let mut out = kind.to_le_bytes().to_vec();
        out.extend_from_slice(&(k as u32).to_le_bytes());
        let mut masked = body.to_vec();
        let mut ann = None;
        for f in fields {
            match *f {
                Field::Type(a) | Field::Id(a) | Field::String(a) | Field::Checksum(a) => {
                    masked[a as usize..a as usize + 4].fill(0);
                }
                Field::Annotations(a) => ann = Some(a as usize),
                _ => {}
            }
        }
        if let Some(a) = ann {
            masked.truncate(a);
        }
        out.extend_from_slice(&masked);
        for f in fields {
            canonical_field(*f, body, s, pch, t, &mut out);
        }
        items.push((at as u32, (at + len) as u32, out));
    };
    for (k, x) in c13::section_subsections(data)
        .unwrap()
        .into_iter()
        .enumerate()
    {
        let start = x.data.as_ptr() as usize - data.as_ptr() as usize;
        if x.kind == sub::DEBUG_S_SYMBOLS {
            for r in c13::symbol_records(x.data).unwrap() {
                let mut fields = Vec::new();
                symbol_refs::fields(r.kind, r.data, |f| fields.push(f)).unwrap();
                push(
                    start + r.offset as usize,
                    4 + r.data.len(),
                    u32::from(r.kind),
                    k,
                    r.data,
                    &fields,
                );
            }
        } else {
            let mut fields = Vec::new();
            c13::fields(x.kind, x.data, |f| fields.push(f)).unwrap();
            push(start, x.data.len(), x.kind, k, x.data, &fields);
        }
    }
    for r in &sec.relocations {
        let i = items
            .iter()
            .position(|&(a, b, _)| a <= r.offset && r.offset < b)
            .expect("a relocation in a record");
        let at = r.offset - items[i].0;
        let name = o.symbols[r.symbol.index()].name.as_bytes().to_vec();
        let item = &mut items[i].2;
        item.extend_from_slice(&at.to_le_bytes());
        item.extend_from_slice(&r.kind.to_le_bytes());
        item.extend_from_slice(&name);
    }
    items.into_iter().map(|(_, _, v)| v).collect()
}

/// The COMDAT `.debug$S` sections of `o`, by the name of the external
/// symbol in the section their association chain ends at.
pub(crate) fn unit_sections<'o>(o: &'o Object<'o>) -> Vec<(Vec<u8>, &'o Section<'o>)> {
    let mut assoc: HashMap<u32, u32> = HashMap::default();
    for s in &o.symbols {
        if let (SectionNumber::Section(id), Aux::Section(d)) = (s.section, &s.aux)
            && let Some(n) = d.number
            && d.selection == turbonop_coff::comdat_select::ASSOCIATIVE
        {
            assoc.insert(id.0, n.0);
        }
    }
    let mut out = Vec::new();
    for (i, sec) in o.sections.iter().enumerate() {
        if sec.name.as_bytes() != b".debug$S" || !is_comdat(sec) {
            continue;
        }
        let mut at = i as u32;
        while let Some(&n) = assoc.get(&at) {
            at = n;
        }
        if let Some(s) = o
            .symbols
            .iter()
            .find(|s| s.section == SectionNumber::Section(SectionId(at)) && s.is_external())
        {
            out.push((s.name.as_bytes().to_vec(), &o.sections[i]));
        }
    }
    out
}

pub(crate) fn inputs<'a>(names: &'a [&str], datas: &'a [Vec<u8>]) -> Vec<Input<'a>> {
    names
        .iter()
        .zip(datas)
        .map(|(n, d)| Input {
            name: n.as_bytes(),
            data: d,
        })
        .collect()
}

/// `RELAXED` with the library's types shared (`--precomp=library`).
pub(crate) const RELAXED_LIBRARY: Options = Options {
    precomp: crate::rewrite::Precomp::Library { min: 2 },
    ..RELAXED
};

/// Folds `names` and checks every synthesized member's CodeView against
/// the copies it came from; the number of unit sections checked. `pch`
/// names the users' PCH object, if any (among them or not). Done with
/// each member's own types, and with the library's shared: the same units
/// are checked.
fn check(names: &[&str], pch: Option<&str>) -> usize {
    let datas: Vec<Vec<u8>> = names.iter().map(|n| fixture(n)).collect();
    check_datas(names, &datas, pch)
}

fn check_datas(names: &[&str], datas: &[Vec<u8>], pch: Option<&str>) -> usize {
    let n = check_with(names, datas, pch, RELAXED);
    assert_eq!(check_with(names, datas, pch, RELAXED_LIBRARY), n);
    n
}

/// The PCH among `pchs` that `o`'s `LF_PRECOMP` names, by signature.
pub(crate) fn pch_of<'s>(o: &Object<'_>, pchs: &'s [Stream]) -> Option<&'s Stream> {
    let t = o
        .sections
        .iter()
        .find(|s| s.name.as_bytes() == b".debug$T")?;
    let (_, r) = remap::type_records(&t.bytes()[4..]).next()?.ok()?;
    let sig = (r.kind == leaf::LF_PRECOMP).then(|| le32(r.data, 8))?;
    pchs.iter()
        .find(|p| p.precomp.is_none() && p.signature == Some(sig))
}

/// The PCH objects' streams among `pch` and `members`.
pub(crate) fn pch_streams(pch: Option<&[u8]>, members: &[&[u8]]) -> Vec<Stream> {
    pch.into_iter()
        .chain(members.iter().copied())
        .filter_map(|d| {
            let o = object(d);
            o.sections
                .iter()
                .any(|s| s.name.as_bytes() == b".debug$P")
                .then(|| stream(&o, None))
        })
        .collect()
}

pub(crate) fn check_with(
    names: &[&str],
    datas: &[Vec<u8>],
    pch: Option<&str>,
    options: Options,
) -> usize {
    let out = rewrite(&inputs(names, datas), options);
    assert!(out.report.synthesized > 0, "{:?}", out.report);
    let objects: Vec<Object<'_>> = datas.iter().map(|d| object(d)).collect();
    let pch_data = pch.map(fixture);
    let made: Vec<&[u8]> = out
        .members
        .iter()
        .filter(|m| m.synthesized)
        .map(|m| &m.data[..])
        .collect();
    let pchs = pch_streams(pch_data.as_deref(), &made);
    let streams: Vec<Stream> = objects
        .iter()
        .map(|o| stream(o, pch_of(o, &pchs)))
        .collect();
    let tabs: Vec<Tables> = objects.iter().map(tables).collect();
    let mut checked = 0;
    for m in out.members.iter().filter(|m| m.synthesized) {
        let o = object(&m.data);
        if o.sections.iter().any(|s| s.name.as_bytes() == b".debug$P") {
            // The precompiled-types member (see precomp's tests).
            continue;
        }
        well_formed(&o, &m.name);
        let pch_stream = pch_of(&o, &pchs);
        let s = stream(&o, pch_stream);
        let t = tables(&o);
        for (name, sec) in unit_sections(&o) {
            // The copy that survived: the first member's.
            let (k, original) = objects
                .iter()
                .enumerate()
                .find_map(|(k, x)| {
                    unit_sections(x)
                        .into_iter()
                        .find(|(n, _)| *n == name)
                        .map(|(_, s)| (k, s))
                })
                .expect("the original");
            let want = canonical(
                &objects[k],
                original,
                &streams[k],
                pch_of(&objects[k], &pchs),
                &tabs[k],
            );
            let got = canonical(&o, sec, &s, pch_stream, &t);
            assert_eq!(got, want, "{}", String::from_utf8_lossy(&name));
            checked += 1;
        }
        inlinees_carried(&o, &objects, &streams, &tabs, &pchs);
    }
    checked
}

/// A synthesized member's own CodeView: `S_OBJNAME` naming it and the
/// compiler first, and type records each naming only earlier ones, no two
/// the same.
pub(crate) fn well_formed(o: &Object<'_>, name: &[u8]) {
    let first = o
        .sections
        .iter()
        .find(|s| s.name.as_bytes() == b".debug$S")
        .unwrap();
    assert!(!is_comdat(first), "the per-object .debug$S first");
    let x = &c13::section_subsections(first.bytes()).unwrap()[0];
    let records = c13::symbol_records(x.data).unwrap();
    assert_eq!(records[0].kind, sym::S_OBJNAME);
    let mut want = b"turbolib:".to_vec();
    want.extend_from_slice(name);
    want.push(0);
    assert_eq!(&records[0].data[4..], &want[..]);
    assert_eq!(records[1].kind, sym::S_COMPILE3);
    assert!(
        !records.iter().any(|r| r.kind == sym::S_BUILDINFO),
        "no S_BUILDINFO"
    );

    if let Some(t) = o.sections.iter().find(|s| s.name.as_bytes() == b".debug$T") {
        let mut seen = HashSet::default();
        let mut refs = Vec::new();
        let mut index = FIRST;
        for (i, r) in remap::type_records(&t.bytes()[4..]).enumerate() {
            let (at, r) = r.unwrap();
            if r.kind == leaf::LF_TYPESERVER2 {
                continue;
            }
            if i == 0 && r.kind == leaf::LF_PRECOMP {
                // A PCH's records come first.
                index = le32(r.data, 0) + le32(r.data, 4);
                continue;
            }
            refs.clear();
            type_refs::refs(r.kind, r.data, &mut refs).unwrap();
            for x in &refs {
                let v = le32(r.data, x.offset as usize);
                assert!(v < index, "record {index:#x} names {v:#x}");
            }
            let len = 4 + r.data.len();
            assert!(
                seen.insert(t.bytes()[4 + at..4 + at + len].to_vec()),
                "a duplicate"
            );
            index += 1;
        }
    }
}

/// The per-object inlinee lines of `o`.
fn entries(o: &Object<'_>) -> Vec<InlineeSource> {
    let mut v = Vec::new();
    for sec in &o.sections {
        if sec.name.as_bytes() != b".debug$S" || is_comdat(sec) {
            continue;
        }
        for x in c13::section_subsections(sec.bytes()).unwrap() {
            if x.kind == sub::DEBUG_S_INLINEELINES {
                v.extend(c13::InlineeLines::parse(x.data).unwrap().entries);
            }
        }
    }
    v
}

/// Every inline site's function has an inlinee lines entry in the
/// synthesized member, and each entry is one of the originals' (the same
/// function, file, and line).
fn inlinees_carried(
    o: &Object<'_>,
    objects: &[Object<'_>],
    streams: &[Stream],
    tabs: &[Tables],
    pchs: &[Stream],
) {
    let pch = pch_of(o, pchs);
    let s = stream(o, pch);
    let t = tables(o);
    let mut known = HashSet::default();
    for (k, x) in objects.iter().enumerate() {
        for e in entries(x) {
            known.insert((
                index_hash(&streams[k], pch_of(x, pchs), e.inlinee),
                file(&tabs[k], e.file),
                e.line,
            ));
        }
    }
    let mine = entries(o);
    let have: HashSet<u32> = mine.iter().map(|e| e.inlinee).collect();
    for e in &mine {
        assert!(known.contains(&(index_hash(&s, pch, e.inlinee), file(&t, e.file), e.line)));
    }
    let mut sites = 0;
    for (_, sec) in unit_sections(o) {
        for x in c13::section_subsections(sec.bytes()).unwrap() {
            if x.kind != sub::DEBUG_S_SYMBOLS {
                continue;
            }
            for r in c13::symbol_records(x.data).unwrap() {
                if r.kind == sym::S_INLINESITE {
                    sites += 1;
                    assert!(have.contains(&le32(r.data, 8)), "an inline site's entry");
                }
            }
        }
    }
    assert_eq!(sites == 0, mine.is_empty());
}

#[test]
fn msvc_od_units_keep_their_codeview() {
    let n = check(&["msvc-a.obj", "msvc-b.obj"], None);
    assert!(n >= 5, "{n}");
}

#[test]
fn msvc_o2_inline_sites_and_inlinee_lines() {
    let n = check(&["msvc-c.obj", "msvc-d.obj"], None);
    assert!(n >= 2, "{n}");
    // In the other order, d's copies survive.
    let n = check(&["msvc-d.obj", "msvc-c.obj"], None);
    assert!(n >= 2, "{n}");
}

#[test]
fn clang_units_keep_their_codeview() {
    assert!(check(&["clang-a.obj", "clang-b.obj"], None) >= 5);
    assert!(check(&["clang-c.obj", "clang-d.obj"], None) >= 2);
}

#[test]
fn pch_users_types_come_from_their_pch() {
    let n = check(
        &["msvc-pch.obj", "msvc-u1.obj", "msvc-u2.obj"],
        Some("msvc-pch.obj"),
    );
    assert!(n >= 5, "{n}");
    // The PCH object can come after its users.
    let n = check(
        &["msvc-u1.obj", "msvc-u2.obj", "msvc-pch.obj"],
        Some("msvc-pch.obj"),
    );
    assert!(n >= 5, "{n}");
}

/// `u1` and `u2` with their PCH object's `/INCLUDE` removed.
fn users_without_include() -> Vec<Vec<u8>> {
    ["msvc-u1.obj", "msvc-u2.obj"]
        .iter()
        .map(|n| {
            let data = fixture(n);
            let mut o = object(&data);
            for s in &mut o.sections {
                if s.name.as_bytes() == b".drectve" {
                    let text: Vec<&[u8]> = units::directives(s.bytes())
                        .into_iter()
                        .filter(|w| !w.to_ascii_lowercase()[1..].starts_with(b"include:"))
                        .collect();
                    s.data = SectionData::Bytes(Cow::Owned(text.join(&b' ')));
                }
            }
            o.write().unwrap()
        })
        .collect()
}

#[test]
fn pch_users_name_their_pch_wherever_its_object_is() {
    // Without the PCH object in the library: the synthesized member names
    // the same PCH, and refers to its object's symbol.
    let names = ["msvc-u1.obj", "msvc-u2.obj"];
    let n = check(&names, Some("msvc-pch.obj"));
    assert!(n >= 5, "{n}");
    let datas: Vec<Vec<u8>> = names.iter().map(|n| fixture(n)).collect();
    let out = rewrite(&inputs(&names, &datas), RELAXED);
    let first = |o: &Object<'_>| {
        let t = o
            .sections
            .iter()
            .find(|s| s.name.as_bytes() == b".debug$T")
            .unwrap();
        let (_, r) = remap::type_records(&t.bytes()[4..])
            .next()
            .unwrap()
            .unwrap();
        (r.kind, r.data.to_vec())
    };
    let want = first(&object(&datas[0]));
    assert_eq!(want.0, leaf::LF_PRECOMP);
    for m in out.members.iter().filter(|m| m.synthesized) {
        let o = object(&m.data);
        if !o.sections.iter().any(|s| s.name.as_bytes() == b".debug$T") {
            continue;
        }
        assert_eq!(first(&o), want);
        assert!(
            o.symbols
                .iter()
                .any(|s| s.section == SectionNumber::Undefined
                    && s.name.as_bytes().starts_with(b"__@@_PchSym_"))
        );
    }
}

#[test]
fn pch_users_that_dont_name_their_pch_copy_its_types_or_dont_fold() {
    let users = users_without_include();
    // With the PCH object in the library, its records are copied.
    let mut datas = users.clone();
    datas.push(fixture("msvc-pch.obj"));
    let names = ["u1.obj", "u2.obj", "pch.obj"];
    let n = check_datas(&names, &datas, Some("msvc-pch.obj"));
    assert!(n >= 5, "{n}");
    let out = rewrite(&inputs(&names, &datas), RELAXED);
    for m in out.members.iter().filter(|m| m.synthesized) {
        let o = object(&m.data);
        if let Some(t) = o.sections.iter().find(|s| s.name.as_bytes() == b".debug$T") {
            let (_, r) = remap::type_records(&t.bytes()[4..])
                .next()
                .unwrap()
                .unwrap();
            assert_ne!(r.kind, leaf::LF_PRECOMP);
        }
    }
    // Without it, their units with debug information don't fold, and
    // what folds has none.
    let out = rewrite(&inputs(&names[..2], &users), RELAXED);
    for m in out.members.iter().filter(|m| m.synthesized) {
        let o = object(&m.data);
        assert!(
            !o.sections
                .iter()
                .any(|s| units::is_debug(s.name.as_bytes()))
        );
    }
    assert!(
        out.report
            .not_folded
            .contains_key(&Unfoldable::PrecompMissing),
        "{:?}",
        out.report
    );
}

#[test]
fn zi_units_name_the_type_server_and_keep_their_indices() {
    let names = ["msvc-zi-a.obj", "msvc-zi-b.obj"];
    let n = check(&names, None);
    assert!(n >= 5, "{n}");
    let datas: Vec<Vec<u8>> = names.iter().map(|n| fixture(n)).collect();
    let out = rewrite(&inputs(&names, &datas), RELAXED);
    let types = |o: &Object<'_>| -> Vec<u8> {
        o.sections
            .iter()
            .find(|s| s.name.as_bytes() == b".debug$T")
            .unwrap()
            .bytes()
            .to_vec()
    };
    let synth = out.members.iter().find(|m| m.synthesized).unwrap();
    let t = types(&object(&synth.data));
    let records: Vec<_> = remap::type_records(&t[4..]).map(|r| r.unwrap().1).collect();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, leaf::LF_TYPESERVER2);
    // The record of the members the units came from (the first copies,
    // a's): the ages differ (each compile into the PDB adds one), and the
    // key doesn't hold them.
    let a = types(&object(&datas[0]));
    let b = types(&object(&datas[1]));
    assert_ne!(a, b);
    assert_eq!(t, a);
}

#[test]
fn types_are_merged_once() {
    let names = ["msvc-a.obj", "msvc-b.obj"];
    let datas: Vec<Vec<u8>> = names.iter().map(|n| fixture(n)).collect();
    let out = rewrite(&inputs(&names, &datas), RELAXED);
    let synth = out.members.iter().find(|m| m.synthesized).unwrap();
    let s = stream(&object(&synth.data), None);
    // No two records are structurally the same.
    let distinct: HashSet<u128> = s.hashes.iter().copied().collect();
    assert_eq!(distinct.len(), s.hashes.len());
    let total: usize = datas
        .iter()
        .map(|d| stream(&object(d), None).hashes.len())
        .sum();
    assert!(s.hashes.len() < total);
}

/// A rebuilt `.debug$S` (an inline site's annotations change length):
/// with every checksum offset pushed past 0x4000, each file change in the
/// annotations takes 4 bytes rather than 1 or 2.
#[test]
fn annotations_that_change_length_rebuild_the_section() {
    // (step1's body is partly in step1.inc, so its inline sites change
    // files.)
    for name in ["msvc-c.obj", "clang-c.obj"] {
        let data = fixture(name);
        let input = [Input {
            name: name.as_bytes(),
            data: &data,
        }];
        let parsed: Vec<Parsed<'_>> = input
            .iter()
            .map(|i| super::super::parse(i, units::Mode::default(), false))
            .collect();
        let o = parsed[0].object.as_ref().unwrap();
        let s = stream(o, None);
        let t = tables(o);
        let mut rebuilt = 0;
        for (k, unit) in parsed[0].facts.units.iter().enumerate() {
            for &id in &unit.sections {
                let sec = &o.sections[id.index()];
                if sec.name.as_bytes() != b".debug$S" {
                    continue;
                }
                let mut b = Builder::new(&parsed, super::super::Objects::kept(&parsed), None);
                b.checksums.resize(0x4000, 0);
                let mut copy = b.section(k, 0, id, sec);
                copy.relocations = sec
                    .relocations
                    .iter()
                    .map(|r| turbonop_coff::Relocation {
                        offset: b.offset(k, id, r.offset),
                        ..*r
                    })
                    .collect();
                rebuilt += usize::from(b.moved.contains_key(&(k, id.0)));
                // Read through the builder's own tables and types.
                let mine = Tables {
                    checksums: b.checksums.clone(),
                    strings: b.strings.bytes().to_vec(),
                };
                let mut types = CV_SIGNATURE_C13.to_le_bytes().to_vec();
                types.extend_from_slice(&b.types);
                let mine_s = stream_of(&types, None);
                assert_eq!(
                    canonical(o, &copy, &mine_s, None, &mine),
                    canonical(o, sec, &s, None, &t),
                    "{name}"
                );
            }
        }
        assert!(rebuilt > 0, "{name}: no section was rebuilt");
    }
}

#[test]
fn unusable_types_make_units_with_debug_information_unfoldable() {
    // A record naming one after it.
    let data = fixture("msvc-a.obj");
    let mut o = object(&data);
    let t = o
        .sections
        .iter()
        .position(|s| s.name.as_bytes() == b".debug$T")
        .unwrap();
    let mut bytes = o.sections[t].bytes().to_vec();
    let mut refs = Vec::new();
    let mut at_ref = None;
    for r in remap::type_records(&bytes[4..]) {
        let (at, r) = r.unwrap();
        refs.clear();
        type_refs::refs(r.kind, r.data, &mut refs).unwrap();
        if let Some(x) = refs.first() {
            at_ref = Some(4 + at + 4 + x.offset as usize);
            break;
        }
    }
    let p = at_ref.unwrap();
    bytes[p..p + 4].copy_from_slice(&0xFFFFu32.to_le_bytes());
    o.sections[t].data = SectionData::Bytes(Cow::Owned(bytes));
    let written = o.write().unwrap();
    let o = object(&written);
    let mut facts = units::find(&o).unwrap();
    let d = read(&o, &mut facts);
    assert!(matches!(d.types, Types::Bad(_)));
    let mut with_debug = 0;
    for u in &facts.units {
        let debug = u
            .sections
            .iter()
            .any(|s| units::is_debug(o.sections[s.index()].name.as_bytes()));
        if debug && u.unfoldable != Some(Unfoldable::InternalLinkage) {
            with_debug += 1;
            assert_eq!(u.unfoldable, Some(Unfoldable::DebugInfo));
        }
    }
    assert!(with_debug > 0);
}

#[test]
fn a_unit_whose_local_symbol_the_per_object_debug_s_names_doesnt_fold() {
    // A relocation from the per-object .debug$S to a unit's section symbol,
    // as an S_LDATA32 or S_LPROC32 there for something in the unit would
    // make: removing the unit would leave it dangling.
    let data = fixture("msvc-a.obj");
    let mut o = object(&data);
    let main = o
        .sections
        .iter()
        .position(|s| s.name.as_bytes() == b".debug$S" && !is_comdat(s))
        .unwrap();
    let facts = units::find(&o).unwrap();
    let unit = facts
        .units
        .iter()
        .find(|u| u.unfoldable.is_none())
        .expect("a foldable unit");
    let local = unit
        .symbols
        .iter()
        .copied()
        .find(|&s| !o.symbols[s.index()].is_external())
        .expect("a local symbol (its section's)");
    let leader = unit.leader;
    o.sections[main]
        .relocations
        .push(turbonop_coff::Relocation {
            offset: 8,
            symbol: local,
            kind: 0x000B, // IMAGE_REL_AMD64_SECREL
        });
    let written = o.write().unwrap();
    let o = object(&written);
    let facts = units::find(&o).unwrap();
    let u = facts.units.iter().find(|u| u.leader == leader).unwrap();
    assert_eq!(u.unfoldable, Some(Unfoldable::ReferencedLocally));
}

#[test]
fn units_without_debug_sections_add_no_codeview() {
    // objects.obj's units have .debug$S; with them stripped from both
    // copies, the synthesized member has none either.
    let data = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../testdata/coff/objects.obj"),
    )
    .unwrap();
    let mut o = object(&data);
    let debug: Vec<bool> = o
        .sections
        .iter()
        .map(|s| units::is_debug(s.name.as_bytes()))
        .collect();
    o.remove_sections(|id, _| debug[id.index()]).unwrap();
    let stripped = o.write().unwrap();
    let datas = vec![stripped.clone(), stripped];
    let out = rewrite(&inputs(&["a.obj", "b.obj"], &datas), RELAXED);
    let synth = out.members.iter().find(|m| m.synthesized).unwrap();
    let o = object(&synth.data);
    assert!(
        !o.sections
            .iter()
            .any(|s| units::is_debug(s.name.as_bytes()))
    );
}

use std::borrow::Cow;
use std::collections::BTreeMap;

use super::*;
use turbonop_macho_object::consts::{
    CPU_TYPE_ARM64, MH_MAGIC_64, MH_OBJECT, S_ATTR_DEBUG, S_ATTR_PURE_INSTRUCTIONS,
};
use turbonop_macho_object::{
    Command, Endian, Header, MH_SUBSECTIONS_VIA_SYMBOLS, Relocation, Section, Segment, name_field,
};

/// An attribute value of a test DIE.
#[derive(Clone, Copy)]
enum V {
    /// A reference to the DIE of this id.
    Ref(usize),
    /// `DW_AT_sibling`: the DIE after this one's subtree.
    Sibling,
    Str(&'static str),
    /// An address in `__text`, relocated.
    Addr(u64),
    /// An address of lost code: all ones, no relocation (a place `gone`).
    Gone,
    D4(u32),
    D1(u8),
}

struct T {
    id: usize,
    tag: u16,
    attrs: Vec<(u16, V)>,
    kids: Vec<T>,
    children: bool,
}

fn die(id: usize, tag: u16, attrs: Vec<(u16, V)>, kids: Vec<T>) -> T {
    let children = !kids.is_empty() || tag == DW_TAG_COMPILE_UNIT || tag == DW_TAG_NAMESPACE;
    T {
        id,
        tag,
        attrs,
        kids,
        children,
    }
}

fn form(v: V) -> u16 {
    match v {
        V::Ref(_) | V::Sibling => DW_FORM_REF4,
        V::Str(_) => DW_FORM_STRP,
        V::Addr(_) | V::Gone => DW_FORM_ADDR,
        V::D4(_) => DW_FORM_DATA4,
        V::D1(_) => DW_FORM_DATA1,
    }
}

/// An abbreviation: tag, children, and attributes (name, form).
type Shape = (u16, bool, Vec<(u16, u16)>);

fn shape(t: &T) -> Shape {
    (
        t.tag,
        t.children,
        t.attrs.iter().map(|&(n, v)| (n, form(v))).collect(),
    )
}

/// The abbreviations of `t`'s tree, by first use.
fn abbrevs(t: &T, out: &mut Vec<Shape>) {
    let s = shape(t);
    if !out.contains(&s) {
        out.push(s);
    }
    for k in &t.kids {
        abbrevs(k, out);
    }
}

fn abbrev_bytes(a: &[Shape]) -> Vec<u8> {
    let mut b = Vec::new();
    for (i, (tag, ch, attrs)) in a.iter().enumerate() {
        b.push(i as u8 + 1);
        b.push(*tag as u8);
        b.push(u8::from(*ch));
        for &(n, f) in attrs {
            b.push(n as u8);
            b.push(f as u8);
        }
        b.extend_from_slice(&[0, 0]);
    }
    b.push(0);
    b
}

/// The strings of `t`'s tree, by first use.
fn strings(t: &T, out: &mut Vec<&'static str>) {
    for &(_, v) in &t.attrs {
        if let V::Str(s) = v
            && !out.contains(&s)
        {
            out.push(s);
        }
    }
    for k in &t.kids {
        strings(k, out);
    }
}

struct Emitted {
    info: Vec<u8>,
    /// Relocated places, and places of lost code.
    relocs: Vec<u32>,
    gone: Vec<u32>,
    offsets: BTreeMap<usize, u32>,
}

fn size(t: &T, keep: &dyn Fn(usize) -> bool) -> u32 {
    let mut n = 1;
    for &(_, v) in &t.attrs {
        n += match v {
            V::Ref(_) | V::Sibling | V::Str(_) | V::D4(_) => 4,
            V::Addr(_) | V::Gone => 8,
            V::D1(_) => 1,
        };
    }
    if t.children {
        n += t
            .kids
            .iter()
            .filter(|k| keep(k.id))
            .map(|k| size(k, keep))
            .sum::<u32>()
            + 1;
    }
    n
}

fn layout(t: &T, at: u32, keep: &dyn Fn(usize) -> bool, offsets: &mut BTreeMap<usize, u32>) {
    offsets.insert(t.id, at);
    let mut x = at + size(t, keep)
        - if t.children {
            t.kids
                .iter()
                .filter(|k| keep(k.id))
                .map(|k| size(k, keep))
                .sum::<u32>()
                + 1
        } else {
            0
        };
    for k in t.kids.iter().filter(|k| keep(k.id)) {
        layout(k, x, keep, offsets);
        x += size(k, keep);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_die(
    t: &T,
    keep: &dyn Fn(usize) -> bool,
    ab: &[Shape],
    pool: &[&str],
    str_at: &dyn Fn(&str) -> u32,
    e: &mut Emitted,
) {
    let _ = pool;
    let code = ab.iter().position(|a| *a == shape(t)).unwrap() as u8 + 1;
    let start = e.info.len() as u32;
    e.info.push(code);
    for &(_, v) in &t.attrs {
        let pos = e.info.len() as u32;
        match v {
            V::Ref(id) => e.info.extend_from_slice(&e.offsets[&id].to_le_bytes()),
            V::Sibling => {
                let end = start + size(t, keep);
                e.info.extend_from_slice(&end.to_le_bytes());
            }
            V::Str(s) => e.info.extend_from_slice(&str_at(s).to_le_bytes()),
            V::Addr(a) => {
                e.relocs.push(pos);
                e.info.extend_from_slice(&a.to_le_bytes());
            }
            V::Gone => {
                e.gone.push(pos);
                e.info.extend_from_slice(&u64::MAX.to_le_bytes());
            }
            V::D4(x) => e.info.extend_from_slice(&x.to_le_bytes()),
            V::D1(x) => e.info.push(x),
        }
    }
    if t.children {
        for k in t.kids.iter().filter(|k| keep(k.id)) {
            emit_die(k, keep, ab, pool, str_at, e);
        }
        e.info.push(0);
    }
}

/// `t`'s unit with only the DIEs `keep` keeps; the strings: `pool`, of
/// which those `used` keeps.
fn emit(
    t: &T,
    keep: &dyn Fn(usize) -> bool,
    ab: &[Shape],
    pool: &[&'static str],
    used: &dyn Fn(&str) -> bool,
) -> (Emitted, Vec<u8>) {
    let mut strs = Vec::new();
    let mut at = BTreeMap::new();
    for &s in pool.iter().filter(|s| used(s)) {
        at.insert(s, strs.len() as u32);
        strs.extend_from_slice(s.as_bytes());
        strs.push(0);
    }
    let mut e = Emitted {
        info: Vec::new(),
        relocs: Vec::new(),
        gone: Vec::new(),
        offsets: BTreeMap::new(),
    };
    layout(t, 11, keep, &mut e.offsets);
    e.info.extend_from_slice(&[0; 4]);
    e.info.extend_from_slice(&4u16.to_le_bytes());
    e.info.extend_from_slice(&0u32.to_le_bytes());
    e.info.push(8);
    emit_die(t, keep, ab, pool, &|s| at[s], &mut e);
    let len = e.info.len() as u32 - 4;
    e.info[..4].copy_from_slice(&len.to_le_bytes());
    (e, strs)
}

fn djb(s: &str) -> u32 {
    s.bytes().fold(5381u32, |h, c| {
        h.wrapping_mul(33).wrapping_add(u32::from(c))
    })
}

/// An `__apple_names` of `names` (each a name and its DIEs' offsets).
fn apple_names(names: &[(&str, Vec<u32>)], str_at: &dyn Fn(&str) -> u32) -> Vec<u8> {
    let mut header = Vec::new();
    header.extend_from_slice(&APPLE_MAGIC.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&0u16.to_le_bytes());
    header.extend_from_slice(&[0; 8]);
    header.extend_from_slice(&12u32.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&1u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&DW_FORM_DATA4.to_le_bytes());
    let t = Apple {
        header,
        buckets: 0,
        die_at: 0,
        entry: 4,
        base: 0,
        names: Vec::new(),
    };
    let names: Vec<Name> = names
        .iter()
        .map(|(s, v)| {
            (
                djb(s),
                str_at(s),
                v.iter().flat_map(|o| o.to_le_bytes()).collect(),
            )
        })
        .collect();
    write_apple(&t, &names, None)
}

fn section(
    seg: &[u8],
    name: &[u8],
    flags: u32,
    bytes: Vec<u8>,
    rels: Vec<Relocation>,
) -> Section<'static> {
    Section {
        sectname: name_field(name),
        segname: name_field(seg),
        addr: 0,
        align: 0,
        flags,
        reserved1: 0,
        reserved2: 0,
        reserved3: 0,
        contents: Contents::Bytes(Cow::Owned(bytes)),
        relocations: rels,
    }
}

fn object(sections: Vec<Section<'static>>) -> Object<'static> {
    let header = Header {
        magic: MH_MAGIC_64,
        endian: Endian::Little,
        cputype: CPU_TYPE_ARM64,
        cpusubtype: 0,
        filetype: MH_OBJECT,
        flags: MH_SUBSECTIONS_VIA_SYMBOLS,
        reserved: 0,
    };
    let seg = Segment {
        segname: name_field(b""),
        vmaddr: 0,
        maxprot: 7,
        initprot: 7,
        flags: 0,
        sections,
    };
    let mut o = Object::new(
        header,
        vec![
            Command::Segment(seg),
            Command::Symtab,
            Command::Dysymtab(Default::default()),
        ],
    );
    assert!(o.assign_addresses().is_empty());
    o
}

fn bytes_of<'a>(o: &'a Object<'_>, name: &[u8]) -> &'a [u8] {
    o.sections()
        .find(|s| s.name() == name)
        .and_then(|s| s.bytes())
        .unwrap()
}

/// The test unit: what the dead code alone names goes; what something
/// kept names stays (a dead subprogram a kept call site names, a type a
/// kept subprogram names), and so does a named type; a namespace of dead code goes; a namespace's
/// sibling reference moves to the next sibling kept.
fn unit() -> T {
    use V::*;
    let sub = |id, name, at: V, ty: Option<usize>, kids| {
        let mut a = vec![
            (DW_AT_LOW_PC, at),
            (DW_AT_HIGH_PC, D4(4)),
            (DW_AT_NAME, Str(name)),
        ];
        if let Some(t) = ty {
            a.push((DW_AT_TYPE, Ref(t)));
        }
        die(id, DW_TAG_SUBPROGRAM, a, kids)
    };
    die(
        0,
        DW_TAG_COMPILE_UNIT,
        vec![
            (DW_AT_PRODUCER, Str("clang")),
            (DW_AT_LOW_PC, Addr(0)),
            (DW_AT_HIGH_PC, D4(0x40)),
        ],
        vec![
            die(
                1,
                DW_TAG_BASE_TYPE,
                vec![
                    (DW_AT_NAME, Str("int")),
                    (DW_AT_ENCODING, D1(5)),
                    (DW_AT_BYTE_SIZE, D1(4)),
                ],
                vec![],
            ),
            die(
                2,
                DW_TAG_NAMESPACE,
                vec![(DW_AT_SIBLING, Sibling), (DW_AT_NAME, Str("ns"))],
                vec![
                    sub(10, "dead", Gone, Some(3), vec![]),
                    sub(11, "live", Addr(0x10), Some(1), vec![]),
                ],
            ),
            die(3, DW_TAG_POINTER_TYPE, vec![(DW_AT_TYPE, Ref(4))], vec![]),
            die(
                4,
                DW_TAG_STRUCTURE_TYPE,
                vec![(DW_AT_NAME, Str("T")), (DW_AT_BYTE_SIZE, D1(1))],
                vec![],
            ),
            die(
                5,
                DW_TAG_STRUCTURE_TYPE,
                vec![(DW_AT_NAME, Str("U")), (DW_AT_BYTE_SIZE, D1(1))],
                vec![],
            ),
            sub(12, "dead2", Gone, Some(5), vec![]),
            sub(
                13,
                "live2",
                Addr(0x20),
                Some(5),
                vec![die(
                    20,
                    DW_TAG_CALL_SITE,
                    vec![(DW_AT_CALL_ORIGIN, Ref(14))],
                    vec![],
                )],
            ),
            sub(14, "dead3", Gone, None, vec![]),
            die(
                6,
                DW_TAG_NAMESPACE,
                vec![(DW_AT_NAME, Str("empty_ns"))],
                vec![sub(15, "dead4", Gone, None, vec![])],
            ),
        ],
    )
}

#[test]
fn removes_dead_code_and_what_only_it_names() {
    let t = unit();
    let mut ab = Vec::new();
    abbrevs(&t, &mut ab);
    let mut pool = Vec::new();
    strings(&t, &mut pool);
    let all = |_: usize| true;
    let (full, strs) = emit(&t, &all, &ab, &pool, &|_| true);
    let full_at = |s: &str| {
        let mut n = 0;
        for p in &pool {
            if *p == s {
                return n;
            }
            n += p.len() as u32 + 1;
        }
        unreachable!()
    };
    let subs = ["dead", "live", "dead2", "live2", "dead3", "dead4"];
    let ids = [10, 11, 12, 13, 14, 15];
    let names: Vec<(&str, Vec<u32>)> = subs
        .iter()
        .zip(ids)
        .map(|(s, id)| (*s, vec![full.offsets[&id]]))
        .collect();
    let table = apple_names(&names, &full_at);
    let rels: Vec<Relocation> = full
        .relocs
        .iter()
        .rev()
        .map(|&p| Relocation::plain(p, 1, false, 0, 3))
        .collect();
    let mut o = object(vec![
        section(
            b"__TEXT",
            b"__text",
            S_ATTR_PURE_INSTRUCTIONS,
            vec![0; 0x40],
            Vec::new(),
        ),
        section(
            b"__DWARF",
            b"__debug_abbrev",
            S_ATTR_DEBUG,
            abbrev_bytes(&ab),
            Vec::new(),
        ),
        section(
            b"__DWARF",
            b"__debug_info",
            S_ATTR_DEBUG,
            full.info.clone(),
            rels,
        ),
        section(b"__DWARF", b"__debug_str", S_ATTR_DEBUG, strs, Vec::new()),
        section(
            b"__DWARF",
            b"__apple_names",
            S_ATTR_DEBUG,
            table,
            Vec::new(),
        ),
    ]);
    let gone: BTreeSet<u32> = full.gone.iter().copied().collect();
    let st = prune(&mut o, &gone, &BTreeSet::new()).unwrap().unwrap();
    assert_eq!(
        st,
        Pruned {
            dead: 3,
            dead_kept: 1,
            reached: 2,
            bytes: st.bytes,
        }
    );
    // What it should be: the same unit emitted without what went.
    let removed = [10usize, 3, 12, 6, 15];
    let keep = |id: usize| !removed.contains(&id);
    let gone_strs = ["dead", "dead2", "empty_ns", "dead4"];
    let (want, want_strs) = emit(&t, &keep, &ab, &pool, &|s| !gone_strs.contains(&s));
    assert_eq!(bytes_of(&o, b"__debug_info"), want.info.as_slice());
    assert_eq!(bytes_of(&o, b"__debug_str"), want_strs.as_slice());
    let info_rels: Vec<u32> = o
        .sections()
        .find(|s| s.name() == b"__debug_info")
        .unwrap()
        .relocations
        .iter()
        .map(|r| r.address)
        .rev()
        .collect();
    assert_eq!(info_rels, want.relocs);
    // The accelerator table: the kept DIEs' entries, at their new places.
    let a = apple(bytes_of(&o, b"__apple_names")).unwrap();
    let mut got: Vec<(u32, u32, u32)> = a
        .names
        .iter()
        .map(|(h, s, e)| (*h, *s, u32::from_le_bytes(e[..4].try_into().unwrap())))
        .collect();
    got.sort_unstable();
    let new_at = |s: &str| {
        let mut n = 0;
        for p in pool.iter().filter(|p| !gone_strs.contains(p)) {
            if *p == s {
                return n;
            }
            n += p.len() as u32 + 1;
        }
        unreachable!()
    };
    let mut expect: Vec<(u32, u32, u32)> = [("live", 11), ("live2", 13), ("dead3", 14)]
        .iter()
        .map(|&(s, id)| (djb(s), new_at(s), want.offsets[&id]))
        .collect();
    expect.sort_unstable();
    assert_eq!(got, expect);
    // The sections after the cut ones moved with them.
    assert!(o.assign_addresses().is_empty());
}

#[test]
fn nothing_dead_changes_nothing() {
    let t = unit();
    let mut ab = Vec::new();
    abbrevs(&t, &mut ab);
    let mut pool = Vec::new();
    strings(&t, &mut pool);
    let (full, strs) = emit(&t, &|_| true, &ab, &pool, &|_| true);
    let rels: Vec<Relocation> = full
        .relocs
        .iter()
        .rev()
        .map(|&p| Relocation::plain(p, 1, false, 0, 3))
        .collect();
    let mut o = object(vec![
        section(
            b"__TEXT",
            b"__text",
            S_ATTR_PURE_INSTRUCTIONS,
            vec![0; 0x40],
            Vec::new(),
        ),
        section(
            b"__DWARF",
            b"__debug_abbrev",
            S_ATTR_DEBUG,
            abbrev_bytes(&ab),
            Vec::new(),
        ),
        section(
            b"__DWARF",
            b"__debug_info",
            S_ATTR_DEBUG,
            full.info.clone(),
            rels,
        ),
        section(b"__DWARF", b"__debug_str", S_ATTR_DEBUG, strs, Vec::new()),
    ]);
    // (No place named lost code: nothing is dead.)
    assert_eq!(prune(&mut o, &BTreeSet::new(), &BTreeSet::new()), Ok(None));
    assert_eq!(bytes_of(&o, b"__debug_info"), full.info.as_slice());
}

#[test]
fn shifts_map_offsets_past_removed_ranges() {
    let s = Shift::new(vec![4..8, 10..12]);
    assert_eq!(s.map(0), 0);
    assert_eq!(s.map(4), 4);
    assert_eq!(s.map(7), 4);
    assert_eq!(s.map(8), 4);
    assert_eq!(s.map(9), 5);
    assert_eq!(s.map(11), 6);
    assert_eq!(s.map(12), 6);
    assert_eq!(s.map(20), 14);

    assert_eq!(s.total(), 6);
}

#[test]
fn bucket_counts_are_llvms() {
    assert_eq!(bucket_count(0), 1);
    assert_eq!(bucket_count(16), 16);
    assert_eq!(bucket_count(17), 8);
    assert_eq!(bucket_count(1024), 512);
    assert_eq!(bucket_count(1025), 256);
}

#[test]
fn finds_nuls_eight_bytes_at_a_time() {
    for n in 0..40 {
        let mut b = vec![b'x'; 40];
        b[n] = 0;
        assert_eq!(nul(&b), Some(n));
        // (A byte of 0x80 or 0x01 around it isn't one.)
        b.insert(0, 0x80);
        b.insert(0, 0x01);
        assert_eq!(nul(&b), Some(n + 2));
    }
    assert_eq!(nul(&[b'x'; 17]), None);
}

/// What a giver's DWARF defines in every link that loads it, by dsymutil's
/// rules (see `reach`): classes live code names, what they name, what a
/// using declaration or a constant names, and a class a member function's
/// definition is of (whole); not what only dead code names, nothing names,
/// or only a `using namespace` reaches, nor a declaration.
#[test]
fn reach_follows_dsymutils_rules() {
    use V::*;
    let st = |id: usize, name: &'static str, kids: Vec<T>| {
        die(
            id,
            DW_TAG_STRUCTURE_TYPE,
            vec![
                (DW_AT_NAME, Str(name)),
                (DW_AT_BYTE_SIZE, D1(1)),
                (DW_AT_DECL_LINE, D1(id as u8)),
            ],
            kids,
        )
    };
    let member = |id: usize, ty: usize| {
        die(
            id,
            DW_TAG_MEMBER,
            vec![(DW_AT_NAME, Str("m")), (DW_AT_TYPE, Ref(ty))],
            vec![],
        )
    };
    let sub = |id: usize, at: V, more: Vec<(u16, V)>| {
        let mut a = vec![(DW_AT_LOW_PC, at), (DW_AT_HIGH_PC, D4(4))];
        a.extend(more);
        die(id, DW_TAG_SUBPROGRAM, a, vec![])
    };
    let t = die(
        0,
        DW_TAG_COMPILE_UNIT,
        vec![(DW_AT_PRODUCER, Str("clang"))],
        vec![
            st(1, "A", vec![member(30, 2)]),
            st(2, "B", vec![]),
            st(3, "C", vec![]),
            st(4, "D", vec![]),
            st(5, "E", vec![]),
            die(
                6,
                DW_TAG_NAMESPACE,
                vec![(DW_AT_NAME, Str("ns"))],
                vec![
                    st(7, "F", vec![]),
                    st(
                        8,
                        "I",
                        vec![
                            die(
                                31,
                                DW_TAG_SUBPROGRAM,
                                vec![(DW_AT_NAME, Str("f")), (DW_AT_DECLARATION, D1(1))],
                                vec![],
                            ),
                            member(33, 9),
                        ],
                    ),
                    st(9, "J", vec![]),
                ],
            ),
            die(10, DW_TAG_POINTER_TYPE, vec![(DW_AT_TYPE, Ref(1))], vec![]),
            sub(11, Addr(0), vec![(DW_AT_TYPE, Ref(10))]),
            sub(12, Gone, vec![(DW_AT_TYPE, Ref(3))]),
            die(
                13,
                DW_TAG_IMPORTED_DECLARATION,
                vec![(DW_AT_IMPORT, Ref(5))],
                vec![],
            ),
            die(14, DW_TAG_IMPORTED_MODULE, vec![(DW_AT_IMPORT, Ref(6))], vec![]),
            sub(15, Addr(0x10), vec![(DW_AT_SPECIFICATION, Ref(31))]),
            die(
                16,
                DW_TAG_VARIABLE,
                vec![
                    (DW_AT_NAME, Str("k")),
                    (DW_AT_CONST_VALUE, D1(0)),
                    (DW_AT_TYPE, Ref(17)),
                ],
                vec![],
            ),
            st(17, "G", vec![]),
            die(
                18,
                DW_TAG_STRUCTURE_TYPE,
                vec![(DW_AT_NAME, Str("K")), (DW_AT_DECLARATION, D1(1))],
                vec![],
            ),
            sub(19, Addr(0x20), vec![(DW_AT_TYPE, Ref(18))]),
        ],
    );
    let mut ab = Vec::new();
    abbrevs(&t, &mut ab);
    let mut pool = Vec::new();
    strings(&t, &mut pool);
    let (full, strs) = emit(&t, &|_| true, &ab, &pool, &|_| true);
    let rels: Vec<Relocation> = full
        .relocs
        .iter()
        .rev()
        .map(|&p| Relocation::plain(p, 1, false, 0, 3))
        .collect();
    let o = object(vec![
        section(
            b"__TEXT",
            b"__text",
            S_ATTR_PURE_INSTRUCTIONS,
            vec![0; 0x40],
            Vec::new(),
        ),
        section(
            b"__DWARF",
            b"__debug_abbrev",
            S_ATTR_DEBUG,
            abbrev_bytes(&ab),
            Vec::new(),
        ),
        section(b"__DWARF", b"__debug_info", S_ATTR_DEBUG, full.info, rels),
        section(b"__DWARF", b"__debug_str", S_ATTR_DEBUG, strs, Vec::new()),
    ]);
    let got = super::super::reach::reach(&o, &BTreeSet::new())
        .unwrap()
        .unwrap();
    let k = |parent: u128, name: &str, line: u64| {
        crate::dwarf::key::of(parent, DW_TAG_STRUCTURE_TYPE, name.as_bytes(), Some(1), 0, line)
    };
    let root = crate::dwarf::key::ROOT;
    let ns = crate::dwarf::key::of(root, DW_TAG_NAMESPACE, b"ns", None, 0, 0);
    let want: turbonop_hash::Set<u128> = [
        k(root, "A", 1),
        k(root, "B", 2),
        k(root, "E", 5),
        k(ns, "I", 8),
        k(ns, "J", 9),
        k(root, "G", 17),
    ]
    .into_iter()
    .collect();
    assert_eq!(got, want);
}

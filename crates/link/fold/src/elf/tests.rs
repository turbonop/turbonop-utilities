use super::*;
use turbonop_elf_object::{Relocation, SHN_COMMON};

fn sec<'a>(name: &'a [u8], kind: u32, flags: u64, data: Data<'a>) -> Section<'a> {
    Section {
        name: Cow::Borrowed(name),
        name_offset: 0,
        kind,
        flags,
        addr: 0,
        link: 0,
        info: 0,
        addralign: 1,
        entsize: 0,
        nobits_size: 0,
        data,
    }
}

fn sym(name: &'static [u8], info: u8, other: u8, shndx: u32) -> Symbol<'static> {
    Symbol {
        name: Cow::Borrowed(name),
        name_offset: 0,
        value: 0,
        size: 0,
        info,
        other,
        shndx,
        xindex: false,
    }
}

fn reloc(o: &Object<'_>, offset: u64, kind: u32, symbol: u32, addend: i64) -> Relocation {
    let mut r = Relocation {
        offset,
        info: u64::from(kind),
        addend,
    };
    o.set_reloc_symbol(&mut r, symbol);
    r
}

/// A CIE (zR, pc-relative sdata4) and an FDE for each of `n` functions.
fn eh_frame(n: usize) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&0u32.to_le_bytes());
    v.extend_from_slice(&[1, b'z', b'R', 0, 1, 0x78, 16, 1, 0x1B, 0x0C, 7, 8]);
    for _ in 0..n {
        let at = v.len();
        v.extend_from_slice(&20u32.to_le_bytes());
        v.extend_from_slice(&((at + 4) as u32).to_le_bytes());
        // pc_begin, pc_range, augmentation length, instructions.
        v.extend_from_slice(&[0, 0, 0, 0, 6, 0, 0, 0, 0, 0x41, 0x0E, 0x10, 0, 0, 0, 0]);
    }
    v
}

/// A member: `own` (a global function in `.text`) calling the inline
/// function `_Z1fv` (in a COMDAT group, calling `ext`), each with an FDE,
/// and debug information naming both through section symbols. `other`
/// gives `_Z1fv` other bytes (another definition); `side_effects` adds an
/// `.init_array`.
fn member(own: &'static [u8], other: bool, side_effects: bool) -> Vec<u8> {
    let mut ident = [0u8; 16];
    ident[..4].copy_from_slice(b"\x7fELF");
    ident[4] = 2;
    ident[5] = 1;
    ident[6] = 1;
    let header = Header {
        ident,
        kind: 1,
        machine: 62,
        version: 1,
        entry: 0,
        flags: 0,
    };
    let f_bytes: &'static [u8] = if other {
        b"\xe8\0\0\0\0\x90\xc3"
    } else {
        b"\xe8\0\0\0\0\xc3"
    };
    let text_flags = shf::ALLOC | shf::EXECINSTR;
    let mut sections = vec![
        sec(b"", 0, 0, Data::NoBits),
        sec(
            b".text",
            sht::PROGBITS,
            text_flags,
            Data::Bytes(Cow::Borrowed(b"\xe8\0\0\0\0\xc3")),
        ),
        sec(
            b".rela.text",
            sht::RELA,
            shf::INFO_LINK,
            Data::Relocations(Vec::new()),
        ),
        sec(
            b".group",
            sht::GROUP,
            0,
            Data::Group(Group {
                flags: 1,
                members: vec![4, 5],
            }),
        ),
        sec(
            b".text._Z1fv",
            sht::PROGBITS,
            text_flags | shf::GROUP,
            Data::Bytes(Cow::Borrowed(f_bytes)),
        ),
        sec(
            b".rela.text._Z1fv",
            sht::RELA,
            shf::INFO_LINK | shf::GROUP,
            Data::Relocations(Vec::new()),
        ),
        sec(
            b".eh_frame",
            sht::X86_64_UNWIND,
            shf::ALLOC,
            Data::Bytes(Cow::Owned(eh_frame(2))),
        ),
        sec(
            b".rela.eh_frame",
            sht::RELA,
            shf::INFO_LINK,
            Data::Relocations(Vec::new()),
        ),
        sec(
            b".debug_info",
            sht::PROGBITS,
            0,
            Data::Bytes(Cow::Owned(vec![0xAA; 32])),
        ),
        sec(
            b".rela.debug_info",
            sht::RELA,
            shf::INFO_LINK,
            Data::Relocations(Vec::new()),
        ),
        sec(
            b".note.GNU-stack",
            sht::PROGBITS,
            0,
            Data::Bytes(Cow::Borrowed(b"")),
        ),
        sec(b".symtab", sht::SYMTAB, 0, Data::Symbols(Vec::new())),
        sec(
            b".strtab",
            sht::STRTAB,
            0,
            Data::Bytes(Cow::Borrowed(b"\0")),
        ),
        sec(
            b".shstrtab",
            sht::STRTAB,
            0,
            Data::Bytes(Cow::Borrowed(b"\0")),
        ),
    ];
    if side_effects {
        sections.insert(
            11,
            sec(
                b".init_array",
                sht::INIT_ARRAY,
                shf::ALLOC | shf::WRITE,
                Data::Bytes(Cow::Borrowed(&[0; 8])),
            ),
        );
    }
    let symtab = sections.iter().position(|s| s.kind == sht::SYMTAB).unwrap() as u32;
    for s in &mut sections {
        if s.is_relocations() || s.kind == sht::GROUP {
            s.link = symtab;
            s.entsize = if s.kind == sht::GROUP { 4 } else { 24 };
            s.addralign = 8;
        }
    }
    sections[2].info = 1;
    // The group's signature: symbol 4, _Z1fv.
    sections[3].info = 4;
    sections[5].info = 4;
    sections[7].info = 6;
    sections[9].info = 8;
    let t = symtab as usize;
    sections[t].link = symtab + 1;
    sections[t].entsize = 24;
    sections[t].info = 4;
    sections[t].data = Data::Symbols(vec![
        sym(b"", 0, 0, 0),
        sym(b"", 3, 0, 1),
        sym(b"", 3, 0, 4),
        sym(b"", 3, 0, 6),
        Symbol {
            size: f_bytes.len() as u64,
            ..sym(b"_Z1fv", (2 << 4) | 2, 2, 4)
        },
        sym(own, (1 << 4) | 2, 0, 1),
        sym(b"ext", 1 << 4, 0, SHN_UNDEF),
        Symbol {
            size: 4,
            value: 4,
            ..sym(b"common_thing", (1 << 4) | 1, 0, SHN_COMMON)
        },
    ]);
    let mut o = Object::new(header, sections);
    o.shstrndx = o.sections.len() as u32 - 1;
    // R_X86_64_PLT32 (4), R_X86_64_PC32 (2), R_X86_64_64 (1).
    let to_f = reloc(&o, 1, 4, 4, -4);
    let to_ext = reloc(&o, 1, 4, 6, -4);
    let fde1 = reloc(&o, 20 + 8, 2, 1, 0);
    let fde2 = reloc(&o, 20 + 24 + 8, 2, 2, 0);
    let d1 = reloc(&o, 0, 1, 1, 0);
    let d2 = reloc(&o, 8, 1, 2, 0);
    let d3 = reloc(&o, 16, 1, 4, 0);
    o.sections[2].data = Data::Relocations(vec![to_f]);
    o.sections[5].data = Data::Relocations(vec![to_ext]);
    o.sections[7].data = Data::Relocations(vec![fde1, fde2]);
    o.sections[9].data = Data::Relocations(vec![d1, d2, d3]);
    o.write().unwrap()
}

fn fold_all(members: &[Vec<u8>], pull: Pull) -> (Vec<Output>, Report) {
    let names: Vec<Vec<u8>> = (0..members.len())
        .map(|i| format!("m{i}.o").into_bytes())
        .collect();
    let inputs: Vec<Input<'_>> = names
        .iter()
        .zip(members)
        .map(|(n, d)| Input { name: n, data: d })
        .collect();
    fold(&inputs, &Options { pull })
}

fn relocations_for<'o>(o: &'o Object<'_>, section: usize) -> &'o [Relocation] {
    let rela = o
        .sections
        .iter()
        .find(|s| s.is_relocations() && s.info as usize == section)
        .unwrap();
    match &rela.data {
        Data::Relocations(rs) => rs,
        _ => panic!("relocations"),
    }
}

fn bytes_of<'o>(o: &'o Object<'_>, name: &[u8]) -> (usize, &'o [u8]) {
    let i = o.sections.iter().position(|s| &*s.name == name).unwrap();
    match &o.sections[i].data {
        Data::Bytes(b) => (i, b),
        _ => panic!("bytes"),
    }
}

fn anchor_of(o: &Object<'_>) -> Vec<u8> {
    o.symbols()
        .iter()
        .find(|s| s.name.starts_with(b"__turbolib.anchor."))
        .unwrap()
        .name
        .to_vec()
}

#[test]
fn identical_copies_fold() {
    let members = vec![member(b"a", false, false), member(b"b", false, false)];
    let (out, report) = fold_all(&members, Pull::Preserve);
    assert_eq!(
        (
            report.units,
            report.duplicated,
            report.folded,
            report.copies
        ),
        (2, 1, 1, 2)
    );
    assert_eq!(report.synthesized, 1);
    assert_eq!(report.tombstoned, 2);
    // (Its .debug_info is no DWARF: nothing describes the unit.)
    assert_eq!((report.debug_moved, report.debug_dropped), (0, 1));
    assert_eq!(out.len(), 3);
    let synth = Object::parse(out[2].data.as_ref().unwrap()).unwrap();
    assert_eq!(out[2].name, b"__turbolib.0.o");
    let anchor = anchor_of(&synth);
    for (i, o) in out.iter().take(2).enumerate() {
        let bytes = o.data.as_ref().unwrap();
        let obj = Object::parse(bytes).unwrap();
        // No group, no function's section; _Z1fv a weak reference now
        // (the anchor loads the code).
        assert!(
            obj.sections
                .iter()
                .all(|s| s.kind != sht::GROUP && &*s.name != b".text._Z1fv")
        );
        let f = obj.symbols().iter().find(|s| &*s.name == b"_Z1fv").unwrap();
        assert_eq!((f.shndx, f.bind(), f.visibility()), (SHN_UNDEF, 2, 2));
        // The anchor: a strong reference, the first global.
        let t = obj.symtab().unwrap();
        let a = &obj.symbols()[obj.sections[t].info as usize];
        assert!(a.name.starts_with(b"__turbolib.anchor."));
        assert_eq!((a.shndx, a.bind(), a.visibility()), (SHN_UNDEF, 1, 2));
        assert_eq!(anchor_of(&obj), anchor, "member {i}");
        // One FDE left, for .text; its relocation moved with it.
        let (eh, b) = bytes_of(&obj, b".eh_frame");
        assert_eq!(eh_frame::records(b, false).unwrap().len(), 2);
        let rs = relocations_for(&obj, eh);
        assert_eq!(rs.len(), 1);
        assert_eq!(rs[0].offset, 28);
        // Debug: the reference to the function's section tombstoned (its
        // bytes 0), the ones to .text and to the global kept.
        let (di, d) = bytes_of(&obj, b".debug_info");
        assert_eq!(&d[8..16], &[0; 8]);
        assert_eq!(&d[0..8], &[0xAA; 8]);
        let offsets: Vec<u64> = relocations_for(&obj, di).iter().map(|r| r.offset).collect();
        assert_eq!(offsets, [0, 16]);
        // The symbol table is consistent: locals first.
        let t = obj.symtab().unwrap();
        let first_global = obj.sections[t].info as usize;
        assert!(obj.symbols()[..first_global].iter().all(|s| s.bind() == 0));
        assert!(obj.symbols()[first_global..].iter().all(|s| s.bind() != 0));
        assert!(turbonop_elf::ElfFile::parse(bytes).is_ok());
    }
    // The synthesized member: the group, the function, its FDE and CIE,
    // its relocation to `ext`, and the anchor.
    let g = synth
        .sections
        .iter()
        .find(|s| s.kind == sht::GROUP)
        .unwrap();
    let Data::Group(grp) = &g.data else {
        panic!("a group")
    };
    assert_eq!(grp.members.len(), 2);
    assert_eq!(&*synth.symbols()[g.info as usize].name, b"_Z1fv");
    let f = synth
        .symbols()
        .iter()
        .find(|s| &*s.name == b"_Z1fv")
        .unwrap();
    assert_eq!((f.bind(), f.kind(), f.visibility(), f.size), (2, 2, 2, 6));
    assert!(f.in_section());
    let a = synth
        .symbols()
        .iter()
        .find(|s| s.name.starts_with(b"__turbolib.anchor."))
        .unwrap();
    assert_eq!((a.bind(), a.visibility(), a.shndx), (2, 2, SHN_ABS));
    let ext = synth
        .symbols()
        .iter()
        .position(|s| &*s.name == b"ext")
        .unwrap();
    let (text, _) = bytes_of(&synth, b".text._Z1fv");
    assert_eq!(
        synth.reloc_symbol(&relocations_for(&synth, text)[0]) as usize,
        ext
    );
    let (eh, b) = bytes_of(&synth, b".eh_frame");
    assert_eq!(eh_frame::records(b, false).unwrap().len(), 2);
    let rs = relocations_for(&synth, eh);
    assert_eq!(rs.len(), 1);
    let target = &synth.symbols()[synth.reloc_symbol(&rs[0]) as usize];
    assert_eq!((target.kind(), target.shndx as usize), (3, text));
    assert!(
        synth
            .sections
            .iter()
            .any(|s| &*s.name == b".note.GNU-stack")
    );
    // Deterministic: the same bytes again.
    let (again, _) = fold_all(&members, Pull::Preserve);
    for (x, y) in out.iter().zip(&again) {
        assert_eq!(x.data, y.data);
    }
}

#[test]
fn what_stops_folding() {
    // Another definition of the name.
    let three = [
        member(b"a", false, false),
        member(b"b", true, false),
        member(b"c", false, false),
    ];
    let (_, r) = fold_all(&three, Pull::Preserve);
    assert_eq!(r.folded, 0);
    assert_eq!(
        r.kept
            .get("its names have other definitions in the library"),
        Some(&2)
    );
    // The first definer has side effects: preserve keeps it, relaxed folds.
    let members = [member(b"a", false, true), member(b"b", false, false)];
    let (_, r) = fold_all(&members, Pull::Preserve);
    assert_eq!(r.folded, 0);
    assert_eq!(
        r.kept
            .get("the member the index sends its names to has side effects"),
        Some(&2)
    );
    let (out, r) = fold_all(&members, Pull::Relaxed);
    assert_eq!(r.folded, 1);
    assert_eq!(out.len(), 3);
    // One copy: nothing to fold.
    let (out, r) = fold_all(&[member(b"a", false, false)], Pull::Preserve);
    assert_eq!((r.folded, out.len()), (0, 1));
    assert!(out[0].data.is_none());
    // A member nothing here can read: nothing folds.
    let members = vec![
        member(b"a", false, false),
        member(b"b", false, false),
        b"BC\xC0\xDE...".to_vec(),
    ];
    let (out, r) = fold_all(&members, Pull::Preserve);
    assert_eq!(r.folded, 0);
    assert!(out.iter().all(|o| o.data.is_none()));
}

#[test]
fn helpers() {
    let mut v = Vec::new();
    for x in [0u64, 1, 127, 128, 300, u64::from(u32::MAX)] {
        v.clear();
        put_uleb(&mut v, x);
        assert_eq!(uleb(&v), Some((x, v.len())));
    }
    assert!(is_c_identifier(b"my_section"));
    assert!(!is_c_identifier(b".text"));
    assert!(!is_c_identifier(b"9lives"));
    let mut b = [0xFFu8; 8];
    put(&mut b, 0, 4, 1, Endian::Little);
    assert_eq!(b, [1, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]);
    put(&mut b, 0, 8, 0x0102, Endian::Big);
    assert_eq!(b, [0, 0, 0, 0, 0, 0, 1, 2]);
    assert_eq!(tombstone(b".debug_ranges"), 1);
    assert_eq!(tombstone(b".debug_info"), 0);
}

/// A member with the inline function `_Z1gv` and, if `with_f`, `_Z1fv`,
/// which calls it; neither with FDEs or debug information.
fn member_fg(with_f: bool, g: &'static [u8]) -> Vec<u8> {
    let mut ident = [0u8; 16];
    ident[..4].copy_from_slice(b"\x7fELF");
    ident[4] = 2;
    ident[5] = 1;
    ident[6] = 1;
    let header = Header {
        ident,
        kind: 1,
        machine: 62,
        version: 1,
        entry: 0,
        flags: 0,
    };
    let text = shf::ALLOC | shf::EXECINSTR | shf::GROUP;
    let mut sections = vec![
        sec(b"", 0, 0, Data::NoBits),
        sec(
            b".group",
            sht::GROUP,
            0,
            Data::Group(Group {
                flags: 1,
                members: vec![2],
            }),
        ),
        sec(
            b".text._Z1gv",
            sht::PROGBITS,
            text,
            Data::Bytes(Cow::Borrowed(g)),
        ),
    ];
    if with_f {
        sections.push(sec(
            b".group",
            sht::GROUP,
            0,
            Data::Group(Group {
                flags: 1,
                members: vec![4, 5],
            }),
        ));
        sections.push(sec(
            b".text._Z1fv",
            sht::PROGBITS,
            text,
            Data::Bytes(Cow::Borrowed(b"\xe8\0\0\0\0\xc3")),
        ));
        sections.push(sec(
            b".rela.text._Z1fv",
            sht::RELA,
            shf::INFO_LINK | shf::GROUP,
            Data::Relocations(Vec::new()),
        ));
    }
    sections.push(sec(
        b".note.GNU-stack",
        sht::PROGBITS,
        0,
        Data::Bytes(Cow::Borrowed(b"")),
    ));
    let symtab = sections.len() as u32;
    sections.push(sec(b".symtab", sht::SYMTAB, 0, Data::Symbols(Vec::new())));
    sections.push(sec(
        b".strtab",
        sht::STRTAB,
        0,
        Data::Bytes(Cow::Borrowed(b"\0")),
    ));
    sections.push(sec(
        b".shstrtab",
        sht::STRTAB,
        0,
        Data::Bytes(Cow::Borrowed(b"\0")),
    ));
    let mut symbols = vec![sym(b"", 0, 0, 0), sym(b"_Z1gv", (2 << 4) | 2, 0, 2)];
    sections[1].info = 1;
    if with_f {
        symbols.push(sym(b"_Z1fv", (2 << 4) | 2, 0, 4));
        sections[3].info = 2;
        sections[5].info = 4;
    }
    for s in &mut sections {
        if s.is_relocations() || s.kind == sht::GROUP {
            s.link = symtab;
            s.entsize = if s.kind == sht::GROUP { 4 } else { 24 };
        }
    }
    let t = symtab as usize;
    sections[t].link = symtab + 1;
    sections[t].entsize = 24;
    sections[t].info = 1;
    sections[t].data = Data::Symbols(symbols);
    let mut o = Object::new(header, sections);
    o.shstrndx = o.sections.len() as u32 - 1;
    if with_f {
        let call = reloc(&o, 1, 4, 1, -4);
        o.sections[5].data = Data::Relocations(vec![call]);
    }
    o.write().unwrap()
}

#[test]
fn synthesized_members_refer_to_each_other_through_anchors() {
    // f's copies are in m0 and m1; g's in all three: two synthesized
    // members, and f's refers to g weakly, with g's member's anchor.
    let members = vec![
        member_fg(true, b"\xc3"),
        member_fg(true, b"\xc3"),
        member_fg(false, b"\xc3"),
    ];
    let (out, r) = fold_all(&members, Pull::Preserve);
    assert_eq!((r.folded, r.synthesized), (2, 2));
    let s: Vec<Object<'_>> = out[3..]
        .iter()
        .map(|o| Object::parse(o.data.as_ref().unwrap()).unwrap())
        .collect();
    let defines =
        |o: &Object<'_>, n: &[u8]| o.symbols().iter().any(|x| &*x.name == n && x.in_section());
    let (with_f, with_g) = if defines(&s[0], b"_Z1fv") {
        (&s[0], &s[1])
    } else {
        (&s[1], &s[0])
    };
    assert!(defines(with_g, b"_Z1gv"));
    let g = with_f
        .symbols()
        .iter()
        .find(|x| &*x.name == b"_Z1gv")
        .unwrap();
    assert_eq!((g.shndx, g.bind()), (SHN_UNDEF, 2));
    let t = with_f.symtab().unwrap();
    let first = &with_f.symbols()[with_f.sections[t].info as usize];
    assert_eq!(first.name, anchor_of(with_g));
    assert_eq!((first.shndx, first.bind()), (SHN_UNDEF, 1));
    // m2 gave only g: one anchor; m0 two.
    let anchors = |o: &Output| {
        Object::parse(o.data.as_ref().unwrap())
            .unwrap()
            .symbols()
            .iter()
            .filter(|x| x.name.starts_with(b"__turbolib.anchor."))
            .count()
    };
    assert_eq!((anchors(&out[0]), anchors(&out[2])), (2, 1));
}

#[test]
fn a_unit_calling_what_stays_stays() {
    // g's copies differ (m2's), so g stays everywhere; f calls g, which
    // f's givers define themselves: f stays too.
    let members = vec![
        member_fg(true, b"\xc3"),
        member_fg(true, b"\xc3"),
        member_fg(false, b"\x90\xc3"),
    ];
    let (out, r) = fold_all(&members, Pull::Preserve);
    assert_eq!(r.folded, 0);
    assert_eq!(
        r.kept.get("it calls a name of its member that stays"),
        Some(&2)
    );
    assert!(out.iter().all(|o| o.data.is_none()));
}

/// `member`, with an initializer entry in `_Z1fv`'s group (as clang writes
/// an inline variable's): an `.init_array` section `name` of `kind`.
fn member_with_grouped_initializer(own: &'static [u8], name: &'static [u8], kind: u32) -> Vec<u8> {
    let bytes = member(own, false, false);
    let mut o = Object::parse(&bytes).unwrap();
    let at = o.sections.len() as u32;
    o.sections.push(Section {
        addralign: 8,
        ..sec(
            name,
            kind,
            shf::ALLOC | shf::WRITE | shf::GROUP,
            Data::Bytes(Cow::Borrowed(&[0; 8])),
        )
    });
    let group = o
        .sections
        .iter()
        .position(|s| s.kind == sht::GROUP)
        .unwrap();
    if let Data::Group(g) = &mut o.sections[group].data {
        g.members.push(at);
    }
    o.write().unwrap()
}

#[test]
fn initializer_entries_in_a_group_stay() {
    for (name, kind) in [
        (&b".init_array"[..], sht::INIT_ARRAY),
        (b".init_array.00200", sht::INIT_ARRAY),
        (b".fini_array", sht::FINI_ARRAY),
        (b".preinit_array", sht::PREINIT_ARRAY),
        (b".ctors", sht::PROGBITS),
        (b".dtors.65435", sht::PROGBITS),
        // By type alone.
        (b".data.rel.ro.x", sht::INIT_ARRAY),
    ] {
        let members = [
            member_with_grouped_initializer(b"a", name, kind),
            member_with_grouped_initializer(b"b", name, kind),
        ];
        for pull in [Pull::Preserve, Pull::Relaxed] {
            let (out, r) = fold_all(&members, pull);
            assert_eq!((r.duplicated, r.folded), (1, 0), "{name:?}");
            assert_eq!(
                r.kept
                    .get("it holds initializer, terminator, or warning sections"),
                Some(&2)
            );
            assert!(out.iter().all(|o| o.data.is_none()));
        }
    }
}

#[test]
fn an_initializer_entry_in_a_group_is_a_side_effect() {
    // m0 is _Z1fv's first definer and holds a grouped initializer in
    // another unit (`_Z1gv`'s): under preserve, `_Z1fv` stays, since a link
    // that loaded m0 for it ran that initializer.
    let bytes = member(b"a", false, false);
    let mut o = Object::parse(&bytes).unwrap();
    let n = o.sections.len() as u32;
    let group = Section {
        link: o.symtab().unwrap() as u32,
        info: 6, // the signature: `ext`, never mind.
        entsize: 4,
        addralign: 4,
        ..sec(
            b".group",
            sht::GROUP,
            0,
            Data::Group(Group {
                flags: 1,
                members: vec![n + 1],
            }),
        )
    };
    o.sections.push(group);
    o.sections.push(Section {
        addralign: 8,
        ..sec(
            b".init_array",
            sht::INIT_ARRAY,
            shf::ALLOC | shf::WRITE | shf::GROUP,
            Data::Bytes(Cow::Borrowed(&[0; 8])),
        )
    });
    let m0 = o.write().unwrap();
    let members = [m0, member(b"b", false, false)];
    let (_, r) = fold_all(&members, Pull::Preserve);
    assert_eq!(r.folded, 0);
    assert_eq!(
        r.kept
            .get("the member the index sends its names to has side effects"),
        Some(&2)
    );
    let (_, r) = fold_all(&members, Pull::Relaxed);
    assert_eq!(r.folded, 1);
}

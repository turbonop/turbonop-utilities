use super::*;
use turbonop_macho_object::Endian;
use turbonop_macho_object::consts::{
    CPU_TYPE_ARM64, MH_MAGIC_64, S_ATTR_PURE_INSTRUCTIONS, S_ATTR_SOME_INSTRUCTIONS,
};

const RET: [u8; 4] = [0xC0, 0x03, 0x5F, 0xD6];
const BL: [u8; 4] = [0x00, 0x00, 0x00, 0x94];

fn sym(name: &str, n_type: u8, n_sect: u8, n_desc: u16, n_value: u64) -> Symbol<'static> {
    Symbol {
        name: Cow::Owned(name.as_bytes().to_vec()),
        name_offset: 0,
        n_type,
        n_sect,
        n_desc,
        n_value,
        indirect: None,
    }
}

/// An arm64 object: `strong` (calling `_inl`), then the weak `_inl`
/// (`inl` bytes), and an unwind entry for each.
fn object(strong: &str, inl: &[u8]) -> Vec<u8> {
    let mut text = Vec::new();
    text.extend_from_slice(&BL);
    text.extend_from_slice(&RET);
    let inl_at = text.len() as u64;
    text.extend_from_slice(inl);
    let size = text.len() as u64;
    let mut cu = Vec::new();
    for (start, len) in [(0u64, inl_at), (inl_at, size - inl_at)] {
        cu.extend_from_slice(&start.to_le_bytes());
        cu.extend_from_slice(&(len as u32).to_le_bytes());
        cu.extend_from_slice(&0x0400_0000u32.to_le_bytes());
        cu.extend_from_slice(&[0; 16]);
    }
    let text_sec = Section {
        sectname: name_field(b"__text"),
        segname: name_field(b"__TEXT"),
        addr: 0,
        align: 2,
        flags: S_ATTR_PURE_INSTRUCTIONS | S_ATTR_SOME_INSTRUCTIONS,
        reserved1: 0,
        reserved2: 0,
        reserved3: 0,
        contents: Contents::Bytes(Cow::Owned(text)),
        // bl _inl: symbol 1 (sorted: _inl after the strong name? set below).
        relocations: vec![Relocation {
            address: 0,
            symbolnum: 1,
            pcrel: true,
            length: 2,
            external: true,
            kind: reloc::arm64::BRANCH26,
            value: None,
        }],
    };
    let cu_sec = Section {
        sectname: name_field(b"__compact_unwind"),
        segname: name_field(b"__LD"),
        addr: 0,
        align: 3,
        flags: S_ATTR_DEBUG,
        reserved1: 0,
        reserved2: 0,
        reserved3: 0,
        contents: Contents::Bytes(Cow::Owned(cu)),
        relocations: vec![
            Relocation::plain(32, 1, false, 0, 3),
            Relocation::plain(0, 1, false, 0, 3),
        ],
    };
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
        sections: vec![text_sec, cu_sec],
    };
    let mut o = Object::new(
        header,
        vec![
            Command::Segment(seg),
            Command::Symtab,
            Command::Dysymtab(Default::default()),
        ],
    );
    o.symbols = vec![
        sym(strong, N_SECT | N_EXT, 1, 0, 0),
        sym("_inl", N_SECT | N_EXT | N_PEXT, 1, N_WEAK_DEF, inl_at),
    ];
    assert!(o.assign_addresses().is_empty());
    o.update_dysymtab();
    o.write().unwrap()
}

#[test]
fn folds_identical_weak_definitions() {
    let inl = [RET, RET].concat();
    let a = object("_a", &inl);
    let b = object("_b", &inl);
    let inputs = [
        Input {
            name: b"a.o",
            data: &a,
        },
        Input {
            name: b"b.o",
            data: &b,
        },
    ];
    let (out, report) = fold(
        &inputs,
        &Options {
            pull: Pull::Preserve,
            salt: b"libt.a",
            debug_types: DebugTypes::Complete,
        },
    );
    assert_eq!(report.folded, 1, "{report:?}");
    assert_eq!(report.copies, 2);
    assert_eq!(report.synthesized, 1);
    assert_eq!(out.len(), 3);
    // The synthesized member first, defining `_inl` and the anchor.
    assert_eq!(out[0].name, b"__turbolib.0.o");
    let s = Object::parse(out[0].data.as_deref().unwrap()).unwrap();
    let def = s.symbols.iter().find(|y| &*y.name == b"_inl").unwrap();
    assert!(def.in_section() && def.n_desc & N_WEAK_DEF != 0);
    let anchor = s
        .symbols
        .iter()
        .find(|y| y.name.starts_with(ANCHOR_PREFIX))
        .unwrap();
    assert_eq!(anchor.n_type, N_ABS | N_EXT | N_PEXT);
    assert_eq!(s.section(0).unwrap().size(), 8);
    // Its unwind entry, at its function.
    let cu = s
        .sections()
        .find(|x| x.name() == b"__compact_unwind")
        .unwrap();
    assert_eq!(cu.size(), 32);
    // Each giver: `_inl` a weak reference, the anchor its first undefined
    // name, and 8 bytes of code and one unwind entry fewer.
    for o in &out[1..] {
        let g = Object::parse(o.data.as_deref().unwrap()).unwrap();
        let undef: Vec<&Symbol<'_>> = g.symbols.iter().filter(|y| y.is_undefined()).collect();
        // The anchor, then the moved name.
        assert_eq!(undef.len(), 2);
        assert_eq!(&*undef[0].name, &*anchor.name);
        assert_eq!(&*undef[1].name, b"_inl");
        let inl = undef.iter().find(|y| &*y.name == b"_inl").unwrap();
        assert_eq!(inl.n_desc, N_WEAK_REF);
        assert_eq!(g.section(0).unwrap().size(), 8);
        let cu = g
            .sections()
            .find(|x| x.name() == b"__compact_unwind")
            .unwrap();
        assert_eq!(cu.size(), 32);
        // The call still names `_inl`.
        let r = g.section(0).unwrap().relocations[0];
        assert!(r.external);
        assert_eq!(&*g.symbols[r.symbolnum as usize].name, b"_inl");
    }
    // Deterministic.
    let (again, _) = fold(
        &inputs,
        &Options {
            pull: Pull::Preserve,
            salt: b"libt.a",
            debug_types: DebugTypes::Complete,
        },
    );
    assert!(out.iter().zip(&again).all(|(x, y)| x.data == y.data));
}

#[test]
fn different_copies_stay() {
    let a = object("_a", &[RET, RET].concat());
    let b = object("_b", &[RET, RET, RET].concat());
    let inputs = [
        Input {
            name: b"a.o",
            data: &a,
        },
        Input {
            name: b"b.o",
            data: &b,
        },
    ];
    let (out, report) = fold(
        &inputs,
        &Options {
            pull: Pull::Preserve,
            salt: b"",
            debug_types: DebugTypes::Complete,
        },
    );
    assert_eq!(report.folded, 0);
    assert!(out.iter().all(|o| o.data.is_none()));
    assert_eq!(
        report.kept.get("no copy in another member"),
        Some(&2),
        "{report:?}"
    );
}

/// The classes named in the DWARF of a synthesized member of the
/// `--debug-types` fixture (`tools/turbolib/macho-types`), each with
/// whether it is a declaration.
fn synthesized_classes(debug_types: DebugTypes) -> BTreeMap<String, bool> {
    use turbonop_dwarf::consts::{DW_AT_DECLARATION, DW_AT_NAME, DW_TAG_STRUCTURE_TYPE};
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../testdata/turbolib/macho-types");
    let a = std::fs::read(dir.join("a-arm64.o")).unwrap();
    let b = std::fs::read(dir.join("b-arm64.o")).unwrap();
    let inputs = [
        Input {
            name: b"a.o",
            data: &a,
        },
        Input {
            name: b"b.o",
            data: &b,
        },
    ];
    let (out, report) = fold(
        &inputs,
        &Options {
            pull: Pull::Preserve,
            salt: b"libtypes.a",
            debug_types,
        },
    );
    assert_eq!(report.synthesized, 1, "{report:?}");
    assert_eq!(report.debug_dropped, 0, "{report:?}");
    let s = Object::parse(out[0].data.as_deref().unwrap()).unwrap();
    let mut d = turbonop_dwarf::Sections::new(turbonop_core::Endian::Little);
    for x in s.sections() {
        if let Some(n) = x.name().strip_prefix(b"__debug_") {
            d.set(n, x.bytes().unwrap_or(&[]));
        }
    }
    let u = turbonop_dwarf::Unit::iter(d, turbonop_dwarf::UnitSection::Info, 0)
        .next()
        .unwrap();
    let abbrevs = turbonop_dwarf::Abbrevs::parse(d.abbrev, u.abbrev_offset, d.endian).unwrap();
    let mut classes = BTreeMap::new();
    let mut r = u.dies();
    let mut attrs = Vec::new();
    while let Some(ab) = r.next(&abbrevs) {
        r.attrs(&u, ab, &mut attrs).unwrap();
        if ab.tag != DW_TAG_STRUCTURE_TYPE {
            continue;
        }
        let name = attrs
            .iter()
            .find(|a| a.name == DW_AT_NAME)
            .and_then(|a| u.string(&a.value))
            .map(|s| String::from_utf8_lossy(s.bytes).into_owned())
            .unwrap();
        let decl = attrs.iter().any(|a| a.name == DW_AT_DECLARATION);
        classes.insert(name, decl);
    }
    classes
}

/// `--debug-types=complete` (the default): the classes the moved code names
/// are whole in the synthesized member.
#[test]
fn debug_types_complete_keeps_named_classes_whole() {
    let c = synthesized_classes(DebugTypes::Complete);
    assert_eq!(c.get("Point"), Some(&false), "{c:?}");
    assert_eq!(c.get("Hidden"), Some(&false), "{c:?}");
}

/// `--debug-types=shared`: a class every giver's own code defines (`Point`,
/// a's parameter, b's local) is a declaration; one only the moved code
/// names (`Hidden`) stays whole.
#[test]
fn debug_types_shared_declares_what_the_givers_define() {
    let c = synthesized_classes(DebugTypes::Shared);
    assert_eq!(c.get("Point"), Some(&true), "{c:?}");
    assert_eq!(c.get("Hidden"), Some(&false), "{c:?}");
}

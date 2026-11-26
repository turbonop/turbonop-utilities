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
        assert_eq!(&*undef[0].name, &*anchor.name);
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

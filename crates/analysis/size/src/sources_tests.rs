use super::*;
use crate::rangemap::RangeMap;
use crate::testimage::{Buf, SECTION_RVA, image};

/// (start, end, label) of a map.
fn entries<'a>(m: &RangeMap, labels: &'a Labels) -> Vec<(u64, u64, &'a str)> {
    m.iter()
        .map(|e| (e.start, e.end, labels.name(e.label)))
        .collect()
}

fn base_of(data: &[u8]) -> (DualMap, Labels) {
    let pe = PeFile::parse(data).unwrap();
    let mut labels = Labels::default();
    let base = pe_base(&pe, &mut labels);
    (base, labels)
}

#[test]
fn base_map_of_a_simple_image() {
    // One section of 0x30 bytes in a 0x200-byte raw block.
    let data = image(true, &[0xCC; 0x30], &[]);
    let (base, labels) = base_of(&data);
    // Headers: the section table (one entry) at 0x188, after the 0xF0-byte
    // optional header at 0x98.
    assert_eq!(
        entries(&base.file, &labels),
        [
            (0, 0x188, HEADERS),
            (0x188, 0x1B0, SECTION_HEADERS),
            (0x1B0, 0x200, HEADERS),
            (0x200, 0x400, ".data"),
        ]
    );
    // In memory: the headers at the image base, then the section's 0x200
    // bytes of virtual size (the helper's virtual size is the raw size).
    let vm = entries(&base.vm, &labels);
    assert_eq!(vm[0], (0, 0x188, HEADERS));
    assert_eq!(vm[3], (0x1000, 0x1200, ".data"));
    assert_eq!(base.vm.translate(0x1010), Some(0x210));
    assert_eq!(base.file.translate(0x210), Some(0x1010));
}

#[test]
fn zero_fill_padding_and_structures_outside_sections() {
    let mut data = image(true, &[1; 0x10], &[]);
    let pe_offset = 0x80usize;
    let section = pe_offset + 0x18 + 0xF0;
    // Virtual size 0x1800: 0x1600 bytes of zero fill past the raw 0x200.
    data[section + 8..section + 12].copy_from_slice(&0x1800u32.to_le_bytes());
    // A certificate table (by file offset) and an overlay after the image.
    let cert_dir = pe_offset + 0x18 + 112 + 4 * 8;
    data[cert_dir..cert_dir + 4].copy_from_slice(&0x400u32.to_le_bytes());
    data[cert_dir + 4..cert_dir + 8].copy_from_slice(&0x40u32.to_le_bytes());
    data.resize(0x480, 7);
    let (base, labels) = base_of(&data);
    let file = entries(&base.file, &labels);
    assert_eq!(
        &file[3..],
        [
            (0x200, 0x400, ".data"),
            (0x400, 0x440, CERTIFICATES),
            (0x440, 0x480, UNMAPPED),
        ]
    );
    assert_eq!(base.file.total(), 0x480);
    let vm = entries(&base.vm, &labels);
    assert_eq!(
        &vm[3..],
        [(0x1000, 0x1200, ".data"), (0x1200, 0x2800, ".data")]
    );
    assert_eq!(base.vm.translate(0x1300), None);
}

#[test]
fn hostile_section_headers() {
    let mut data = image(true, &[1; 0x10], &[]);
    let section = 0x80 + 0x18 + 0xF0;
    // Raw data claimed far past the end of the file, and a virtual size of
    // 0 (meaning the raw size).
    data[section + 8..section + 12].copy_from_slice(&0u32.to_le_bytes());
    data[section + 16..section + 20].copy_from_slice(&0xFFFF_0000u32.to_le_bytes());
    let (base, labels) = base_of(&data);
    assert_eq!(base.file.total(), data.len() as u64);
    // The raw data is clamped to the file.
    let file = entries(&base.file, &labels);
    assert_eq!(file.last(), Some(&(0x200, 0x400, ".data")));
    // A raw data pointer into the headers: the headers keep their bytes.
    let mut data = image(true, &[1; 0x10], &[]);
    data[section + 20..section + 24].copy_from_slice(&0x100u32.to_le_bytes());
    let (base, labels) = base_of(&data);
    assert_eq!(base.file.total(), data.len() as u64);
    assert_eq!(
        base.file.get(0x150).map(|e| labels.name(e.label)),
        Some(HEADERS)
    );
}

#[test]
fn coff_symbol_table() {
    let mut data = image(true, &[1; 0x10], &[]);
    // Two 18-byte symbols at 0x400, then a 12-byte string table.
    let mut b = Buf(std::mem::take(&mut data));
    b.u32(0x80 + 4 + 8, 0x400).u32(0x80 + 4 + 12, 2);
    b.u32(0x400 + 36, 12).bytes(0x400 + 36 + 4, b"abcdefgh");
    let data = b.0;
    let (base, labels) = base_of(&data);
    let file = entries(&base.file, &labels);
    assert_eq!(
        &file[file.len() - 2..],
        [(0x400, 0x424, COFF_SYMBOLS), (0x424, 0x430, COFF_STRINGS)]
    );
}

#[test]
fn sources_fall_back_to_the_base() {
    let data = image(true, &[0xCC; 0x30], &[]);
    let image = Image::parse(&data).unwrap();
    let symbols = [Symbol {
        addr: u64::from(SECTION_RVA) + 0x10,
        size: crate::Size::new(0x10, 0x10),
        kind: crate::symbols::SymbolKind::Function,
        folded: false,
        name: "f".into(),
        compiland: None,
        file_static: false,
    }];
    let inputs = Inputs {
        model: None,
        symbols: Some(&symbols),
        file_name: "a.dll",
        lines: None,
    };
    let maps = maps(
        &image,
        inputs,
        &[Source::Symbols, Source::InputFiles, Source::CompileUnits],
    );
    let (_, symbols_map) = &maps.sources[0];
    let file = entries(&symbols_map.file, &maps.labels);
    assert_eq!(
        file,
        [
            (0, 0x40, "[DOS Header]"),
            (0x40, 0x80, "[DOS Stub]"),
            (0x80, 0x98, "[PE File Header]"),
            (0x98, 0x108, "[PE Optional Header]"),
            (0x108, 0x188, "[PE Data Directories]"),
            (0x188, 0x1B0, SECTION_HEADERS),
            (0x1B0, 0x200, HEADERS),
            (0x200, 0x210, "[section .data]"),
            (0x210, 0x220, "f"),
            (0x220, 0x400, "[section .data]"),
        ]
    );
    let (_, input) = &maps.sources[1];
    // (Two entries: they map to memory that isn't contiguous.)
    assert_eq!(
        entries(&input.file, &maps.labels),
        [(0, 0x200, "a.dll"), (0x200, 0x400, "a.dll")]
    );
    // Without a model, compilands are only fallbacks.
    let (_, units) = &maps.sources[2];
    assert!(
        units
            .vm
            .iter()
            .all(|e| maps.labels.name(e.label).starts_with('['))
    );
}

#[test]
fn source_names() {
    for &s in Source::ALL {
        assert_eq!(Source::parse(s.name()), Some(s));
        assert!(!s.description().is_empty());
    }
    assert_eq!(Source::parse("armembers"), Some(Source::ArMembers));
    assert_eq!(Source::parse("compilands"), Some(Source::CompileUnits));
    assert_eq!(Source::parse("coffgroups"), Some(Source::Groups));
    assert_eq!(Source::parse("bogus"), None);
    assert!(Source::Symbols.needs_debug_info());
    assert!(!Source::Sections.needs_debug_info());
}

/// Data no symbol labels, which a function's code refers to (a
/// RIP-relative `lea`, and a table read as MSVC compiles it: from a
/// register holding `__ImageBase`), is the function's.
#[test]
fn data_the_code_refers_to_is_the_functions() {
    let mut contents = vec![0u8; 0x40];
    // 0x1000: lea rax, [rip + 0x19] (0x1020).
    contents[0..7].copy_from_slice(&[0x48, 0x8D, 0x05, 0x19, 0, 0, 0]);
    // 0x1008: lea r9, [rip - 0x100F] (the image's base, RVA 0).
    contents[8..15].copy_from_slice(&[0x4C, 0x8D, 0x0D, 0xF1, 0xEF, 0xFF, 0xFF]);
    // 0x100F: mov eax, [r9 + rcx*4 + 0x1030].
    contents[15..23].copy_from_slice(&[0x41, 0x8B, 0x84, 0x89, 0x30, 0x10, 0, 0]);
    // 0x1017: ret.
    contents[23] = 0xC3;
    let data = image(true, &contents, &[]);
    let image = Image::parse(&data).unwrap();
    let symbols = [Symbol {
        addr: u64::from(SECTION_RVA),
        size: crate::Size::new(0x18, 0x18),
        kind: crate::symbols::SymbolKind::Function,
        folded: false,
        name: "f".into(),
        compiland: None,
        file_static: false,
    }];
    let inputs = Inputs {
        model: None,
        symbols: Some(&symbols),
        file_name: "a.dll",
        lines: None,
    };
    let maps = maps(&image, inputs, &[Source::Symbols]);
    let vm = entries(&maps.sources[0].1.vm, &maps.labels);
    let label_at = |a: u64| vm.iter().find(|e| e.0 <= a && a < e.1).map(|e| e.2);
    assert_eq!(label_at(0x1000), Some("f"));
    assert_eq!(label_at(0x1018), Some("[section .data]"), "{vm:?}");
    assert_eq!(label_at(0x1020), Some("f"), "{vm:?}");
    assert_eq!(label_at(0x1030), Some("f"), "{vm:?}");
}

/// What a function's code refers to is the function's only within its own
/// object's contribution: never past that contribution's end, nor in
/// another object's (a constant folded with one of its, a table it
/// exports). What no symbol labels of a contribution is its object's.
#[test]
fn referenced_data_stays_in_its_objects_contribution() {
    let mut contents = vec![0u8; 0x40];
    // 0x1000: lea rax, [rip + 0x19] (0x1020).
    contents[0..7].copy_from_slice(&[0x48, 0x8D, 0x05, 0x19, 0, 0, 0]);
    // 0x1007: lea rax, [rip + 0x29] (0x1037).
    contents[7..14].copy_from_slice(&[0x48, 0x8D, 0x05, 0x29, 0, 0, 0]);
    // 0x100E: ret.
    contents[14] = 0xC3;
    let data = image(true, &contents, &[]);
    let image = Image::parse(&data).unwrap();
    let at = u64::from(SECTION_RVA);
    let symbols = [Symbol {
        addr: at,
        size: crate::Size::new(0x10, 0x10),
        kind: crate::symbols::SymbolKind::Function,
        folded: false,
        name: "f".into(),
        compiland: None,
        file_static: false,
    }];
    let piece = |start: u64, end: u64, compiland: u32| crate::Piece {
        range: crate::Range {
            addr: at + start,
            len: end - start,
            zero_fill: false,
        },
        compiland,
        section: 0,
        group: crate::NO_GROUP,
    };
    let model = Model {
        sections: vec![crate::Section {
            name: ".data".into(),
            addr: at,
            size: crate::Size::new(0x200, 0x200),
            vm_padding: 0,
            characteristics: 0,
            flags: turbonop_image::SectionFlags(
                turbonop_image::SectionFlags::READ | turbonop_image::SectionFlags::DATA,
            ),
        }],
        libraries: vec![crate::Library {
            name: String::new(),
        }],
        compilands: vec![
            crate::Compiland {
                name: r"C:\o\a.obj".into(),
                library: 0,
            },
            crate::Compiland {
                name: "/t/b-0123.b.4567-cgu.0.rcgu.o".into(),
                library: 0,
            },
        ],
        // f and its data (0x1020..0x1028), then b's (0x1028..0x1040).
        pieces: vec![
            piece(0, 0x10, 0),
            piece(0x20, 0x28, 0),
            piece(0x28, 0x40, 1),
        ],
        ..Model::default()
    };
    let inputs = Inputs {
        model: Some(&model),
        symbols: Some(&symbols),
        file_name: "a.dll",
        lines: None,
    };
    let maps = maps(&image, inputs, &[Source::Symbols]);
    let vm = entries(&maps.sources[0].1.vm, &maps.labels);
    let label_at = |a: u64| vm.iter().find(|e| e.0 <= a && a < e.1).map(|e| e.2);
    assert_eq!(label_at(0x1000), Some("f"));
    assert_eq!(label_at(0x1010), Some("[section .data]"), "{vm:?}");
    assert_eq!(label_at(0x1020), Some("f"), "{vm:?}");
    assert_eq!(label_at(0x1027), Some("f"), "{vm:?}");
    // Not f's: b's, though f refers to it, and up to its end.
    assert_eq!(label_at(0x1028), Some("[.data of b]"), "{vm:?}");
    assert_eq!(label_at(0x1037), Some("[.data of b]"), "{vm:?}");
    assert_eq!(label_at(0x103F), Some("[.data of b]"), "{vm:?}");
    assert_eq!(label_at(0x1040), Some("[section .data]"), "{vm:?}");
    // In the file (the section's bytes at 0x200), alike.
    let file = entries(&maps.sources[0].1.file, &maps.labels);
    let file_at = |a: u64| file.iter().find(|e| e.0 <= a && a < e.1).map(|e| e.2);
    assert_eq!(file_at(0x220), Some("f"), "{file:?}");
    assert_eq!(file_at(0x228), Some("[.data of b]"), "{file:?}");
    assert_eq!(file_at(0x23F), Some("[.data of b]"), "{file:?}");
    assert_eq!(file_at(0x240), Some("[section .data]"), "{file:?}");
    assert_eq!(file_at(0), Some("[DOS Header]"), "{file:?}");
}

#[test]
fn objects_and_their_crates() {
    assert_eq!(
        object_name(r"C:\t\turbonop_powerpc-932f.turbonop_powerpc.3598-cgu.0.rcgu.o"),
        "turbonop_powerpc"
    );
    assert_eq!(object_name("lib/foo.obj"), "foo.obj");
    assert_eq!(object_name("bar.o"), "bar.o");
    assert_eq!(
        crate_of("[.rdata of turbonop_powerpc]").as_deref(),
        Some("turbonop_powerpc")
    );
    assert_eq!(crate_of("[.rdata of foo.obj]"), None);
    assert_eq!(crate_of("[section .rdata]"), None);
}

/// MSVC's names of Rust's types are their insides' crates': an enum's
/// method (`enum2$<alloc::...>::f`), a reference, a tuple.
#[test]
fn msvc_wrapped_types_are_their_crates() {
    let c = |n: &str| crate_of(n);
    assert_eq!(
        c("enum2$<alloc::collections::btree::map::entry::Entry<u64,u64> >::or_insert<u64>(u64)")
            .as_deref(),
        Some("alloc")
    );
    assert_eq!(c("ref$<str$>").as_deref(), Some("[global]"));
    assert_eq!(
        c("tuple$<turbonop_size::Size,u64>::clone()").as_deref(),
        Some("turbonop_size")
    );
    // (Not a wrapper: a generic's argument.)
    assert_eq!(
        c("core::ptr::drop_in_place<enum2$<foo::X> >(ptr)").as_deref(),
        Some("core")
    );
}

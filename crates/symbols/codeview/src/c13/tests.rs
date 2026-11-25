use super::*;

fn words(w: &[u32]) -> Vec<u8> {
    w.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn fields_of(kind: u32, data: &[u8]) -> Result<Vec<Field>, Error> {
    let mut v = Vec::new();
    fields(kind, data, |f| v.push(f))?;
    Ok(v)
}

/// Reads `data` as `kind` and writes it back: the same bytes.
fn round_trip(kind: u32, data: &[u8]) -> Contents<'_> {
    let c = Contents::parse(kind, data).unwrap();
    let mut out = Vec::new();
    c.write(&mut out);
    assert_eq!(out, data, "{kind:#x}");
    c
}

#[test]
fn subsections_are_framed_and_padded() {
    let mut s = CV_SIGNATURE_C13.to_le_bytes().to_vec();
    push_subsection(&mut s, kind::DEBUG_S_STRINGTABLE, b"\0a.cpp\0");
    push_subsection(&mut s, kind::DEBUG_S_SYMBOLS, &[2, 0, 6, 0]);
    push_subsection(&mut s, 0x8000_00F1, &[1]);
    assert_eq!(&s[4..12], &[0xF3, 0, 0, 0, 7, 0, 0, 0]);
    assert_eq!(s.len(), 4 + (8 + 8) + (8 + 4) + (8 + 4));
    let got = section_subsections(&s).unwrap();
    assert_eq!(
        got,
        [
            Subsection {
                kind: kind::DEBUG_S_STRINGTABLE,
                data: b"\0a.cpp\0"
            },
            Subsection {
                kind: kind::DEBUG_S_SYMBOLS,
                data: &[2, 0, 6, 0]
            },
            Subsection {
                kind: 0x8000_00F1,
                data: &[1]
            },
        ]
    );
    // Padding that isn't zero, or missing at the end, is an error.
    let mut bad = s.clone();
    bad[4 + 8 + 7] = 1;
    assert!(matches!(
        section_subsections(&bad),
        Err(Error::Malformed { .. })
    ));
    assert!(matches!(
        section_subsections(&s[..s.len() - 1]),
        Err(Error::Truncated { .. })
    ));
    // A length past the end, a cut header, a wrong signature.
    assert!(section_subsections(&s[..10]).is_err());
    assert!(subsections(&words(&[0xF1, 100])).is_err());
    assert!(section_subsections(&[2, 0, 0, 0]).is_err());
    assert!(section_subsections(&[4, 0]).is_err());
    assert_eq!(section_subsections(&[4, 0, 0, 0]), Ok(vec![]));
}

#[test]
fn symbols() {
    // S_OBJNAME (unpadded, as MSVC), S_END (padded, as clang).
    let mut data = Vec::new();
    crate::write::obj_name(0, b"a.obj")
        .write_symbol(&mut data, 1)
        .unwrap();
    let end_at = data.len();
    crate::write::push_symbol(&mut data, crate::symbols::kind::S_END, &[], 4).unwrap();
    // S_BUILDINFO, padded: an id at 4 in its body.
    let bi_at = data.len();
    crate::write::push_symbol(
        &mut data,
        crate::symbols::kind::S_BUILDINFO,
        &[5, 0x10, 0, 0],
        4,
    )
    .unwrap();
    match round_trip(kind::DEBUG_S_SYMBOLS, &data) {
        Contents::Symbols(r) => {
            assert_eq!(r.len(), 3);
            assert_eq!((r[1].offset as usize, r[1].kind), (end_at, 6));
            assert_eq!(r[2].data, [5, 0x10, 0, 0]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fields_of(kind::DEBUG_S_SYMBOLS, &data),
        Ok(vec![Field::Id(bi_at as u32 + 4)])
    );
    // A record past the end, a length too short for a kind, a kind
    // without a layout.
    assert!(symbol_records(&data[..data.len() - 1]).is_err());
    assert!(symbol_records(&[1, 0, 6]).is_err());
    assert!(symbol_records(&[0]).is_err());
    assert_eq!(
        fields_of(kind::DEBUG_S_SYMBOLS, &[2, 0, 0x99, 0x11]),
        Err(Error::Symbol {
            at: 0,
            error: FieldError::Unknown(0x1199)
        })
    );
}

#[test]
fn lines_with_and_without_columns() {
    // offset, segment, flags, size; a block of two lines; a block of one.
    let mut plain = words(&[0x10]);
    plain.extend_from_slice(&[1, 0, 0, 0]);
    plain.extend_from_slice(&words(&[0x40]));
    plain.extend_from_slice(&words(&[0x18, 2, 12 + 16, 0, 0x8000_0005, 4, 0x8000_0006]));
    plain.extend_from_slice(&words(&[0x30, 1, 12 + 8, 8, 9]));
    match round_trip(kind::DEBUG_S_LINES, &plain) {
        Contents::Lines(l) => {
            assert_eq!(
                (l.offset, l.segment, l.flags, l.code_size),
                (0x10, 1, 0, 0x40)
            );
            assert_eq!(l.blocks.len(), 2);
            assert_eq!(l.blocks[0].lines, [(0, 0x8000_0005), (4, 0x8000_0006)]);
            assert_eq!(l.blocks[1].file, 0x30);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fields_of(kind::DEBUG_S_LINES, &plain),
        Ok(vec![
            Field::Address {
                offset: 0,
                section: 4
            },
            Field::Checksum(12),
            Field::Checksum(12 + 28),
        ])
    );
    // With columns: each block's lines, then their columns.
    let mut cols = words(&[0]);
    cols.extend_from_slice(&[1, 0, 1, 0]);
    cols.extend_from_slice(&words(&[8]));
    cols.extend_from_slice(&words(&[0, 1, 12 + 12, 0, 0x8000_0001]));
    cols.extend_from_slice(&[5, 0, 9, 0]);
    cols.extend_from_slice(&words(&[0x18, 0, 12]));
    match round_trip(kind::DEBUG_S_IL_LINES, &cols) {
        Contents::IlLines(l) => {
            assert_eq!(l.blocks[0].columns, [(5, 9)]);
            assert!(l.blocks[1].lines.is_empty());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fields_of(kind::DEBUG_S_IL_LINES, &cols).unwrap()[1..],
        [Field::Checksum(12), Field::Checksum(36)]
    );
    // A block size that disagrees with its lines; truncated.
    let mut bad = plain.clone();
    bad[20] += 4;
    assert!(matches!(
        Lines::parse(&bad),
        Err(Error::Malformed { at: 20, .. })
    ));
    assert!(Lines::parse(&plain[..plain.len() - 1]).is_err());
    assert!(Lines::parse(&plain[..8]).is_err());
    let il = Contents::parse(kind::DEBUG_S_IL_LINES, &bad);
    assert_eq!(
        il,
        Err(Error::Malformed {
            kind: kind::DEBUG_S_IL_LINES,
            at: 20
        })
    );
}

#[test]
fn file_checksums_are_padded_entries() {
    let mut data = Vec::new();
    let md5 = [0xAB; 16];
    let a = push_file_checksum(
        &mut data,
        &FileChecksum {
            name: 1,
            kind: 1,
            checksum: &md5,
        },
    );
    let b = push_file_checksum(
        &mut data,
        &FileChecksum {
            name: 7,
            kind: 0,
            checksum: &[],
        },
    );
    let c = push_file_checksum(
        &mut data,
        &FileChecksum {
            name: 9,
            kind: 3,
            checksum: &[1, 2, 3],
        },
    );
    assert_eq!((a, b, c), (0, 24, 32));
    assert_eq!(data.len(), 32 + 12);
    let entries = file_checksums(&data).unwrap();
    assert_eq!(
        entries.iter().map(|e| (e.0, e.1.name)).collect::<Vec<_>>(),
        [(0, 1), (24, 7), (32, 9)]
    );
    round_trip(kind::DEBUG_S_FILECHKSMS, &data);
    assert_eq!(
        fields_of(kind::DEBUG_S_FILECHKSMS, &data),
        Ok(vec![Field::String(0), Field::String(24), Field::String(32)])
    );
    let mut bad = data.clone();
    bad[31] = 1;
    assert!(file_checksums(&bad).is_err());
    assert!(file_checksums(&data[..data.len() - 1]).is_err());
}

#[test]
fn string_tables() {
    let table = b"\0a.cpp\0b.h\0";
    round_trip(kind::DEBUG_S_STRINGTABLE, table);
    assert_eq!(
        strings(table),
        Ok(vec![(0, &b""[..]), (1, b"a.cpp"), (7, b"b.h")])
    );
    assert!(strings(b"\0x").is_err());
    let mut b = StringTableBuilder::default();
    assert_eq!(b.insert(b"a.cpp"), 1);
    assert_eq!(b.insert(b"b.h"), 7);
    assert_eq!(b.insert(b"a.cpp"), 1);
    assert_eq!(b.insert(b""), 0);
    assert_eq!(b.bytes(), table);
}

#[test]
fn inlinee_lines_both_signatures() {
    let plain = words(&[0, 0x1050, 0x18, 8, 0x104D, 0, 5]);
    match round_trip(kind::DEBUG_S_INLINEELINES, &plain) {
        Contents::InlineeLines(i) => {
            assert!(!i.extended);
            assert_eq!(i.entries[1].inlinee, 0x104D);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fields_of(kind::DEBUG_S_INLINEELINES, &plain),
        Ok(vec![
            Field::Id(4),
            Field::Checksum(8),
            Field::Id(16),
            Field::Checksum(20)
        ])
    );
    let ex = words(&[1, 0x1000, 0x30, 12, 2, 0x48, 0x60, 0x1001, 0, 3, 0]);
    match round_trip(kind::DEBUG_S_INLINEELINES, &ex) {
        Contents::InlineeLines(i) => {
            assert!(i.extended);
            assert_eq!(i.entries[0].extra_files, [0x48, 0x60]);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fields_of(kind::DEBUG_S_INLINEELINES, &ex),
        Ok(vec![
            Field::Id(4),
            Field::Checksum(8),
            Field::Checksum(20),
            Field::Checksum(24),
            Field::Id(28),
            Field::Checksum(32),
        ])
    );
    // An unknown signature, truncation, a count past the end.
    assert!(InlineeLines::parse(&words(&[7])).is_err());
    assert!(InlineeLines::parse(&plain[..plain.len() - 2]).is_err());
    assert!(InlineeLines::parse(&words(&[1, 1, 2, 3, u32::MAX])).is_err());
    assert!(InlineeLines::parse(&[]).is_err());
}

#[test]
fn frame_data_with_and_without_its_base() {
    let frame = words(&[0x10, 0x20, 4, 8, 0x30, 0x55, 0x0002_0003, 5]);
    let mut with = words(&[0]);
    with.extend_from_slice(&frame);
    with.extend_from_slice(&frame);
    match round_trip(kind::DEBUG_S_FRAMEDATA, &with) {
        Contents::FrameData(f) => {
            assert_eq!(f.base, Some(0));
            assert_eq!(f.frames.len(), 2);
            let g = f.frames[0];
            assert_eq!(
                (
                    g.rva_start,
                    g.code_size,
                    g.frame_func,
                    g.prolog_size,
                    g.saved_regs_size
                ),
                (0x10, 0x20, 0x55, 3, 2)
            );
            assert_eq!(g.flags, 5);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fields_of(kind::DEBUG_S_FRAMEDATA, &with),
        Ok(vec![Field::Rva(0), Field::String(24), Field::String(56)])
    );
    match round_trip(kind::DEBUG_S_FRAMEDATA, &frame) {
        Contents::FrameData(f) => assert_eq!(f.base, None),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fields_of(kind::DEBUG_S_FRAMEDATA, &frame),
        Ok(vec![Field::String(20)])
    );
    assert!(FrameData::parse(&frame[..30]).is_err());
}

#[test]
fn cross_scope_imports_and_exports() {
    let imports = words(&[0x10, 2, 0x8000_1000, 0x8000_1001, 0x20, 0]);
    match round_trip(kind::DEBUG_S_CROSSSCOPEIMPORTS, &imports) {
        Contents::CrossScopeImports(i) => {
            assert_eq!(i[0].ids, [0x8000_1000, 0x8000_1001]);
            assert_eq!(i[1].module, 0x20);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fields_of(kind::DEBUG_S_CROSSSCOPEIMPORTS, &imports),
        Ok(vec![Field::String(0), Field::String(16)])
    );
    assert!(cross_scope_imports(&words(&[1, 9])).is_err());
    let exports = words(&[0x1003, 0x8000_0001, 0x1004, 0x8000_0002]);
    match round_trip(kind::DEBUG_S_CROSSSCOPEEXPORTS, &exports) {
        Contents::CrossScopeExports(p) => assert_eq!(p[1], (0x1004, 0x8000_0002)),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        fields_of(kind::DEBUG_S_CROSSSCOPEEXPORTS, &exports),
        Ok(vec![Field::Id(0), Field::Id(8)])
    );
    assert!(pairs(kind::DEBUG_S_CROSSSCOPEEXPORTS, &exports[..12]).is_err());
}

#[test]
fn token_maps_merged_assemblies_and_rvas() {
    // Two entries, then data cvdump shows raw.
    let mut map = words(&[2, 0x1000, 0x8600_0001, 0x1001, 0x40]);
    map.extend_from_slice(&[9, 9]);
    for k in [
        kind::DEBUG_S_FUNC_MDTOKEN_MAP,
        kind::DEBUG_S_TYPE_MDTOKEN_MAP,
    ] {
        match round_trip(k, &map) {
            Contents::FuncTokenMap(m) | Contents::TypeTokenMap(m) => {
                assert_eq!(m.entries, [(0x1000, 0x8600_0001), (0x1001, 0x40)]);
                assert_eq!(m.rest, [9, 9]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(fields_of(k, &map), Ok(vec![]));
        assert!(TokenMap::parse(k, &words(&[3, 1, 2])).is_err());
        assert!(TokenMap::parse(k, &words(&[u32::MAX])).is_err());
    }
    // A time stamp, an index, a version of 3 bytes (5 with its size), a
    // name: 8 + 5 + 4 = 17, padded to 20.
    let mut asm = words(&[0x5F00_0000, 0x8000_0001]);
    asm.extend_from_slice(&[5, 0, 1, 2, 3]);
    asm.extend_from_slice(b"a.b\0");
    asm.extend_from_slice(&[0, 0, 0]);
    match round_trip(kind::DEBUG_S_MERGED_ASSEMBLYINPUT, &asm) {
        Contents::MergedAssemblyInput(m) => {
            assert_eq!(m[0].version, [1, 2, 3]);
            assert_eq!(m[0].name, b"a.b");
        }
        other => panic!("{other:?}"),
    }
    let mut bad = asm.clone();
    bad[19] = 1;
    assert!(merged_assemblies(&bad).is_err());
    let mut short = asm.clone();
    short[8] = 1; // a size below its own two bytes
    assert!(merged_assemblies(&short).is_err());
    let rvas = words(&[0x1000, 0x2000]);
    match round_trip(kind::DEBUG_S_COFF_SYMBOL_RVA, &rvas) {
        Contents::CoffSymbolRva(r) => assert_eq!(r, [0x1000, 0x2000]),
        other => panic!("{other:?}"),
    }
    assert!(Contents::parse(kind::DEBUG_S_COFF_SYMBOL_RVA, &[1, 2]).is_err());
}

#[test]
fn unknown_and_ignored_kinds_are_kept() {
    for k in [0xFE, 0x8000_00F2, 0x1234] {
        assert_eq!(round_trip(k, &[1, 2, 3]), Contents::Other(&[1, 2, 3]));
        assert_eq!(fields_of(k, &[1, 2, 3]), Ok(vec![]));
    }
}

#[test]
fn errors_say_where() {
    assert_eq!(
        Error::Truncated { kind: 0xF2, at: 8 }.to_string(),
        "subsection 0xf2 truncated at 0x8"
    );
    assert_eq!(
        Error::Malformed { kind: 0xF6, at: 0 }.to_string(),
        "subsection 0xf6 malformed at 0x0"
    );
    assert_eq!(
        Error::Symbol {
            at: 4,
            error: FieldError::Unknown(0x1199)
        }
        .to_string(),
        "symbol at 0x4: unknown symbol record kind 0x1199"
    );
}

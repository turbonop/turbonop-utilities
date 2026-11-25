use super::*;
use Field::*;

fn of(kind: u16, body: &[u8]) -> Result<Vec<Field>, FieldError> {
    fields_of(kind, body)
}

fn addr(offset: u32, section: u32) -> Field {
    Address { offset, section }
}

/// Every kind with a fixed layout: its fields, from a body long enough for
/// any of them (zeros: counts of 0, flags clear).
#[test]
fn fixed_layouts() {
    let none: &[u16] = &[
        S_COMPILE,
        S_SKIP,
        S_CVRESERVE,
        S_OBJNAME_ST,
        S_ENDARG,
        S_RETURN,
        S_ENTRYTHIS,
        S_SLINK32,
        S_PROCREF_ST,
        S_DATAREF_ST,
        S_ALIGN,
        S_LPROCREF_ST,
        S_COMPILE2_ST,
        S_UNAMESPACE_ST,
        S_OBJNAME,
        S_COMPILE2,
        S_COMPILE3,
        S_ENVBLOCK,
        S_UNAMESPACE,
        S_PROCREF,
        S_DATAREF,
        S_LPROCREF,
        S_ANNOTATIONREF,
        S_TOKENREF,
        S_SECTION,
        S_EXPORT,
        S_FRAMECOOKIE,
        S_END,
        S_INLINESITE_END,
        S_PROC_ID_END,
        S_DEFRANGE_FRAMEPOINTER_REL_FULL_SCOPE,
        S_POGODATA,
        S_MOD_TYPEREF,
        S_PDBMAP,
        S_DPC_SYM_TAG_MAP,
        // Counts of 0, and a REFMINIPDB that isn't a UDT's.
        S_CALLEES,
        S_CALLERS,
        S_INLINEES,
        S_REF_MINIPDB,
    ];
    let body = [0u8; 64];
    for &k in none {
        assert_eq!(of(k, &body), Ok(vec![]), "{}", name(k));
    }
    let proc_type = vec![Link(0), Link(4), Link(8), Type(24), addr(28, 32)];
    let proc_id = vec![Link(0), Link(4), Link(8), Id(24), addr(28, 32)];
    let table: Vec<(&[u16], Vec<Field>)> = vec![
        (&[S_SSEARCH], vec![Link(0)]),
        (&[S_OEM], vec![Type(16)]),
        (&[S_CEXMODEL32], vec![addr(0, 4)]),
        (
            &[S_THUNK32, S_THUNK32_ST],
            vec![Link(0), Link(4), Link(8), addr(12, 16)],
        ),
        (
            &[S_BLOCK32, S_BLOCK32_ST, S_WITH32, S_WITH32_ST],
            vec![Link(0), Link(4), addr(12, 16)],
        ),
        (&[S_LABEL32, S_LABEL32_ST], vec![addr(0, 4)]),
        (
            &[
                S_REGISTER,
                S_REGISTER_ST,
                S_CONSTANT,
                S_CONSTANT_ST,
                S_UDT,
                S_UDT_ST,
                S_COBOLUDT,
                S_COBOLUDT_ST,
                S_MANYREG,
                S_MANYREG_ST,
                S_MANYREG2,
                S_MANYREG2_ST,
                S_MANTYPREF,
                S_LOCAL,
                S_GDATA_HLSL,
                S_LDATA_HLSL,
                S_GDATA_HLSL32,
                S_LDATA_HLSL32,
                S_GDATA_HLSL32_EX,
                S_LDATA_HLSL32_EX,
                S_LOCAL_DPC_GROUPSHARED,
            ],
            vec![Type(0)],
        ),
        (&[S_MANCONSTANT], vec![Token(0)]),
        (
            &[
                S_BPREL32,
                S_BPREL32_ST,
                S_REGREL32,
                S_REGREL32_ST,
                S_REGREL32_INDIR,
                S_LOCALSLOT,
                S_LOCALSLOT_ST,
                S_PARAMSLOT,
                S_PARAMSLOT_ST,
            ],
            vec![Type(4)],
        ),
        (
            &[
                S_LDATA32,
                S_LDATA32_ST,
                S_GDATA32,
                S_GDATA32_ST,
                S_LTHREAD32,
                S_LTHREAD32_ST,
                S_GTHREAD32,
                S_GTHREAD32_ST,
            ],
            vec![Type(0), addr(4, 8)],
        ),
        (
            &[S_LMANDATA, S_LMANDATA_ST, S_GMANDATA, S_GMANDATA_ST],
            vec![Token(0), addr(4, 8)],
        ),
        (&[S_PUB32, S_PUB32_ST], vec![addr(4, 8)]),
        (
            &[
                S_LPROC32,
                S_LPROC32_ST,
                S_GPROC32,
                S_GPROC32_ST,
                S_LPROC32_DPC,
                S_LPROCIA64,
                S_LPROCIA64_ST,
                S_GPROCIA64,
                S_GPROCIA64_ST,
            ],
            proc_type,
        ),
        (
            &[
                S_LPROC32_ID,
                S_GPROC32_ID,
                S_LPROC32_DPC_ID,
                S_LPROCIA64_ID,
                S_GPROCIA64_ID,
            ],
            proc_id,
        ),
        (
            &[S_LPROCMIPS, S_LPROCMIPS_ST, S_GPROCMIPS, S_GPROCMIPS_ST],
            vec![Link(0), Link(4), Link(8), Type(40), addr(44, 48)],
        ),
        (
            &[S_LPROCMIPS_ID, S_GPROCMIPS_ID],
            vec![Link(0), Link(4), Link(8), Id(40), addr(44, 48)],
        ),
        (&[S_VFTABLE32], vec![Type(0), Type(4), addr(8, 12)]),
        (&[S_FRAMEPROC], vec![addr(16, 20)]),
        (&[S_ANNOTATION], vec![addr(0, 4)]),
        (
            &[S_GMANPROC, S_GMANPROC_ST, S_LMANPROC, S_LMANPROC_ST],
            vec![Link(0), Link(4), Link(8), Token(24), addr(28, 32)],
        ),
        (&[S_ATTR_FRAMEREL], vec![Type(4), addr(8, 12)]),
        (
            &[S_MANFRAMEREL, S_MANFRAMEREL_ST, S_MANSLOT, S_MANSLOT_ST],
            vec![Token(4), addr(8, 12)],
        ),
        (
            &[S_ATTR_REGISTER, S_ATTR_MANYREG],
            vec![Type(0), addr(4, 8)],
        ),
        (
            &[
                S_MANREGISTER,
                S_MANREGISTER_ST,
                S_MANMANYREG,
                S_MANMANYREG_ST,
                S_MANMANYREG2,
                S_MANMANYREG2_ST,
            ],
            vec![Token(0), addr(4, 8)],
        ),
        (&[S_ATTR_REGREL], vec![Type(4), addr(10, 14)]),
        (&[S_MANREGREL, S_MANREGREL_ST], vec![Token(4), addr(10, 14)]),
        (&[S_TRAMPOLINE], vec![addr(4, 12), addr(8, 14)]),
        (
            &[S_SEPCODE],
            vec![Link(0), Link(4), addr(16, 24), addr(20, 26)],
        ),
        (&[S_COFFGROUP], vec![addr(8, 12)]),
        (
            &[S_CALLSITEINFO, S_HEAPALLOCSITE],
            vec![addr(0, 4), Type(8)],
        ),
        (&[S_DISCARDED], vec![Checksum(4)]),
        (&[S_FILESTATIC], vec![Type(0), String(4)]),
        (&[S_DEFRANGE], vec![String(0), addr(4, 8)]),
        (&[S_DEFRANGE_SUBFIELD], vec![String(0), addr(8, 12)]),
        (
            &[S_DEFRANGE_REGISTER, S_DEFRANGE_FRAMEPOINTER_REL],
            vec![addr(4, 8)],
        ),
        (
            &[
                S_DEFRANGE_SUBFIELD_REGISTER,
                S_DEFRANGE_REGISTER_REL,
                S_DEFRANGE_HLSL,
                S_DEFRANGE_DPC_PTR_TAG,
            ],
            vec![addr(8, 12)],
        ),
        (&[S_DEFRANGE_REGISTER_REL_INDIR], vec![addr(12, 16)]),
        (&[S_BUILDINFO, S_HOTPATCHFUNC], vec![Id(0)]),
        (
            &[S_INLINESITE],
            vec![Link(0), Link(4), Id(8), Annotations(12)],
        ),
        (
            &[S_INLINESITE2],
            vec![Link(0), Link(4), Id(8), Annotations(16)],
        ),
        (
            &[S_ARMSWITCHTABLE],
            vec![addr(0, 4), addr(8, 16), addr(12, 18)],
        ),
        (&[S_ASSOCIATION], vec![addr(2, 6)]),
    ];
    for (kinds, want) in &table {
        for &k in *kinds {
            assert_eq!(of(k, &body).as_ref(), Ok(want), "{}", name(k));
        }
    }
}

/// Each kind with fields reports a body too short for its last field as
/// malformed, never a field past the end.
#[test]
fn truncated_records_are_malformed() {
    let body = [0u8; 64];
    for &k in ALL {
        let Ok(full) = of(k, &body) else { continue };
        let Some(end) = full
            .iter()
            .map(|f| match *f {
                Address { offset, section } => (offset + 4).max(section + 2),
                Annotations(o) => o,
                f => f.offset() + 4,
            })
            .max()
        else {
            continue;
        };
        for len in 0..end as usize {
            let got = of(k, &body[..len]);
            assert_eq!(got, Err(FieldError::Malformed(k)), "{} {len}", name(k));
        }
        // (A constant's numeric follows its type.)
        let numeric = matches!(k, S_CONSTANT | S_CONSTANT_ST | S_MANCONSTANT);
        assert!(
            of(k, &body[..end as usize]).is_ok() || numeric,
            "{}",
            name(k)
        );
    }
}

/// Every kind cvinfo.h or LLVM names is handled on purpose: it has a
/// layout, or it is one of the 16-bit kinds, or one of the named kinds
/// without a documented layout. A kind added to the table without a
/// layout fails here.
#[test]
fn every_named_kind_is_decided() {
    let undocumented = [
        S_RESERVED1,
        S_RESERVED2,
        S_RESERVED3,
        S_RESERVED4,
        S_LOCAL_2005,
        S_DEFRANGE_2005,
        S_DEFRANGE2_2005,
        S_FRAMEREG,
        S_FASTLINK,
        S_BPREL32_INDIR,
        S_GPROC32EX,
        S_LPROC32EX,
        S_GPROC32EX_ID,
        S_LPROC32EX_ID,
        S_STATICLOCAL,
        S_BPREL32_ENCTMP,
        S_REGREL32_ENCTMP,
        S_BPREL32_INDIR_ENCTMP,
        S_REGREL32_INDIR_ENCTMP,
        S_SOURCELINK,
        S_DEFRANGE_CONSTVAL_ON_ENTRY,
        S_DEFRANGE_GLOBALSYM_ON_ENTRY,
        S_ALTOBJNAME,
    ];
    let body = [0u8; 64];
    let (mut ok, mut legacy) = (0, 0);
    for &k in ALL {
        match of(k, &body) {
            Ok(_) => ok += 1,
            Err(FieldError::Legacy(_)) => {
                legacy += 1;
                assert!(k < 0x1000, "{}", name(k));
            }
            Err(FieldError::Unknown(_)) => {
                assert!(undocumented.contains(&k), "{} has no layout", name(k));
            }
            Err(e) => panic!("{}: {e}", name(k)),
        }
    }
    assert_eq!(legacy, 30);
    assert_eq!(ok + legacy + undocumented.len(), ALL.len());
    // And an unnamed kind is unknown.
    assert_eq!(of(0x1200, &body), Err(FieldError::Unknown(0x1200)));
    assert_eq!(of(0x117D, &body), Err(FieldError::Unknown(0x117D)));
}

#[test]
fn function_lists_by_count() {
    let mut body = 3u32.to_le_bytes().to_vec();
    body.extend_from_slice(&[0; 12]);
    // (Followed by the invocation counts, which aren't fields.)
    body.extend_from_slice(&[0; 12]);
    for k in [S_CALLEES, S_CALLERS, S_INLINEES] {
        assert_eq!(of(k, &body), Ok(vec![Id(4), Id(8), Id(12)]));
        assert_eq!(of(k, &body[..15]), Err(FieldError::Malformed(k)));
    }
    // A count that overflows is malformed, not a panic.
    let huge = u32::MAX.to_le_bytes();
    assert_eq!(of(S_CALLEES, &huge), Err(FieldError::Malformed(S_CALLEES)));
}

#[test]
fn mini_pdb_references_name_a_type_only_for_a_udt() {
    let mut body = vec![0u8; 12];
    body[6] = REF_MINIPDB_UDT as u8;
    assert_eq!(of(S_REF_MINIPDB, &body), Ok(vec![Type(0)]));
    body[6] = 1 << 1; // fData: a section, not a type
    assert_eq!(of(S_REF_MINIPDB, &body), Ok(vec![]));
}

#[test]
fn constants_need_their_whole_numeric() {
    // type, LF_ULONG and its 4 bytes, a name.
    let mut body = 0x1003u32.to_le_bytes().to_vec();
    body.extend_from_slice(&crate::types::leaf::LF_ULONG.to_le_bytes());
    body.extend_from_slice(&0x1234_5678u32.to_le_bytes());
    body.extend_from_slice(b"k\0");
    assert_eq!(of(S_CONSTANT, &body), Ok(vec![Type(0)]));
    assert_eq!(of(S_MANCONSTANT, &body), Ok(vec![Token(0)]));
    // Cut inside the numeric's value.
    assert_eq!(
        of(S_CONSTANT, &body[..8]),
        Err(FieldError::Malformed(S_CONSTANT))
    );
}

#[test]
fn refs_are_the_index_fields() {
    let mut r = Vec::new();
    refs(S_GPROC32_ID, &[0; 40], &mut r).unwrap();
    assert_eq!(
        r,
        [crate::type_refs::Ref {
            offset: 24,
            id: true
        }]
    );
    r.clear();
    refs(S_CALLSITEINFO, &[0; 12], &mut r).unwrap();
    assert_eq!(
        r,
        [crate::type_refs::Ref {
            offset: 8,
            id: false
        }]
    );
}

#[test]
fn shifting_moves_every_part() {
    assert_eq!(addr(4, 8).shifted(4), addr(8, 12));
    assert_eq!(Id(8).shifted(4), Id(12));
    assert_eq!(addr(4, 8).offset(), 4);
}

#[test]
fn errors_name_the_kind() {
    assert_eq!(
        FieldError::Malformed(S_UDT).to_string(),
        "malformed symbol record (S_UDT 0x1108)"
    );
    assert_eq!(
        FieldError::Unknown(0x1234).to_string(),
        "unknown symbol record kind 0x1234"
    );
    assert_eq!(
        FieldError::Legacy(S_BPREL16).to_string(),
        "16-bit symbol record kind S_BPREL16 (0x0100)"
    );
}

use super::*;

fn offsets(kind: u16, body: &[u8]) -> Result<Vec<(u32, bool)>, RefError> {
    let mut v = Vec::new();
    refs(kind, body, &mut v)?;
    Ok(v.into_iter().map(|r| (r.offset, r.id)).collect())
}

fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

fn cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

#[test]
fn modifier_and_bitfield() {
    assert_eq!(offsets(leaf::LF_MODIFIER, &[0; 6]), Ok(vec![(0, false)]));
    assert_eq!(offsets(leaf::LF_BITFIELD, &[0; 6]), Ok(vec![(0, false)]));
}

#[test]
fn pointers_name_their_class_only_for_members() {
    let plain = cat(&[&le32(0x1000), &le32(0x1000C)]);
    assert_eq!(offsets(leaf::LF_POINTER, &plain), Ok(vec![(0, false)]));
    // ptrmode 2 (pointer to data member) at bits 5-7.
    let member = cat(&[
        &le32(0x1000),
        &le32((2 << 5) | 0x1000C),
        &le32(0x1001),
        &[0, 0],
    ]);
    assert_eq!(
        offsets(leaf::LF_POINTER, &member),
        Ok(vec![(0, false), (8, false)])
    );
    let function = cat(&[
        &le32(0x1000),
        &le32((3 << 5) | 0x1000C),
        &le32(0x1001),
        &[0, 0],
    ]);
    assert_eq!(
        offsets(leaf::LF_POINTER, &function),
        Ok(vec![(0, false), (8, false)])
    );
    // A member pointer without its class is malformed.
    let short = cat(&[&le32(0x1000), &le32(2 << 5)]);
    assert_eq!(
        offsets(leaf::LF_POINTER, &short),
        Err(RefError::Malformed(leaf::LF_POINTER))
    );
}

#[test]
fn procedures_and_member_functions() {
    assert_eq!(
        offsets(leaf::LF_PROCEDURE, &[0; 12]),
        Ok(vec![(0, false), (8, false)])
    );
    assert_eq!(
        offsets(leaf::LF_MFUNCTION, &[0; 24]),
        Ok(vec![(0, false), (4, false), (8, false), (16, false)])
    );
}

#[test]
fn argument_lists_by_count() {
    let body = cat(&[&le32(3), &[0; 12]]);
    assert_eq!(
        offsets(leaf::LF_ARGLIST, &body),
        Ok(vec![(4, false), (8, false), (12, false)])
    );
    let short = cat(&[&le32(3), &[0; 8]]);
    assert_eq!(
        offsets(leaf::LF_ARGLIST, &short),
        Err(RefError::Malformed(leaf::LF_ARGLIST))
    );
    // A count that overflows is malformed, not a panic.
    let huge = cat(&[&le32(u32::MAX), &[0; 8]]);
    assert!(offsets(leaf::LF_ARGLIST, &huge).is_err());
}

#[test]
fn method_lists_skip_virtual_base_offsets() {
    // Vanilla (no offset), then introducing virtual (mprop 4: an offset).
    let body = cat(&[
        &[0, 0, 0, 0],
        &le32(0x1000),
        &[4 << 2, 0, 0, 0],
        &le32(0x1001),
        &le32(8),
    ]);
    assert_eq!(
        offsets(leaf::LF_METHODLIST, &body),
        Ok(vec![(4, false), (12, false)])
    );
}

#[test]
fn user_defined_types() {
    let class = [0u8; 18];
    assert_eq!(
        offsets(leaf::LF_STRUCTURE, &class),
        Ok(vec![(4, false), (8, false), (12, false)])
    );
    assert_eq!(
        offsets(leaf::LF_STRUCTURE2, &[0; 20]),
        Ok(vec![(4, false), (8, false), (12, false)])
    );
    assert_eq!(offsets(leaf::LF_UNION, &[0; 10]), Ok(vec![(4, false)]));
    assert_eq!(offsets(leaf::LF_UNION2, &[0; 12]), Ok(vec![(4, false)]));
    assert_eq!(
        offsets(leaf::LF_ENUM, &[0; 13]),
        Ok(vec![(4, false), (8, false)])
    );
    assert_eq!(
        offsets(leaf::LF_ARRAY, &[0; 10]),
        Ok(vec![(0, false), (4, false)])
    );
}

#[test]
fn ids() {
    assert_eq!(
        offsets(leaf::LF_FUNC_ID, &[0; 9]),
        Ok(vec![(0, true), (4, false)])
    );
    assert_eq!(
        offsets(leaf::LF_MFUNC_ID, &[0; 9]),
        Ok(vec![(0, false), (4, false)])
    );
    let build = cat(&[&[2, 0], &[0; 8]]);
    assert_eq!(
        offsets(leaf::LF_BUILDINFO, &build),
        Ok(vec![(2, true), (6, true)])
    );
    let list = cat(&[&le32(1), &[0; 4]]);
    assert_eq!(offsets(leaf::LF_SUBSTR_LIST, &list), Ok(vec![(4, true)]));
    assert_eq!(offsets(leaf::LF_STRING_ID, &[0; 5]), Ok(vec![(0, true)]));
    assert_eq!(
        offsets(leaf::LF_UDT_SRC_LINE, &[0; 12]),
        Ok(vec![(0, false), (4, true)])
    );
    assert_eq!(
        offsets(leaf::LF_UDT_MOD_SRC_LINE, &[0; 14]),
        Ok(vec![(0, false)])
    );
}

#[test]
fn records_without_references() {
    for kind in [
        leaf::LF_VTSHAPE,
        leaf::LF_LABEL,
        leaf::LF_PRECOMP,
        leaf::LF_ENDPRECOMP,
    ] {
        assert_eq!(offsets(kind, &[0; 16]), Ok(vec![]));
    }
}

#[test]
fn unknown_kinds_are_errors() {
    assert_eq!(offsets(0x1234, &[]), Err(RefError::Unknown(0x1234)));
}

#[test]
fn field_lists() {
    let mut body = Vec::new();
    // LF_BCLASS: attributes, type, offset 0 (a short numeric).
    body.extend_from_slice(&leaf::LF_BCLASS.to_le_bytes());
    body.extend_from_slice(&[3, 0]);
    body.extend_from_slice(&le32(0x1000));
    body.extend_from_slice(&[0, 0]);
    // 10 bytes so far; pad to 12.
    body.extend_from_slice(&[0xF2, 0xF1]);
    // LF_MEMBER: attributes, type, offset (an LF_ULONG numeric), name "x".
    let member_at = body.len();
    body.extend_from_slice(&leaf::LF_MEMBER.to_le_bytes());
    body.extend_from_slice(&[3, 0]);
    body.extend_from_slice(&le32(0x1001));
    body.extend_from_slice(&leaf::LF_ULONG.to_le_bytes());
    body.extend_from_slice(&le32(0x10000));
    body.extend_from_slice(b"x\0");
    // LF_ENUMERATE: no references.
    body.extend_from_slice(&leaf::LF_ENUMERATE.to_le_bytes());
    body.extend_from_slice(&[3, 0, 5, 0]);
    body.extend_from_slice(b"e\0");
    // LF_ONEMETHOD, introducing virtual: a virtual-base offset before the name.
    let method_at = body.len();
    body.extend_from_slice(&leaf::LF_ONEMETHOD.to_le_bytes());
    body.extend_from_slice(&[4 << 2, 0]);
    body.extend_from_slice(&le32(0x1002));
    body.extend_from_slice(&le32(0));
    body.extend_from_slice(b"f\0");
    // LF_VBCLASS: two references.
    let vb_at = body.len();
    body.extend_from_slice(&leaf::LF_VBCLASS.to_le_bytes());
    body.extend_from_slice(&[3, 0]);
    body.extend_from_slice(&le32(0x1003));
    body.extend_from_slice(&le32(0x1004));
    body.extend_from_slice(&[0, 0, 1, 0]);
    // LF_INDEX: a continuation.
    let index_at = body.len();
    body.extend_from_slice(&leaf::LF_INDEX.to_le_bytes());
    body.extend_from_slice(&[0, 0]);
    body.extend_from_slice(&le32(0x1005));

    let at = |base: usize, plus: usize| ((base + plus) as u32, false);
    assert_eq!(
        offsets(leaf::LF_FIELDLIST, &body),
        Ok(vec![
            (4, false),
            at(member_at, 4),
            at(method_at, 4),
            at(vb_at, 4),
            at(vb_at, 8),
            at(index_at, 4),
        ])
    );
}

#[test]
fn field_list_members_unknown_or_truncated() {
    let unknown = 0x1499u16.to_le_bytes();
    assert_eq!(
        offsets(leaf::LF_FIELDLIST, &unknown),
        Err(RefError::Unknown(0x1499))
    );
    let mut truncated = leaf::LF_MEMBER.to_le_bytes().to_vec();
    truncated.extend_from_slice(&[3, 0, 0]);
    assert!(offsets(leaf::LF_FIELDLIST, &truncated).is_err());
}

#[test]
fn friend_classes_are_0x140a() {
    // LF_FRIENDCLS: pad(2) type(4), then an LF_INDEX.
    let body = cat(&[
        &0x140Au16.to_le_bytes(),
        &[0, 0],
        &le32(0x1000),
        &leaf::LF_INDEX.to_le_bytes(),
        &[0, 0],
        &le32(0x1001),
    ]);
    assert_eq!(
        offsets(leaf::LF_FIELDLIST, &body),
        Ok(vec![(4, false), (12, false)])
    );
}

/// The `*_ST` members, whose names are length-prefixed: the name's bytes
/// are skipped by its length, not to a zero.
#[test]
fn length_prefixed_member_names() {
    // LF_MEMBER_ST: attributes, type, offset 0, name "a\0b" (3 bytes: a
    // zero inside, which a zero-terminated reading would stop at).
    let mut body = cat(&[
        &0x1405u16.to_le_bytes(),
        &[3, 0],
        &le32(0x1000),
        &[0, 0],
        &[3, b'a', 0, b'b'],
    ]);
    // LF_ONEMETHOD_ST, vanilla: attributes, type, name "f".
    let at = body.len();
    body.extend_from_slice(&0x140Bu16.to_le_bytes());
    body.extend_from_slice(&[0, 0]);
    body.extend_from_slice(&le32(0x1001));
    body.extend_from_slice(&[1, b'f']);
    assert_eq!(
        offsets(leaf::LF_FIELDLIST, &body),
        Ok(vec![(4, false), ((at + 4) as u32, false)])
    );
    // A length past the end is malformed.
    let short = cat(&[&0x1408u16.to_le_bytes(), &[0, 0], &le32(0x1000), &[9, b'x']]);
    assert_eq!(
        offsets(leaf::LF_FIELDLIST, &short),
        Err(RefError::Malformed(0x1408))
    );
}

#[test]
fn rare_32_bit_kinds() {
    let counted = cat(&[&le32(2), &le32(0x1000), &le32(0x1001)]);
    // LF_VFTPATH, LF_DERIVED: a count and types.
    for k in [0x100D, 0x1204] {
        assert_eq!(offsets(k, &counted), Ok(vec![(4, false), (8, false)]));
    }
    // LF_DIMARRAY(_ST): underlying, dimensions.
    for k in [0x1508, 0x100C] {
        assert_eq!(offsets(k, &[0; 9]), Ok(vec![(0, false), (4, false)]));
    }
    // LF_DEFARG(_ST), LF_DIMCONU/LU, LF_BARRAY, LF_COBOL0: one type first.
    for k in [0x150B, 0x1202, 0x1207, 0x1208, 0x100B, 0x100A] {
        assert_eq!(offsets(k, &[0; 6]), Ok(vec![(0, false)]));
    }
    // LF_DIMVARU: rank 2, the index type, two bounds; LF_DIMVARLU: four.
    let var = cat(&[&le32(2), &[0; 20]]);
    assert_eq!(
        offsets(0x1209, &var),
        Ok(vec![(4, false), (8, false), (12, false)])
    );
    assert_eq!(
        offsets(0x120A, &var),
        Ok(vec![
            (4, false),
            (8, false),
            (12, false),
            (16, false),
            (20, false)
        ])
    );
    assert!(offsets(0x120A, &var[..20]).is_err());
    // LF_OEM2: a GUID, a count, types.
    let oem = cat(&[&[0; 16], &le32(1), &le32(0x1000), &[7, 7]]);
    assert_eq!(offsets(0x1011, &oem), Ok(vec![(20, false)]));
    // The _ST forms of the UDTs, as their own.
    for k in [0x1004, 0x1005] {
        assert_eq!(
            offsets(k, &[0; 18]),
            Ok(vec![(4, false), (8, false), (12, false)])
        );
    }
    assert_eq!(offsets(0x1006, &[0; 10]), Ok(vec![(4, false)]));
    assert_eq!(offsets(0x1007, &[0; 13]), Ok(vec![(4, false), (8, false)]));
    assert_eq!(offsets(0x1003, &[0; 10]), Ok(vec![(0, false), (4, false)]));
    assert_eq!(offsets(0x1010, &[0; 5]), Ok(vec![(0, false)]));
    // Without references.
    for k in [0x000F, 0x0010, 0x000C, 0x0016, 0x100E] {
        assert_eq!(offsets(k, &[0; 8]), Ok(vec![]));
    }
}

/// LF_REFSYM holds a symbol record: its indices are the symbol's.
#[test]
fn referenced_symbols() {
    // S_REGREL32 (0x1111): offset, type 0x1234, register, name.
    let sym = cat(&[
        &0x1111u16.to_le_bytes(),
        &le32(8),
        &le32(0x1234),
        &[0, 0],
        b"v\0",
    ]);
    let body = cat(&[&(sym.len() as u16).to_le_bytes(), &sym]);
    // The type is 4 bytes into the symbol's body: 2 + 2 + 4.
    assert_eq!(offsets(0x020C, &body), Ok(vec![(8, false)]));
    // A symbol whose layout isn't known, or a length past the end.
    let bad = cat(&[&4u16.to_le_bytes(), &0x7777u16.to_le_bytes(), &[0, 0]]);
    assert_eq!(offsets(0x020C, &bad), Err(RefError::Malformed(0x020C)));
    let long = cat(&[&40u16.to_le_bytes(), &sym]);
    assert_eq!(offsets(0x020C, &long), Err(RefError::Malformed(0x020C)));
}

#[test]
fn sixteen_bit_kinds_are_legacy() {
    for k in [0x0001, 0x0008, 0x0201, 0x0204] {
        assert_eq!(offsets(k, &[0; 16]), Err(RefError::Legacy(k)));
    }
    // And their field-list members.
    let member = cat(&[&0x0406u16.to_le_bytes(), &[0; 8]]);
    assert_eq!(
        offsets(leaf::LF_FIELDLIST, &member),
        Err(RefError::Legacy(0x0406))
    );
    assert_eq!(
        RefError::Legacy(1).to_string(),
        "16-bit type record kind 0x0001"
    );
}

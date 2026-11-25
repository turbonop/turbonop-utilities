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

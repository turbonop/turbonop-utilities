use super::*;
use crate::types::{Numeric, read_numeric};
use turbonop_core::Reader;

#[test]
fn type_records_pad_with_lf_pad() {
    // LF_MODIFIER: type(4) attributes(2): two bytes of padding.
    let mut out = Vec::new();
    push_type(&mut out, leaf::LF_MODIFIER, &[0x74, 0, 0, 0, 1, 0]).unwrap();
    assert_eq!(out, [0x0A, 0, 0x01, 0x10, 0x74, 0, 0, 0, 1, 0, 0xF2, 0xF1]);
    for len in 0..12 {
        let mut out = Vec::new();
        push_type(&mut out, 0x1605, &vec![b'x'; len]).unwrap();
        assert_eq!(out.len() % 4, 0);
        let pad = type_padding(len);
        assert_eq!(out.len(), 4 + len + pad);
        assert_eq!(
            usize::from(u16::from_le_bytes([out[0], out[1]])),
            out.len() - 2
        );
        // The padding counts down to LF_PAD1.
        let tail = &out[4 + len..];
        assert!(
            tail.iter()
                .rev()
                .enumerate()
                .all(|(i, &b)| b == 0xF1 + i as u8)
        );
        // And comes off again.
        assert_eq!(unpadded(&out[4..]), &vec![b'x'; len][..]);
    }
}

#[test]
fn unpadding_reproduces_any_body() {
    // Bodies that end in padding-like bytes, aligned or not: unpadding
    // then padding gives the same bytes.
    let bodies: [&[u8]; 7] = [
        &[1, 2, 0xF2, 0xF1],
        &[1, 0xF3, 0xF2, 0xF1],
        &[1, 2, 3, 0xF1],
        &[1, 2, 0xF1],
        &[0xF1, 0xF1, 0xF1, 0xF1],
        &[1, 2, 0xF1, 0xF2],
        &[],
    ];
    for body in bodies {
        let mut out = Vec::new();
        push_type(&mut out, 0x1605, unpadded(body)).unwrap();
        if body.len() % 4 == 0 {
            assert_eq!(&out[4..], body, "{body:x?}");
        }
    }
    assert_eq!(unpadded(&[1, 2, 0xF2, 0xF1]), [1, 2]);
    assert_eq!(unpadded(&[1, 0xF3, 0xF2, 0xF1]), [1]);
    // Not a padding run: kept.
    assert_eq!(unpadded(&[1, 2, 0xF1, 0xF2]), [1, 2, 0xF1, 0xF2]);
    // Unaligned: not padded at all.
    assert_eq!(unpadded(&[1, 2, 0xF1]), [1, 2, 0xF1]);
}

#[test]
fn symbol_records_pad_with_zeros_to_their_alignment() {
    let body = [1u8, 2, 3, 4, b'n', 0];
    let mut msvc = Vec::new();
    push_symbol(&mut msvc, 0x1108, &body, 1).unwrap();
    assert_eq!(msvc, [8, 0, 0x08, 0x11, 1, 2, 3, 4, b'n', 0]);
    let mut clang = Vec::new();
    push_symbol(&mut clang, 0x1108, &body, 4).unwrap();
    assert_eq!(clang, [10, 0, 0x08, 0x11, 1, 2, 3, 4, b'n', 0, 0, 0]);
    // Already aligned: nothing added.
    let mut aligned = Vec::new();
    push_symbol(&mut aligned, 0x0006, &[], 4).unwrap();
    assert_eq!(aligned, [2, 0, 6, 0]);
}

#[test]
fn records_longer_than_the_length_field_are_errors() {
    let mut out = Vec::new();
    assert!(push_type(&mut out, 0x1605, &vec![0; MAX_RECORD_LEN - 3]).is_ok());
    // (Padded to 4, one byte more is three more.)
    let mut out = Vec::new();
    assert_eq!(
        push_type(&mut out, 0x1605, &vec![0; MAX_RECORD_LEN - 2]),
        Err(WriteError::TooLong(MAX_RECORD_LEN + 3))
    );
    assert!(out.is_empty());
    assert_eq!(
        push_symbol(&mut out, 0x1108, &vec![0; MAX_RECORD_LEN], 1),
        Err(WriteError::TooLong(MAX_RECORD_LEN + 2))
    );
    assert_eq!(
        WriteError::TooLong(7).to_string(),
        "a record of 7 bytes is too long"
    );
}

#[test]
fn numerics_take_the_fewest_bytes_and_read_back() {
    let unsigned: [(u64, usize); 7] = [
        (0, 2),
        (0x7FFF, 2),
        (0x8000, 4),
        (0xFFFF, 4),
        (0x1_0000, 6),
        (0xFFFF_FFFF, 6),
        (0x1_0000_0000, 10),
    ];
    for (v, len) in unsigned {
        let mut b = Vec::new();
        push_unsigned(&mut b, v);
        assert_eq!(b.len(), len, "{v:#x}");
        assert_eq!(
            read_numeric(&mut Reader::new(&b)).unwrap(),
            Numeric::Unsigned(v)
        );
    }
    let signed: [(i64, usize); 8] = [
        (-1, 3),
        (-128, 3),
        (-129, 4),
        (-32768, 4),
        (-32769, 6),
        (i64::from(i32::MIN), 6),
        (i64::from(i32::MIN) - 1, 10),
        (5, 2),
    ];
    for (v, len) in signed {
        let mut b = Vec::new();
        push_signed(&mut b, v);
        assert_eq!(b.len(), len, "{v}");
        let got = read_numeric(&mut Reader::new(&b)).unwrap();
        let want = if v < 0 {
            Numeric::Signed(v)
        } else {
            Numeric::Unsigned(v as u64)
        };
        assert_eq!(got, want);
    }
}

#[test]
fn builders_and_the_records_turbolib_makes() {
    let mut out = Vec::new();
    precomp(0x1000, 0x30, 0xDEAD_BEEF, b"pch.obj")
        .write_type(&mut out)
        .unwrap();
    let mut want = vec![0x16, 0, 0x09, 0x15];
    want.extend_from_slice(&0x1000u32.to_le_bytes());
    want.extend_from_slice(&0x30u32.to_le_bytes());
    want.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
    want.extend_from_slice(b"pch.obj\0");
    assert_eq!(out, want);

    let mut out = Vec::new();
    end_precomp(7).write_type(&mut out).unwrap();
    assert_eq!(out, [6, 0, 0x14, 0, 7, 0, 0, 0]);

    let mut out = Vec::new();
    obj_name(0, b"x.obj").write_symbol(&mut out, 1).unwrap();
    assert_eq!(
        out,
        [
            12, 0, 0x01, 0x11, 0, 0, 0, 0, b'x', b'.', b'o', b'b', b'j', 0
        ]
    );

    let mut b = RecordBuilder::new(0x1107);
    b.u32(0x74).signed(-2).name(b"k");
    assert_eq!(b.kind(), 0x1107);
    assert_eq!(b.body(), [0x74, 0, 0, 0, 0x00, 0x80, 0xFE, b'k', 0]);
    b.u8(1).u16(2).bytes(&[3]).unsigned(4);
    assert_eq!(&b.body()[9..], [1, 2, 0, 3, 4, 0]);
}

//! DWARF location expressions, evaluated against a frame made here.

use turbonop_core::Endian;
use turbonop_eval::Unavailable;
use turbonop_eval::expr::{Frame, Loc, Piece, evaluate};

struct F;

impl Frame for F {
    fn register(&self, n: u16) -> Result<u64, Unavailable> {
        match n {
            6 => Ok(0x7000),
            7 => Ok(0x6000),
            3 => Err(Unavailable::RegisterClobbered(3)),
            _ => Err(Unavailable::RegisterUnknown(n)),
        }
    }
    fn memory(&self, address: u64, len: u8) -> Result<u64, Unavailable> {
        if address == 0x6ff0 {
            Ok(0x1234_5678_9abc_def0 & (u64::MAX >> (64 - 8 * u32::from(len))))
        } else {
            Err(Unavailable::MemoryNotCaptured {
                address,
                len: u64::from(len),
            })
        }
    }
    fn cfa(&self) -> Result<u64, Unavailable> {
        Ok(0x8000)
    }
    fn frame_base(&self) -> Result<u64, Unavailable> {
        Ok(0x7100)
    }
    fn bias(&self) -> u64 {
        0x5555_0000_0000
    }
}

fn one(e: &[u8]) -> Result<Loc, Unavailable> {
    let p = evaluate(e, Endian::Little, 8, &F)?;
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].size, None);
    Ok(p[0].loc.clone())
}

#[test]
fn places() {
    // DW_OP_reg5; DW_OP_regx 33; DW_OP_breg6 -16; DW_OP_fbreg 8;
    // DW_OP_call_frame_cfa; DW_OP_addr.
    assert_eq!(one(&[0x55]), Ok(Loc::Register(5)));
    assert_eq!(one(&[0x90, 33]), Ok(Loc::Register(33)));
    assert_eq!(one(&[0x76, 0x70]), Ok(Loc::Memory(0x6ff0)));
    assert_eq!(one(&[0x91, 8]), Ok(Loc::Memory(0x7108)));
    assert_eq!(one(&[0x9c]), Ok(Loc::Memory(0x8000)));
    let mut addr = vec![0x03];
    addr.extend(0x4010u64.to_le_bytes());
    assert_eq!(one(&addr), Ok(Loc::Memory(0x5555_0000_4010)));
}

#[test]
fn computed() {
    // DW_OP_breg6 -16; DW_OP_deref: the address held at rbp-16.
    assert_eq!(
        one(&[0x76, 0x70, 0x06]),
        Ok(Loc::Memory(0x1234_5678_9abc_def0))
    );
    // DW_OP_breg6 -16; DW_OP_deref_size 2; DW_OP_stack_value.
    assert_eq!(one(&[0x76, 0x70, 0x94, 2, 0x9f]), Ok(Loc::Value(0xdef0)));
    // DW_OP_lit7; DW_OP_lit5; DW_OP_minus; DW_OP_constu 10; DW_OP_mul;
    // DW_OP_stack_value: (7 - 5) * 10.
    assert_eq!(
        one(&[0x37, 0x35, 0x1c, 0x10, 10, 0x1e, 0x9f]),
        Ok(Loc::Value(20))
    );
    // DW_OP_const1s -1; DW_OP_lit1; DW_OP_shra... ; DW_OP_lit3;
    // DW_OP_lit4; DW_OP_lt (signed); DW_OP_stack_value.
    assert_eq!(one(&[0x33, 0x34, 0x2d, 0x9f]), Ok(Loc::Value(1)));
    assert_eq!(
        one(&[0x09, 0xff, 0x31, 0x26, 0x9f]),
        Ok(Loc::Value(u64::MAX))
    );
    // DW_OP_implicit_value 3 bytes.
    assert_eq!(one(&[0x9e, 3, 1, 2, 3]), Ok(Loc::Implicit(vec![1, 2, 3])));
    // DW_OP_lit1; DW_OP_bra +1 (skips DW_OP_lit9); DW_OP_lit2;
    // DW_OP_stack_value.
    assert_eq!(
        one(&[0x31, 0x28, 1, 0, 0x39, 0x32, 0x9f]),
        Ok(Loc::Value(2))
    );
}

#[test]
fn pieces() {
    // DW_OP_reg0; DW_OP_piece 4; (empty) DW_OP_piece 4; DW_OP_breg7 0;
    // DW_OP_piece 8.
    let p = evaluate(
        &[0x50, 0x93, 4, 0x93, 4, 0x77, 0, 0x93, 8],
        Endian::Little,
        8,
        &F,
    )
    .unwrap();
    assert_eq!(
        p,
        [
            Piece {
                loc: Loc::Register(0),
                size: Some(4),
                bit_offset: 0
            },
            Piece {
                loc: Loc::Empty,
                size: Some(4),
                bit_offset: 0
            },
            Piece {
                loc: Loc::Memory(0x6000),
                size: Some(8),
                bit_offset: 0
            },
        ]
    );
}

#[test]
fn unavailable() {
    // A register the frame lost; memory not captured; the caller's.
    assert_eq!(one(&[0x73, 0]), Err(Unavailable::RegisterClobbered(3)));
    assert!(matches!(
        one(&[0x77, 0, 0x06]),
        Err(Unavailable::MemoryNotCaptured {
            address: 0x6000,
            ..
        })
    ));
    assert_eq!(one(&[0xa3, 1, 0x55, 0x9f]), Err(Unavailable::EntryValue));
    assert_eq!(one(&[0xf3, 1, 0x55, 0x9f]), Err(Unavailable::EntryValue));
    assert_eq!(one(&[0xe0]), Err(Unavailable::ThreadLocal));
    // Malformed: an empty stack, a truncated operand, a division by 0, a
    // loop forever.
    assert_eq!(one(&[0x06]), Err(Unavailable::Malformed));
    assert_eq!(one(&[0x0e, 1, 2]), Err(Unavailable::Malformed));
    assert_eq!(one(&[0x31, 0x30, 0x1b]), Err(Unavailable::Malformed));
    assert_eq!(one(&[0x2f, 0xfd, 0xff]), Err(Unavailable::Malformed));
    assert_eq!(one(&[]), Err(Unavailable::Malformed));
}

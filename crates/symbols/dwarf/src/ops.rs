//! The operations of a DWARF expression, decoded with their operands and
//! where each lies, for tools that copy or rewrite expressions (rather than
//! evaluate them: see [`crate::expr`]).
//!
//! Every operation of DWARF 5 and GNU's extensions is known; an unknown
//! opcode (or a truncated operand) ends decoding with an error, since its
//! operands' length can't be known.

use alloc::vec::Vec;

use turbonop_core::Endian;

use crate::Reader;
use crate::consts::*;

/// One operand of an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operand<'d> {
    /// An unsigned constant (fixed-size or ULEB128).
    Unsigned(u64),
    /// A signed constant (fixed-size or SLEB128).
    Signed(i64),
    /// A target address of the unit's address size (`DW_OP_addr`), at
    /// `at` in the expression.
    Address { at: usize, value: u64 },
    /// A `.debug_addr` index (`DW_OP_addrx`, `DW_OP_constx`, and GNU's).
    AddressIndex(u64),
    /// A section offset of `.debug_info` (a DIE of any unit: `DW_OP_call_ref`,
    /// `DW_OP_implicit_pointer`, `DW_OP_GNU_variable_value`), offset-sized,
    /// at `at`.
    InfoOffset { at: usize, value: u64 },
    /// A DIE of the expression's unit, by its offset from the unit's start:
    /// a ULEB128 (`DW_OP_convert` and the typed operations; 0 for
    /// `convert`'s generic type) or `size` fixed bytes (`DW_OP_call2`,
    /// `call4`, `GNU_parameter_ref`).
    UnitDie { value: u64, size: Option<u8> },
    /// A branch's signed 2-byte offset, from the end of the operation.
    Branch(i16),
    /// Literal bytes (`DW_OP_implicit_value`, `DW_OP_const_type`'s value).
    Bytes(&'d [u8]),
    /// A nested expression (`DW_OP_entry_value`).
    Expression(&'d [u8]),
}

/// A decoded operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Op<'d> {
    pub code: u8,
    /// Where it starts and ends in the expression.
    pub start: usize,
    pub end: usize,
    pub operands: Vec<Operand<'d>>,
}

/// Why an expression couldn't be decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// An opcode whose operands aren't known.
    Unknown(u8),
    /// An operand runs past the end.
    Truncated(usize),
}

/// Decodes `expr` into its operations. `address_size` is the unit's,
/// `offset_size` 4 or 8 (64-bit DWARF), `version` the unit's (`DW_OP_call_ref`
/// in DWARF 2 is address-sized).
pub fn decode<'d>(
    expr: &'d [u8],
    endian: Endian,
    address_size: u8,
    offset_size: u8,
    version: u16,
) -> Result<Vec<Op<'d>>, DecodeError> {
    let mut out = Vec::new();
    let mut r = Reader::new(expr, 0, endian);
    while !r.is_empty() {
        let start = r.pos;
        let truncated = DecodeError::Truncated(start);
        let code = r.u8().ok_or(truncated)?;
        let mut operands = Vec::new();
        let mut push = |o: Option<Operand<'d>>| -> Result<(), DecodeError> {
            operands.push(o.ok_or(truncated)?);
            Ok(())
        };
        let a = usize::from(address_size);
        let ref_size = if version <= 2 {
            a
        } else {
            usize::from(offset_size)
        };
        match code {
            DW_OP_ADDR => {
                let at = r.pos;
                push(r.uint(a).map(|value| Operand::Address { at, value }))?;
            }
            DW_OP_CONST1U | DW_OP_PICK | DW_OP_DEREF_SIZE | DW_OP_XDEREF_SIZE => {
                push(r.uint(1).map(Operand::Unsigned))?;
            }
            DW_OP_CONST1S => push(r.uint(1).map(|v| Operand::Signed(crate::sext(v, 1) as i64)))?,
            DW_OP_CONST2U => push(r.uint(2).map(Operand::Unsigned))?,
            DW_OP_CONST2S => push(r.uint(2).map(|v| Operand::Signed(crate::sext(v, 2) as i64)))?,
            DW_OP_CONST4U => push(r.uint(4).map(Operand::Unsigned))?,
            DW_OP_CONST4S => push(r.uint(4).map(|v| Operand::Signed(crate::sext(v, 4) as i64)))?,
            DW_OP_CONST8U => push(r.uint(8).map(Operand::Unsigned))?,
            DW_OP_CONST8S => push(r.uint(8).map(|v| Operand::Signed(v as i64)))?,
            DW_OP_CONSTU | DW_OP_PLUS_UCONST | DW_OP_REGX | DW_OP_PIECE => {
                push(r.uleb().map(Operand::Unsigned))?;
            }
            DW_OP_CONSTS | DW_OP_FBREG => push(r.sleb().map(Operand::Signed))?,
            DW_OP_BREG0..=DW_OP_BREG31 => push(r.sleb().map(Operand::Signed))?,
            DW_OP_BREGX => {
                push(r.uleb().map(Operand::Unsigned))?;
                push(r.sleb().map(Operand::Signed))?;
            }
            DW_OP_BRA | DW_OP_SKIP => {
                push(
                    r.uint(2)
                        .and_then(|v| i16::try_from(crate::sext(v, 2) as i64).ok())
                        .map(Operand::Branch),
                )?;
            }
            DW_OP_CALL2 => push(r.uint(2).map(|value| Operand::UnitDie {
                value,
                size: Some(2),
            }))?,
            DW_OP_CALL4 | DW_OP_GNU_PARAMETER_REF => {
                push(r.uint(4).map(|value| Operand::UnitDie {
                    value,
                    size: Some(4),
                }))?;
            }
            DW_OP_CALL_REF | DW_OP_GNU_VARIABLE_VALUE => {
                let at = r.pos;
                push(
                    r.uint(ref_size)
                        .map(|value| Operand::InfoOffset { at, value }),
                )?;
            }
            DW_OP_IMPLICIT_POINTER | DW_OP_GNU_IMPLICIT_POINTER => {
                let at = r.pos;
                push(
                    r.uint(ref_size)
                        .map(|value| Operand::InfoOffset { at, value }),
                )?;
                push(r.sleb().map(Operand::Signed))?;
            }
            DW_OP_BIT_PIECE => {
                push(r.uleb().map(Operand::Unsigned))?;
                push(r.uleb().map(Operand::Unsigned))?;
            }
            DW_OP_IMPLICIT_VALUE => {
                let n = r.uleb().ok_or(truncated)?;
                push(Some(Operand::Unsigned(n)))?;
                push(r.bytes(n).map(Operand::Bytes))?;
            }
            DW_OP_ADDRX | DW_OP_CONSTX | DW_OP_GNU_ADDR_INDEX | DW_OP_GNU_CONST_INDEX => {
                push(r.uleb().map(Operand::AddressIndex))?;
            }
            DW_OP_ENTRY_VALUE | DW_OP_GNU_ENTRY_VALUE => {
                let n = r.uleb().ok_or(truncated)?;
                push(r.bytes(n).map(Operand::Expression))?;
            }
            DW_OP_CONST_TYPE | DW_OP_GNU_CONST_TYPE => {
                push(r.uleb().map(|value| Operand::UnitDie { value, size: None }))?;
                let n = r.u8().ok_or(truncated)?;
                push(Some(Operand::Unsigned(u64::from(n))))?;
                push(r.bytes(u64::from(n)).map(Operand::Bytes))?;
            }
            DW_OP_REGVAL_TYPE | DW_OP_GNU_REGVAL_TYPE => {
                push(r.uleb().map(Operand::Unsigned))?;
                push(r.uleb().map(|value| Operand::UnitDie { value, size: None }))?;
            }
            DW_OP_DEREF_TYPE | DW_OP_XDEREF_TYPE | DW_OP_GNU_DEREF_TYPE => {
                push(r.uint(1).map(Operand::Unsigned))?;
                push(r.uleb().map(|value| Operand::UnitDie { value, size: None }))?;
            }
            DW_OP_CONVERT | DW_OP_REINTERPRET | DW_OP_GNU_CONVERT | DW_OP_GNU_REINTERPRET => {
                push(r.uleb().map(|value| Operand::UnitDie { value, size: None }))?;
            }
            DW_OP_WASM_LOCATION => {
                let kind = r.u8().ok_or(truncated)?;
                push(Some(Operand::Unsigned(u64::from(kind))))?;
                if kind == 3 {
                    push(r.uint(4).map(Operand::Unsigned))?;
                } else {
                    push(r.uleb().map(Operand::Unsigned))?;
                }
            }
            // No operands.
            DW_OP_DEREF
            | DW_OP_DUP
            | DW_OP_DROP
            | DW_OP_OVER
            | DW_OP_SWAP
            | DW_OP_ROT
            | DW_OP_XDEREF
            | DW_OP_ABS
            | DW_OP_AND
            | DW_OP_DIV
            | DW_OP_MINUS
            | DW_OP_MOD
            | DW_OP_MUL
            | DW_OP_NEG
            | DW_OP_NOT
            | DW_OP_OR
            | DW_OP_PLUS
            | DW_OP_SHL
            | DW_OP_SHR
            | DW_OP_SHRA
            | DW_OP_XOR
            | DW_OP_EQ
            | DW_OP_GE
            | DW_OP_GT
            | DW_OP_LE
            | DW_OP_LT
            | DW_OP_NE
            | DW_OP_NOP
            | DW_OP_PUSH_OBJECT_ADDRESS
            | DW_OP_FORM_TLS_ADDRESS
            | DW_OP_CALL_FRAME_CFA
            | DW_OP_STACK_VALUE
            | DW_OP_GNU_PUSH_TLS_ADDRESS
            | DW_OP_GNU_UNINIT => {}
            DW_OP_LIT0..=DW_OP_LIT31 | DW_OP_REG0..=DW_OP_REG31 => {}
            _ => return Err(DecodeError::Unknown(code)),
        }
        out.push(Op {
            code,
            start,
            end: r.pos,
            operands,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn le(expr: &[u8]) -> Result<Vec<Op<'_>>, DecodeError> {
        decode(expr, Endian::Little, 8, 4, 5)
    }

    #[test]
    fn every_operand_kind() {
        // addr; fbreg -20; bregx 6 8; const2s -2; bra +3; entry_value
        // (reg5); implicit_pointer; convert; const_type; regval_type;
        // deref_type; call2; GNU_parameter_ref; addrx; implicit_value;
        // piece; stack_value.
        let mut e = vec![DW_OP_ADDR, 1, 2, 3, 4, 5, 6, 7, 8];
        e.extend_from_slice(&[DW_OP_FBREG, 0x6c]);
        e.extend_from_slice(&[DW_OP_BREGX, 6, 8]);
        e.extend_from_slice(&[DW_OP_CONST2S, 0xFE, 0xFF]);
        e.extend_from_slice(&[DW_OP_BRA, 3, 0]);
        e.extend_from_slice(&[DW_OP_ENTRY_VALUE, 1, 0x55]);
        e.extend_from_slice(&[DW_OP_IMPLICIT_POINTER, 0x10, 0, 0, 0, 0x7F]);
        e.extend_from_slice(&[DW_OP_CONVERT, 0x2A]);
        e.extend_from_slice(&[DW_OP_CONST_TYPE, 0x2A, 2, 0xAA, 0xBB]);
        e.extend_from_slice(&[DW_OP_REGVAL_TYPE, 3, 0x2A]);
        e.extend_from_slice(&[DW_OP_DEREF_TYPE, 4, 0x2A]);
        e.extend_from_slice(&[DW_OP_CALL2, 0x34, 0x12]);
        e.extend_from_slice(&[DW_OP_GNU_PARAMETER_REF, 1, 0, 0, 0]);
        e.extend_from_slice(&[DW_OP_ADDRX, 0x81, 1]);
        e.extend_from_slice(&[DW_OP_IMPLICIT_VALUE, 2, 9, 9]);
        e.extend_from_slice(&[DW_OP_PIECE, 4, DW_OP_STACK_VALUE]);
        let ops = le(&e).unwrap();
        let operands: Vec<Vec<Operand<'_>>> = ops.iter().map(|o| o.operands.clone()).collect();
        assert_eq!(
            operands,
            [
                vec![Operand::Address {
                    at: 1,
                    value: 0x0807_0605_0403_0201
                }],
                vec![Operand::Signed(-20)],
                vec![Operand::Unsigned(6), Operand::Signed(8)],
                vec![Operand::Signed(-2)],
                vec![Operand::Branch(3)],
                vec![Operand::Expression(&[0x55])],
                vec![
                    Operand::InfoOffset {
                        at: 24,
                        value: 0x10
                    },
                    Operand::Signed(-1)
                ],
                vec![Operand::UnitDie {
                    value: 0x2A,
                    size: None
                }],
                vec![
                    Operand::UnitDie {
                        value: 0x2A,
                        size: None
                    },
                    Operand::Unsigned(2),
                    Operand::Bytes(&[0xAA, 0xBB])
                ],
                vec![
                    Operand::Unsigned(3),
                    Operand::UnitDie {
                        value: 0x2A,
                        size: None
                    }
                ],
                vec![
                    Operand::Unsigned(4),
                    Operand::UnitDie {
                        value: 0x2A,
                        size: None
                    }
                ],
                vec![Operand::UnitDie {
                    value: 0x1234,
                    size: Some(2)
                }],
                vec![Operand::UnitDie {
                    value: 1,
                    size: Some(4)
                }],
                vec![Operand::AddressIndex(0x81)],
                vec![Operand::Unsigned(2), Operand::Bytes(&[9, 9])],
                vec![Operand::Unsigned(4)],
                vec![],
            ]
        );
        // Each operation's extent, end to end.
        assert_eq!(ops[0].start, 0);
        assert!(ops.windows(2).all(|w| w[0].end == w[1].start));
        assert_eq!(ops.last().unwrap().end, e.len());
    }

    #[test]
    fn sizes_follow_the_unit() {
        // DWARF 2's call_ref is address-sized; 64-bit DWARF's offsets 8.
        let e = [DW_OP_CALL_REF, 1, 0, 0, 0, 0, 0, 0, 0];
        let v2 = decode(&e, Endian::Little, 8, 4, 2).unwrap();
        assert_eq!(v2[0].operands, [Operand::InfoOffset { at: 1, value: 1 }]);
        let v64 = decode(&e, Endian::Little, 8, 8, 5).unwrap();
        assert_eq!(v64[0].end, 9);
        let v5 = decode(&e[..5], Endian::Little, 8, 4, 5).unwrap();
        assert_eq!(v5[0].end, 5);
        // A 4-byte address, big-endian.
        let e = [DW_OP_ADDR, 1, 2, 3, 4];
        let b = decode(&e, Endian::Big, 4, 4, 5).unwrap();
        assert_eq!(
            b[0].operands,
            [Operand::Address {
                at: 1,
                value: 0x0102_0304
            }]
        );
    }

    #[test]
    fn errors() {
        assert_eq!(le(&[0xE5]), Err(DecodeError::Unknown(0xE5)));
        assert_eq!(
            le(&[DW_OP_LIT0, DW_OP_CONST4U, 1]),
            Err(DecodeError::Truncated(1))
        );
        assert_eq!(
            le(&[DW_OP_ENTRY_VALUE, 5, 1]),
            Err(DecodeError::Truncated(0))
        );
        assert!(le(&[]).unwrap().is_empty());
    }
}

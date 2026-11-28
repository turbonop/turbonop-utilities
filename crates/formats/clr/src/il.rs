//! CIL (III): every opcode (one byte, and two with the `0xFE` prefix),
//! its operand, and a decoder of method bodies' code, instruction by
//! instruction, with tokens resolved to names.

use std::fmt::Write;

use crate::metadata::Metadata;
use crate::names::quote_utf16;
use crate::signature::parse_method_sig;
use crate::tables::{StandAloneSigRow, TableId, Token};

/// What follows an opcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OperandKind {
    /// Nothing.
    None,
    /// An argument or local's number, one byte.
    ShortVar,
    /// An argument or local's number, two bytes.
    Var,
    /// A signed byte.
    ShortI,
    /// A 32-bit integer.
    I,
    /// A 64-bit integer.
    I8,
    /// A 32-bit float.
    ShortR,
    /// A 64-bit float.
    R,
    /// A branch's signed byte offset (from the next instruction).
    ShortBrTarget,
    /// A branch's 32-bit offset.
    BrTarget,
    /// A count, then that many 32-bit offsets.
    Switch,
    /// A method token (MethodDef, MemberRef, MethodSpec).
    Method,
    /// A field token (Field, MemberRef).
    Field,
    /// A type token (TypeDef, TypeRef, TypeSpec).
    Type,
    /// A type, method, or field token (`ldtoken`).
    Tok,
    /// A user string token.
    String,
    /// A stand-alone signature token (`calli`).
    Sig,
}

impl OperandKind {
    /// Its name as System.Reflection.Emit's `OperandType` spells it.
    pub fn reflection_name(self) -> &'static str {
        match self {
            Self::None => "InlineNone",
            Self::ShortVar => "ShortInlineVar",
            Self::Var => "InlineVar",
            Self::ShortI => "ShortInlineI",
            Self::I => "InlineI",
            Self::I8 => "InlineI8",
            Self::ShortR => "ShortInlineR",
            Self::R => "InlineR",
            Self::ShortBrTarget => "ShortInlineBrTarget",
            Self::BrTarget => "InlineBrTarget",
            Self::Switch => "InlineSwitch",
            Self::Method => "InlineMethod",
            Self::Field => "InlineField",
            Self::Type => "InlineType",
            Self::Tok => "InlineTok",
            Self::String => "InlineString",
            Self::Sig => "InlineSig",
        }
    }

    /// Its fixed size (a switch's is 4 plus 4 a target).
    pub fn size(self) -> usize {
        match self {
            Self::None => 0,
            Self::ShortVar | Self::ShortI | Self::ShortBrTarget => 1,
            Self::Var => 2,
            Self::I8 | Self::R => 8,
            _ => 4,
        }
    }
}

/// An opcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OpCode {
    /// Its value: one byte, or `0xFE00` and the second.
    pub value: u16,
    /// Its name (`ldarg.0`, `call`, `unaligned.`).
    pub name: &'static str,
    /// Its operand.
    pub operand: OperandKind,
}

use OperandKind as K;

macro_rules! ops {
    ($( $v:literal $name:literal $k:ident ),* $(,)?) => {
        &[ $( OpCode { value: $v, name: $name, operand: K::$k } ),* ]
    };
}

/// Every opcode, by value.
pub const OPCODES: &[OpCode] = ops![
    0x00 "nop" None, 0x01 "break" None,
    0x02 "ldarg.0" None, 0x03 "ldarg.1" None, 0x04 "ldarg.2" None, 0x05 "ldarg.3" None,
    0x06 "ldloc.0" None, 0x07 "ldloc.1" None, 0x08 "ldloc.2" None, 0x09 "ldloc.3" None,
    0x0A "stloc.0" None, 0x0B "stloc.1" None, 0x0C "stloc.2" None, 0x0D "stloc.3" None,
    0x0E "ldarg.s" ShortVar, 0x0F "ldarga.s" ShortVar, 0x10 "starg.s" ShortVar,
    0x11 "ldloc.s" ShortVar, 0x12 "ldloca.s" ShortVar, 0x13 "stloc.s" ShortVar,
    0x14 "ldnull" None, 0x15 "ldc.i4.m1" None, 0x16 "ldc.i4.0" None, 0x17 "ldc.i4.1" None,
    0x18 "ldc.i4.2" None, 0x19 "ldc.i4.3" None, 0x1A "ldc.i4.4" None, 0x1B "ldc.i4.5" None,
    0x1C "ldc.i4.6" None, 0x1D "ldc.i4.7" None, 0x1E "ldc.i4.8" None,
    0x1F "ldc.i4.s" ShortI, 0x20 "ldc.i4" I, 0x21 "ldc.i8" I8, 0x22 "ldc.r4" ShortR,
    0x23 "ldc.r8" R, 0x25 "dup" None, 0x26 "pop" None, 0x27 "jmp" Method,
    0x28 "call" Method, 0x29 "calli" Sig, 0x2A "ret" None,
    0x2B "br.s" ShortBrTarget, 0x2C "brfalse.s" ShortBrTarget, 0x2D "brtrue.s" ShortBrTarget,
    0x2E "beq.s" ShortBrTarget, 0x2F "bge.s" ShortBrTarget, 0x30 "bgt.s" ShortBrTarget,
    0x31 "ble.s" ShortBrTarget, 0x32 "blt.s" ShortBrTarget, 0x33 "bne.un.s" ShortBrTarget,
    0x34 "bge.un.s" ShortBrTarget, 0x35 "bgt.un.s" ShortBrTarget, 0x36 "ble.un.s" ShortBrTarget,
    0x37 "blt.un.s" ShortBrTarget,
    0x38 "br" BrTarget, 0x39 "brfalse" BrTarget, 0x3A "brtrue" BrTarget, 0x3B "beq" BrTarget,
    0x3C "bge" BrTarget, 0x3D "bgt" BrTarget, 0x3E "ble" BrTarget, 0x3F "blt" BrTarget,
    0x40 "bne.un" BrTarget, 0x41 "bge.un" BrTarget, 0x42 "bgt.un" BrTarget,
    0x43 "ble.un" BrTarget, 0x44 "blt.un" BrTarget, 0x45 "switch" Switch,
    0x46 "ldind.i1" None, 0x47 "ldind.u1" None, 0x48 "ldind.i2" None, 0x49 "ldind.u2" None,
    0x4A "ldind.i4" None, 0x4B "ldind.u4" None, 0x4C "ldind.i8" None, 0x4D "ldind.i" None,
    0x4E "ldind.r4" None, 0x4F "ldind.r8" None, 0x50 "ldind.ref" None, 0x51 "stind.ref" None,
    0x52 "stind.i1" None, 0x53 "stind.i2" None, 0x54 "stind.i4" None, 0x55 "stind.i8" None,
    0x56 "stind.r4" None, 0x57 "stind.r8" None,
    0x58 "add" None, 0x59 "sub" None, 0x5A "mul" None, 0x5B "div" None, 0x5C "div.un" None,
    0x5D "rem" None, 0x5E "rem.un" None, 0x5F "and" None, 0x60 "or" None, 0x61 "xor" None,
    0x62 "shl" None, 0x63 "shr" None, 0x64 "shr.un" None, 0x65 "neg" None, 0x66 "not" None,
    0x67 "conv.i1" None, 0x68 "conv.i2" None, 0x69 "conv.i4" None, 0x6A "conv.i8" None,
    0x6B "conv.r4" None, 0x6C "conv.r8" None, 0x6D "conv.u4" None, 0x6E "conv.u8" None,
    0x6F "callvirt" Method, 0x70 "cpobj" Type, 0x71 "ldobj" Type, 0x72 "ldstr" String,
    0x73 "newobj" Method, 0x74 "castclass" Type, 0x75 "isinst" Type, 0x76 "conv.r.un" None,
    0x79 "unbox" Type, 0x7A "throw" None, 0x7B "ldfld" Field, 0x7C "ldflda" Field,
    0x7D "stfld" Field, 0x7E "ldsfld" Field, 0x7F "ldsflda" Field, 0x80 "stsfld" Field,
    0x81 "stobj" Type,
    0x82 "conv.ovf.i1.un" None, 0x83 "conv.ovf.i2.un" None, 0x84 "conv.ovf.i4.un" None,
    0x85 "conv.ovf.i8.un" None, 0x86 "conv.ovf.u1.un" None, 0x87 "conv.ovf.u2.un" None,
    0x88 "conv.ovf.u4.un" None, 0x89 "conv.ovf.u8.un" None, 0x8A "conv.ovf.i.un" None,
    0x8B "conv.ovf.u.un" None, 0x8C "box" Type, 0x8D "newarr" Type, 0x8E "ldlen" None,
    0x8F "ldelema" Type,
    0x90 "ldelem.i1" None, 0x91 "ldelem.u1" None, 0x92 "ldelem.i2" None, 0x93 "ldelem.u2" None,
    0x94 "ldelem.i4" None, 0x95 "ldelem.u4" None, 0x96 "ldelem.i8" None, 0x97 "ldelem.i" None,
    0x98 "ldelem.r4" None, 0x99 "ldelem.r8" None, 0x9A "ldelem.ref" None,
    0x9B "stelem.i" None, 0x9C "stelem.i1" None, 0x9D "stelem.i2" None, 0x9E "stelem.i4" None,
    0x9F "stelem.i8" None, 0xA0 "stelem.r4" None, 0xA1 "stelem.r8" None, 0xA2 "stelem.ref" None,
    0xA3 "ldelem" Type, 0xA4 "stelem" Type, 0xA5 "unbox.any" Type,
    0xB3 "conv.ovf.i1" None, 0xB4 "conv.ovf.u1" None, 0xB5 "conv.ovf.i2" None,
    0xB6 "conv.ovf.u2" None, 0xB7 "conv.ovf.i4" None, 0xB8 "conv.ovf.u4" None,
    0xB9 "conv.ovf.i8" None, 0xBA "conv.ovf.u8" None,
    0xC2 "refanyval" Type, 0xC3 "ckfinite" None, 0xC6 "mkrefany" Type,
    0xD0 "ldtoken" Tok, 0xD1 "conv.u2" None, 0xD2 "conv.u1" None, 0xD3 "conv.i" None,
    0xD4 "conv.ovf.i" None, 0xD5 "conv.ovf.u" None, 0xD6 "add.ovf" None, 0xD7 "add.ovf.un" None,
    0xD8 "mul.ovf" None, 0xD9 "mul.ovf.un" None, 0xDA "sub.ovf" None, 0xDB "sub.ovf.un" None,
    0xDC "endfinally" None, 0xDD "leave" BrTarget, 0xDE "leave.s" ShortBrTarget,
    0xDF "stind.i" None, 0xE0 "conv.u" None,
    0xFE00 "arglist" None, 0xFE01 "ceq" None, 0xFE02 "cgt" None, 0xFE03 "cgt.un" None,
    0xFE04 "clt" None, 0xFE05 "clt.un" None, 0xFE06 "ldftn" Method, 0xFE07 "ldvirtftn" Method,
    0xFE09 "ldarg" Var, 0xFE0A "ldarga" Var, 0xFE0B "starg" Var, 0xFE0C "ldloc" Var,
    0xFE0D "ldloca" Var, 0xFE0E "stloc" Var, 0xFE0F "localloc" None, 0xFE11 "endfilter" None,
    0xFE12 "unaligned." ShortI, 0xFE13 "volatile." None, 0xFE14 "tail." None,
    0xFE15 "initobj" Type, 0xFE16 "constrained." Type, 0xFE17 "cpblk" None,
    0xFE18 "initblk" None, 0xFE19 "no." ShortI, 0xFE1A "rethrow" None, 0xFE1C "sizeof" Type,
    0xFE1D "refanytype" None, 0xFE1E "readonly." None,
];

#[allow(clippy::cast_possible_truncation)]
const fn table(two_byte: bool) -> [u8; 256] {
    let mut t = [u8::MAX; 256];
    let mut i = 0;
    while i < OPCODES.len() {
        let v = OPCODES[i].value;
        if (v >> 8 == 0xFE) == two_byte {
            t[(v & 0xFF) as usize] = i as u8;
        }
        i += 1;
    }
    t
}

static ONE_BYTE: [u8; 256] = table(false);
static TWO_BYTE: [u8; 256] = table(true);

/// The opcode with value `value` (a byte, or `0xFE00` and the second).
pub fn opcode(value: u16) -> Option<&'static OpCode> {
    let i = match value >> 8 {
        0 => ONE_BYTE[value as usize],
        0xFE => TWO_BYTE[(value & 0xFF) as usize],
        _ => return None,
    };
    OPCODES.get(i as usize)
}

/// An operand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operand {
    /// None.
    None,
    /// An argument or local's number.
    Var(u16),
    /// An integer (`ShortI`, `I`, `I8`).
    Int(i64),
    /// A 32-bit float's bits.
    Float32(u32),
    /// A 64-bit float's bits.
    Float64(u64),
    /// A branch's target (an offset into the code, perhaps out of it).
    Target(i64),
    /// A switch's targets.
    Switch(Vec<i64>),
    /// A token.
    Token(Token),
}

/// An instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    /// Its offset in the code.
    pub offset: u32,
    /// Its size, operand included.
    pub size: u32,
    /// Its opcode.
    pub opcode: &'static OpCode,
    /// Its operand.
    pub operand: Operand,
}

/// What stopped or interrupted decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IlError {
    /// No opcode has this value (decoding goes on after it).
    Unknown {
        /// Where.
        offset: u32,
        /// The value (one byte, or `0xFE00` and the second).
        value: u16,
    },
    /// The code ends in an instruction (decoding stops): an `0xFE` with no
    /// second byte (`opcode` `None`), or an operand cut short.
    Truncated {
        /// Where the instruction starts.
        offset: u32,
        /// The opcode, if its bytes are there.
        opcode: Option<&'static OpCode>,
    },
}

/// Decodes `code` instruction by instruction: an unknown opcode gives an
/// error and decoding goes on from the next byte; a truncated instruction
/// gives an error and ends it.
pub fn decode(code: &[u8]) -> Instructions<'_> {
    Instructions {
        code,
        at: 0,
        done: false,
    }
}

/// The instructions of a method's code (see [`decode`]).
#[derive(Clone, Debug)]
pub struct Instructions<'c> {
    code: &'c [u8],
    at: usize,
    done: bool,
}

#[allow(clippy::cast_possible_truncation)]
fn le(b: &[u8]) -> u64 {
    b.iter().rev().fold(0u64, |v, &x| (v << 8) | u64::from(x))
}

impl Iterator for Instructions<'_> {
    type Item = Result<Instruction, IlError>;

    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.at >= self.code.len() {
            return None;
        }
        let start = self.at;
        let offset = start as u32;
        let mut value = u16::from(self.code[self.at]);
        self.at += 1;
        if value == 0xFE {
            let Some(&b) = self.code.get(self.at) else {
                self.done = true;
                return Some(Err(IlError::Truncated {
                    offset,
                    opcode: None,
                }));
            };
            value = 0xFE00 | u16::from(b);
            self.at += 1;
        }
        let Some(op) = opcode(value) else {
            return Some(Err(IlError::Unknown { offset, value }));
        };
        let truncated = |s: &mut Self| {
            s.done = true;
            Some(Err(IlError::Truncated {
                offset,
                opcode: Some(op),
            }))
        };
        let n = op.operand.size();
        let Some(raw) = self.code.get(self.at..self.at + n) else {
            return truncated(self);
        };
        self.at += n;
        let v = le(raw);
        let next = self.at as i64;
        let operand = match op.operand {
            K::None => Operand::None,
            K::ShortVar | K::Var => Operand::Var(v as u16),
            K::ShortI => Operand::Int(i64::from(v as u8 as i8)),
            K::I => Operand::Int(i64::from(v as u32 as i32)),
            K::I8 => Operand::Int(v as i64),
            K::ShortR => Operand::Float32(v as u32),
            K::R => Operand::Float64(v),
            K::ShortBrTarget => Operand::Target(next + i64::from(v as u8 as i8)),
            K::BrTarget => Operand::Target(next + i64::from(v as u32 as i32)),
            K::Switch => {
                let count = v as usize;
                let Some(bytes) = count
                    .checked_mul(4)
                    .and_then(|len| self.code.get(self.at..self.at.checked_add(len)?))
                else {
                    return truncated(self);
                };
                self.at += bytes.len();
                let base = self.at as i64;
                Operand::Switch(
                    bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|t| base + i64::from(i32::from_le_bytes(*t)))
                        .collect(),
                )
            }
            _ => Operand::Token(Token(v as u32)),
        };
        Some(Ok(Instruction {
            offset,
            size: (self.at - start) as u32,
            opcode: op,
            operand,
        }))
    }
}

/// A branch target as `IL_xxxx` (System.Reflection.Metadata's offsets
/// are 32-bit: one out of range wraps).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn label(target: i64) -> String {
    format!("IL_{:04x}", target as i32 as u32)
}

impl Metadata<'_> {
    /// An instruction's token operand as IL names it: a type, a member
    /// (`field `/`method ` before it for `ldtoken`), a quoted user
    /// string, or a stand-alone signature; the token in hex if it names
    /// no row, `undecodable` if what it names doesn't decode.
    pub fn token_operand(&self, token: Token, kind: OperandKind) -> String {
        let hex = || token.to_string();
        if kind == OperandKind::String {
            if !token.is_user_string() {
                return hex();
            }
            return match self.user_strings().get(token.rid()) {
                Ok(s) => quote_utf16(s.units()),
                Err(_) => hex(),
            };
        }
        let Some(table) = token.table() else {
            return hex();
        };
        if token.rid() == 0 || token.rid() > self.tables().rows(table) {
            return hex();
        }
        let tok = kind == OperandKind::Tok;
        let r = match table {
            TableId::TypeDef | TableId::TypeRef | TableId::TypeSpec => {
                Ok(self.type_token_name(token))
            }
            TableId::MethodDef | TableId::MethodSpec => self
                .member_name(token)
                .map(|m| if tok { format!("method {m}") } else { m }),
            TableId::Field => self
                .member_name(token)
                .map(|m| if tok { format!("field {m}") } else { m }),
            TableId::MemberRef => self.member_name(token).map(|m| {
                if !tok {
                    m
                } else if self.member_ref_is_field(token.rid()) {
                    format!("field {m}")
                } else {
                    format!("method {m}")
                }
            }),
            TableId::StandAloneSig => self
                .row::<StandAloneSigRow>(token.rid())
                .ok_or(turbonop_core::Error::new(
                    turbonop_core::ErrorKind::Malformed,
                    0,
                    "StandAloneSig",
                ))
                .and_then(|s| self.blob(s.signature))
                .and_then(parse_method_sig)
                .and_then(|m| self.method_sig_name(&m)),
            _ => return hex(),
        };
        r.unwrap_or_else(|_| "undecodable".into())
    }

    /// An instruction as IL text: `call instance void Ns.T::M()`,
    /// `br.s IL_0010`, `ldc.r4 0x40200000` (floats as their bits).
    pub fn instruction_text(&self, i: &Instruction) -> String {
        let mut s = String::from(i.opcode.name);
        match &i.operand {
            Operand::None => return s,
            Operand::Var(v) => {
                let _ = write!(s, " {v}");
            }
            Operand::Int(v) => {
                let _ = write!(s, " {v}");
            }
            Operand::Float32(b) => {
                let _ = write!(s, " {b:#010x}");
            }
            Operand::Float64(b) => {
                let _ = write!(s, " {b:#018x}");
            }
            Operand::Target(t) => {
                let _ = write!(s, " {}", label(*t));
            }
            Operand::Switch(ts) => {
                let list: Vec<String> = ts.iter().map(|&t| label(t)).collect();
                let _ = write!(s, " ({})", list.join(", "));
            }
            Operand::Token(t) => {
                let _ = write!(s, " {}", self.token_operand(*t, i.opcode.operand));
            }
        }
        s
    }
}

#[cfg(test)]
#[allow(clippy::cast_possible_truncation)]
mod tests {
    use super::*;

    #[test]
    fn table_is_consistent() {
        let mut seen = std::collections::HashSet::new();
        for op in OPCODES {
            assert!(seen.insert(op.value), "{:#x} twice", op.value);
            assert_eq!(opcode(op.value), Some(op));
        }
        assert_eq!(OPCODES.len(), 219);
        assert!(opcode(0x24).is_none() && opcode(0xFE08).is_none() && opcode(0xFF).is_none());
        assert!(opcode(0x1234).is_none());
    }

    #[test]
    fn decodes_operands() {
        let code = [
            0x00, // nop
            0x1F, 0xFB, // ldc.i4.s -5
            0x20, 0x78, 0x56, 0x34, 0x12, // ldc.i4
            0x21, 1, 0, 0, 0, 0, 0, 0, 0x80, // ldc.i8
            0x22, 0, 0, 0x20, 0x40, // ldc.r4 2.5
            0x2B, 0xFE, // br.s -2 (to itself)
            0x45, 2, 0, 0, 0, 1, 0, 0, 0, 0xF0, 0xFF, 0xFF, 0xFF, // switch
            0xFE, 0x0C, 0x03, 0x01, // ldloc 259
            0x28, 1, 0, 0, 0x0A, // call 0x0A000001
            0xFE, 0x12, 0x04, // unaligned. 4
        ];
        let ins: Vec<Instruction> = decode(&code).map(Result::unwrap).collect();
        let ops: Vec<(u32, &str, Operand)> = ins
            .iter()
            .map(|i| (i.offset, i.opcode.name, i.operand.clone()))
            .collect();
        assert_eq!(
            ops,
            [
                (0, "nop", Operand::None),
                (1, "ldc.i4.s", Operand::Int(-5)),
                (3, "ldc.i4", Operand::Int(0x1234_5678)),
                (8, "ldc.i8", Operand::Int(i64::MIN + 1)),
                (17, "ldc.r4", Operand::Float32(0x4020_0000)),
                (22, "br.s", Operand::Target(22)),
                (24, "switch", Operand::Switch(vec![38, 21])),
                (37, "ldloc", Operand::Var(259)),
                (41, "call", Operand::Token(Token(0x0A00_0001))),
                (46, "unaligned.", Operand::Int(4)),
            ]
        );
        assert_eq!(ins[6].size, 13);
        assert_eq!(label(-1), "IL_ffffffff");
    }

    #[test]
    fn damage() {
        // An unknown opcode, then a good one.
        let r: Vec<_> = decode(&[0x24, 0x2A]).collect();
        assert_eq!(
            r[0],
            Err(IlError::Unknown {
                offset: 0,
                value: 0x24
            })
        );
        assert_eq!(r[1].as_ref().unwrap().opcode.name, "ret");
        // A lone 0xFE.
        let r: Vec<_> = decode(&[0x00, 0xFE]).collect();
        assert_eq!(
            r[1],
            Err(IlError::Truncated {
                offset: 1,
                opcode: None
            })
        );
        // Truncated operands, and a switch whose count overruns.
        for code in [
            &[0x20, 1, 2][..],
            &[0x45, 0xFF, 0xFF, 0xFF, 0xFF, 0],
            &[0x45, 1, 0, 0, 0],
        ] {
            let r: Vec<_> = decode(code).collect();
            assert_eq!(r.len(), 1);
            assert!(matches!(
                r[0],
                Err(IlError::Truncated {
                    offset: 0,
                    opcode: Some(_)
                })
            ));
        }
        // Every byte value: never a panic.
        for b in 0..=255u8 {
            for c in 0..=255u8 {
                let _ = decode(&[b, c, 0, 0]).count();
            }
        }
    }
}

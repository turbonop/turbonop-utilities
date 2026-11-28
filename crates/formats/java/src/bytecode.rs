//! JVM bytecode (JVMS 6.5): every opcode, decoded an instruction at a time,
//! with `wide`'s forms and the switches' padding.

use turbonop_core::{Error, ErrorKind, Result};

/// An instruction's operand layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OperandKind {
    /// None.
    None,
    /// A signed byte (`bipush`).
    Byte,
    /// A signed short (`sipush`).
    Short,
    /// A one-byte constant pool index (`ldc`).
    Constant1,
    /// A two-byte constant pool index (`ldc_w`, field and method
    /// instructions, `new`, `checkcast`, ...).
    Constant2,
    /// A local variable's slot: a byte, or with `wide`, two.
    Local,
    /// `iinc`: a slot and a signed byte (with `wide`, two and two).
    Iinc,
    /// A signed two-byte branch offset.
    Branch2,
    /// A signed four-byte branch offset.
    Branch4,
    /// `invokeinterface`: an index, a count, and a zero.
    InvokeInterface,
    /// `invokedynamic`: an index and two zeros.
    InvokeDynamic,
    /// `newarray`: an element type code.
    NewArray,
    /// `multianewarray`: an index and dimensions.
    MultiANewArray,
    /// `tableswitch`.
    TableSwitch,
    /// `lookupswitch`.
    LookupSwitch,
    /// `wide`: the prefix of the next instruction.
    Wide,
}

/// An opcode's name and operand layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Opcode {
    /// Its value.
    pub value: u8,
    /// Its mnemonic.
    pub name: &'static str,
    /// Its operands.
    pub operand: OperandKind,
}

macro_rules! opcode_name {
    (r#return) => {
        "return"
    };
    ($n:ident) => {
        stringify!($n)
    };
}

macro_rules! opcodes {
    ($($v:literal $n:ident $k:ident,)*) => {
        /// Every defined opcode, by value (JVMS 6.5, and the reserved
        /// `breakpoint`, `impdep1`, and `impdep2`).
        pub const OPCODES: &[Opcode] = &[
            $(Opcode { value: $v, name: opcode_name!($n), operand: OperandKind::$k },)*
        ];
    };
}

opcodes! {
    0 nop None, 1 aconst_null None, 2 iconst_m1 None, 3 iconst_0 None, 4 iconst_1 None,
    5 iconst_2 None, 6 iconst_3 None, 7 iconst_4 None, 8 iconst_5 None, 9 lconst_0 None,
    10 lconst_1 None, 11 fconst_0 None, 12 fconst_1 None, 13 fconst_2 None, 14 dconst_0 None,
    15 dconst_1 None, 16 bipush Byte, 17 sipush Short, 18 ldc Constant1, 19 ldc_w Constant2,
    20 ldc2_w Constant2, 21 iload Local, 22 lload Local, 23 fload Local, 24 dload Local,
    25 aload Local, 26 iload_0 None, 27 iload_1 None, 28 iload_2 None, 29 iload_3 None,
    30 lload_0 None, 31 lload_1 None, 32 lload_2 None, 33 lload_3 None, 34 fload_0 None,
    35 fload_1 None, 36 fload_2 None, 37 fload_3 None, 38 dload_0 None, 39 dload_1 None,
    40 dload_2 None, 41 dload_3 None, 42 aload_0 None, 43 aload_1 None, 44 aload_2 None,
    45 aload_3 None, 46 iaload None, 47 laload None, 48 faload None, 49 daload None,
    50 aaload None, 51 baload None, 52 caload None, 53 saload None, 54 istore Local,
    55 lstore Local, 56 fstore Local, 57 dstore Local, 58 astore Local, 59 istore_0 None,
    60 istore_1 None, 61 istore_2 None, 62 istore_3 None, 63 lstore_0 None, 64 lstore_1 None,
    65 lstore_2 None, 66 lstore_3 None, 67 fstore_0 None, 68 fstore_1 None, 69 fstore_2 None,
    70 fstore_3 None, 71 dstore_0 None, 72 dstore_1 None, 73 dstore_2 None, 74 dstore_3 None,
    75 astore_0 None, 76 astore_1 None, 77 astore_2 None, 78 astore_3 None, 79 iastore None,
    80 lastore None, 81 fastore None, 82 dastore None, 83 aastore None, 84 bastore None,
    85 castore None, 86 sastore None, 87 pop None, 88 pop2 None, 89 dup None, 90 dup_x1 None,
    91 dup_x2 None, 92 dup2 None, 93 dup2_x1 None, 94 dup2_x2 None, 95 swap None, 96 iadd None,
    97 ladd None, 98 fadd None, 99 dadd None, 100 isub None, 101 lsub None, 102 fsub None,
    103 dsub None, 104 imul None, 105 lmul None, 106 fmul None, 107 dmul None, 108 idiv None,
    109 ldiv None, 110 fdiv None, 111 ddiv None, 112 irem None, 113 lrem None, 114 frem None,
    115 drem None, 116 ineg None, 117 lneg None, 118 fneg None, 119 dneg None, 120 ishl None,
    121 lshl None, 122 ishr None, 123 lshr None, 124 iushr None, 125 lushr None, 126 iand None,
    127 land None, 128 ior None, 129 lor None, 130 ixor None, 131 lxor None, 132 iinc Iinc,
    133 i2l None, 134 i2f None, 135 i2d None, 136 l2i None, 137 l2f None, 138 l2d None,
    139 f2i None, 140 f2l None, 141 f2d None, 142 d2i None, 143 d2l None, 144 d2f None,
    145 i2b None, 146 i2c None, 147 i2s None, 148 lcmp None, 149 fcmpl None, 150 fcmpg None,
    151 dcmpl None, 152 dcmpg None, 153 ifeq Branch2, 154 ifne Branch2, 155 iflt Branch2,
    156 ifge Branch2, 157 ifgt Branch2, 158 ifle Branch2, 159 if_icmpeq Branch2,
    160 if_icmpne Branch2, 161 if_icmplt Branch2, 162 if_icmpge Branch2, 163 if_icmpgt Branch2,
    164 if_icmple Branch2, 165 if_acmpeq Branch2, 166 if_acmpne Branch2, 167 goto Branch2,
    168 jsr Branch2, 169 ret Local, 170 tableswitch TableSwitch, 171 lookupswitch LookupSwitch,
    172 ireturn None, 173 lreturn None, 174 freturn None, 175 dreturn None, 176 areturn None,
    177 r#return None, 178 getstatic Constant2, 179 putstatic Constant2, 180 getfield Constant2,
    181 putfield Constant2, 182 invokevirtual Constant2, 183 invokespecial Constant2,
    184 invokestatic Constant2, 185 invokeinterface InvokeInterface,
    186 invokedynamic InvokeDynamic, 187 new Constant2, 188 newarray NewArray,
    189 anewarray Constant2, 190 arraylength None, 191 athrow None, 192 checkcast Constant2,
    193 instanceof Constant2, 194 monitorenter None, 195 monitorexit None, 196 wide Wide,
    197 multianewarray MultiANewArray, 198 ifnull Branch2, 199 ifnonnull Branch2,
    200 goto_w Branch4, 201 jsr_w Branch4, 202 breakpoint None, 254 impdep1 None,
    255 impdep2 None,
}

/// Opcode `value`, if it's defined.
pub fn opcode(value: u8) -> Option<&'static Opcode> {
    match value {
        0..=202 => OPCODES.get(usize::from(value)),
        254 => OPCODES.get(203),
        255 => OPCODES.get(204),
        _ => None,
    }
}

impl Opcode {
    /// Its mnemonic.
    pub fn mnemonic(&self) -> &'static str {
        self.name
    }
}

/// `newarray`'s element type names.
pub fn array_type_name(atype: u8) -> Option<&'static str> {
    Some(match atype {
        4 => "boolean",
        5 => "char",
        6 => "float",
        7 => "double",
        8 => "byte",
        9 => "short",
        10 => "int",
        11 => "long",
        _ => return None,
    })
}

/// A decoded instruction's operands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operand {
    /// None.
    None,
    /// `bipush`'s or `sipush`'s value.
    Value(i32),
    /// A constant pool index.
    Constant(u16),
    /// A local variable's slot.
    Local(u16),
    /// `iinc`'s slot and increment.
    Iinc {
        /// The slot.
        index: u16,
        /// The increment.
        delta: i16,
    },
    /// A branch's target, relative to the instruction.
    Branch(i32),
    /// `invokeinterface`'s method and argument count.
    InvokeInterface {
        /// The method.
        index: u16,
        /// The arguments' slots, plus one.
        count: u8,
    },
    /// `invokedynamic`'s call site.
    InvokeDynamic {
        /// The InvokeDynamic constant.
        index: u16,
    },
    /// `newarray`'s element type.
    NewArray(u8),
    /// `multianewarray`'s type and dimensions.
    MultiANewArray {
        /// The array class.
        index: u16,
        /// Dimensions to create.
        dims: u8,
    },
    /// `tableswitch`: its range and targets (relative).
    TableSwitch {
        /// The default target.
        default: i32,
        /// The lowest key.
        low: i32,
        /// The highest key.
        high: i32,
        /// The targets of `low..=high`.
        offsets: Vec<i32>,
    },
    /// `lookupswitch`: its keys and targets (relative).
    LookupSwitch {
        /// The default target.
        default: i32,
        /// Each key and its target.
        pairs: Vec<(i32, i32)>,
    },
}

/// One instruction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instruction {
    /// Its offset in the code.
    pub pc: u32,
    /// Its length in bytes (with `wide` and padding).
    pub length: u32,
    /// Its opcode (the one `wide` modifies, when it's wide).
    pub opcode: &'static Opcode,
    /// Whether `wide` prefixes it.
    pub wide: bool,
    /// Its operands.
    pub operand: Operand,
}

impl Instruction {
    /// Its mnemonic as javap writes it: a wide instruction's with `_w`
    /// (`iinc_w`, `lload_w`).
    pub fn mnemonic(&self) -> String {
        if self.wide {
            format!("{}_w", self.opcode.mnemonic())
        } else {
            self.opcode.mnemonic().to_string()
        }
    }

    /// A branch's absolute target (`Branch` operands only).
    pub fn target(&self) -> Option<i64> {
        match self.operand {
            Operand::Branch(d) => Some(i64::from(self.pc) + i64::from(d)),
            _ => None,
        }
    }

    /// Every target it may jump to (branches, both switches' defaults and
    /// cases), absolute.
    pub fn targets(&self) -> Vec<i64> {
        let pc = i64::from(self.pc);
        match &self.operand {
            Operand::Branch(d) => vec![pc + i64::from(*d)],
            Operand::TableSwitch {
                default, offsets, ..
            } => std::iter::once(default)
                .chain(offsets)
                .map(|&d| pc + i64::from(d))
                .collect(),
            Operand::LookupSwitch { default, pairs } => std::iter::once(*default)
                .chain(pairs.iter().map(|p| p.1))
                .map(|d| pc + i64::from(d))
                .collect(),
            _ => Vec::new(),
        }
    }
}

fn be16(code: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*code.get(at)?, *code.get(at + 1)?]))
}

fn be32(code: &[u8], at: usize) -> Option<i32> {
    let b = code.get(at..at + 4)?;
    Some(i32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// The instruction at `pc` in `code` (switches' padding is relative to the
/// code's start).
pub fn decode_at(code: &[u8], pc: usize) -> Result<Instruction> {
    let trunc = || Error::new(ErrorKind::Truncated, pc as u64, "instruction");
    let bad = |what| Error::new(ErrorKind::Malformed, pc as u64, what);
    let op = *code.get(pc).ok_or_else(trunc)?;
    let o = opcode(op).ok_or_else(|| bad("undefined opcode"))?;
    let u8_at = |at: usize| code.get(at).copied().ok_or_else(trunc);
    let u16_at = |at: usize| be16(code, at).ok_or_else(trunc);
    let i32_at = |at: usize| be32(code, at).ok_or_else(trunc);
    let (operand, length, wide, o) = match o.operand {
        OperandKind::None => (Operand::None, 1, false, o),
        OperandKind::Byte => (
            Operand::Value(i32::from(u8_at(pc + 1)?.cast_signed())),
            2,
            false,
            o,
        ),
        OperandKind::Short => (
            Operand::Value(i32::from(u16_at(pc + 1)?.cast_signed())),
            3,
            false,
            o,
        ),
        OperandKind::Constant1 => (Operand::Constant(u16::from(u8_at(pc + 1)?)), 2, false, o),
        OperandKind::Constant2 => (Operand::Constant(u16_at(pc + 1)?), 3, false, o),
        OperandKind::Local => (Operand::Local(u16::from(u8_at(pc + 1)?)), 2, false, o),
        OperandKind::Iinc => (
            Operand::Iinc {
                index: u16::from(u8_at(pc + 1)?),
                delta: i16::from(u8_at(pc + 2)?.cast_signed()),
            },
            3,
            false,
            o,
        ),
        OperandKind::Branch2 => (
            Operand::Branch(i32::from(u16_at(pc + 1)?.cast_signed())),
            3,
            false,
            o,
        ),
        OperandKind::Branch4 => (Operand::Branch(i32_at(pc + 1)?), 5, false, o),
        OperandKind::InvokeInterface => {
            let index = u16_at(pc + 1)?;
            let count = u8_at(pc + 3)?;
            u8_at(pc + 4)?;
            (Operand::InvokeInterface { index, count }, 5, false, o)
        }
        OperandKind::InvokeDynamic => {
            let index = u16_at(pc + 1)?;
            u16_at(pc + 3)?;
            (Operand::InvokeDynamic { index }, 5, false, o)
        }
        OperandKind::NewArray => (Operand::NewArray(u8_at(pc + 1)?), 2, false, o),
        OperandKind::MultiANewArray => (
            Operand::MultiANewArray {
                index: u16_at(pc + 1)?,
                dims: u8_at(pc + 3)?,
            },
            4,
            false,
            o,
        ),
        OperandKind::TableSwitch => {
            let at = (pc + 1).next_multiple_of(4);
            let default = i32_at(at)?;
            let low = i32_at(at + 4)?;
            let high = i32_at(at + 8)?;
            if high < low {
                return Err(bad("tableswitch's high below its low"));
            }
            let n = usize::try_from(i64::from(high) - i64::from(low) + 1)
                .map_err(|_| bad("tableswitch's range"))?;
            if n.checked_mul(4)
                .is_none_or(|need| at + 12 + need > code.len())
            {
                return Err(trunc());
            }
            let offsets: Vec<i32> = (0..n)
                .map(|i| be32(code, at + 12 + 4 * i).unwrap_or(0))
                .collect();
            (
                Operand::TableSwitch {
                    default,
                    low,
                    high,
                    offsets,
                },
                at + 12 + 4 * n - pc,
                false,
                o,
            )
        }
        OperandKind::LookupSwitch => {
            let at = (pc + 1).next_multiple_of(4);
            let default = i32_at(at)?;
            let npairs = i32_at(at + 4)?;
            let n = usize::try_from(npairs).map_err(|_| bad("lookupswitch's pair count"))?;
            if n.checked_mul(8)
                .is_none_or(|need| at + 8 + need > code.len())
            {
                return Err(trunc());
            }
            let pairs: Vec<(i32, i32)> = (0..n)
                .map(|i| {
                    let p = at + 8 + 8 * i;
                    (be32(code, p).unwrap_or(0), be32(code, p + 4).unwrap_or(0))
                })
                .collect();
            if pairs.windows(2).any(|w| w[0].0 >= w[1].0) {
                return Err(bad("lookupswitch's keys out of order"));
            }
            (
                Operand::LookupSwitch { default, pairs },
                at + 8 + 8 * n - pc,
                false,
                o,
            )
        }
        OperandKind::Wide => {
            let inner = u8_at(pc + 1)?;
            let w = opcode(inner).ok_or_else(|| bad("undefined opcode after wide"))?;
            match w.operand {
                OperandKind::Local => (Operand::Local(u16_at(pc + 2)?), 4, true, w),
                OperandKind::Iinc => (
                    Operand::Iinc {
                        index: u16_at(pc + 2)?,
                        delta: u16_at(pc + 4)?.cast_signed(),
                    },
                    6,
                    true,
                    w,
                ),
                _ => return Err(bad("wide before an instruction it can't widen")),
            }
        }
    };
    Ok(Instruction {
        pc: u32::try_from(pc).map_err(|_| bad("code too long"))?,
        length: u32::try_from(length).map_err(|_| bad("instruction too long"))?,
        opcode: o,
        wide,
        operand,
    })
}

/// The instructions of `code`, in order; one that can't be decoded ends
/// the iteration with its error.
pub fn decode(code: &[u8]) -> Instructions<'_> {
    Instructions {
        code,
        pc: 0,
        failed: false,
    }
}

/// An iterator over a method's instructions.
#[derive(Clone, Debug)]
pub struct Instructions<'d> {
    code: &'d [u8],
    pc: usize,
    failed: bool,
}

impl Iterator for Instructions<'_> {
    type Item = Result<Instruction>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.pc >= self.code.len() {
            return None;
        }
        match decode_at(self.code, self.pc) {
            Ok(i) => {
                self.pc += i.length as usize;
                Some(Ok(i))
            }
            Err(e) => {
                self.failed = true;
                Some(Err(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcode_table() {
        assert_eq!(OPCODES.len(), 205);
        for (i, o) in OPCODES.iter().enumerate().take(203) {
            assert_eq!(usize::from(o.value), i);
            assert_eq!(opcode(o.value), Some(o));
        }
        assert_eq!(opcode(177).unwrap().mnemonic(), "return");
        assert_eq!(opcode(254).unwrap().name, "impdep1");
        assert_eq!(opcode(255).unwrap().name, "impdep2");
        assert_eq!(opcode(203), None);
        assert_eq!(opcode(253), None);
        assert_eq!(array_type_name(10), Some("int"));
        assert_eq!(array_type_name(3), None);
    }

    fn all(code: &[u8]) -> Vec<Instruction> {
        decode(code).collect::<Result<_>>().unwrap()
    }

    #[test]
    fn operands_of_each_kind() {
        let code = [
            0x10, 0xFE, // bipush -2
            0x11, 0x80, 0x00, // sipush -32768
            0x12, 7, // ldc #7
            0x13, 1, 2, // ldc_w #258
            0x15, 9, // iload 9
            0x84, 3, 0xFF, // iinc 3, -1
            0x99, 0xFF, 0xF1, // ifeq -15
            0xC8, 0, 0, 0, 5, // goto_w +5
            0xB9, 0, 4, 2, 0, // invokeinterface #4, 2
            0xBA, 0, 5, 0, 0, // invokedynamic #5
            0xBC, 10, // newarray int
            0xC5, 0, 6, 3, // multianewarray #6, 3
            0xC4, 0x16, 1, 4, // wide lload 260
            0xC4, 0x84, 1, 6, 0xFC, 0x18, // wide iinc 262, -1000
            0xB1, // return
        ];
        let got = all(&code);
        let ops: Vec<Operand> = got.iter().map(|i| i.operand.clone()).collect();
        assert_eq!(
            ops,
            [
                Operand::Value(-2),
                Operand::Value(-32768),
                Operand::Constant(7),
                Operand::Constant(258),
                Operand::Local(9),
                Operand::Iinc {
                    index: 3,
                    delta: -1
                },
                Operand::Branch(-15),
                Operand::Branch(5),
                Operand::InvokeInterface { index: 4, count: 2 },
                Operand::InvokeDynamic { index: 5 },
                Operand::NewArray(10),
                Operand::MultiANewArray { index: 6, dims: 3 },
                Operand::Local(260),
                Operand::Iinc {
                    index: 262,
                    delta: -1000
                },
                Operand::None,
            ]
        );
        assert_eq!(got[6].target(), Some(15 - 15));
        assert_eq!(got[12].mnemonic(), "lload_w");
        assert_eq!(got[12].length, 4);
        assert_eq!(got[13].mnemonic(), "iinc_w");
        assert_eq!(got[13].length, 6);
        assert_eq!(got.last().unwrap().pc as usize, code.len() - 1);
    }

    #[test]
    fn switches_and_padding() {
        // At pc 1: padding to 4, default, low 1, high 2, two offsets.
        let mut code = vec![0x00, 0xAA, 0, 0];
        for v in [20i32, 1, 2, 10, 11] {
            code.extend_from_slice(&v.to_be_bytes());
        }
        // A lookupswitch at pc 24: no padding needed... (25 rounds to 28).
        code.push(0xAB);
        code.extend_from_slice(&[0, 0, 0]);
        for v in [30i32, 2, -5, 1, 100, 2] {
            code.extend_from_slice(&v.to_be_bytes());
        }
        let got = all(&code);
        assert_eq!(got.len(), 3);
        assert_eq!(
            got[1].operand,
            Operand::TableSwitch {
                default: 20,
                low: 1,
                high: 2,
                offsets: vec![10, 11]
            }
        );
        assert_eq!(got[1].length, 23);
        assert_eq!(got[1].targets(), [21, 11, 12]);
        assert_eq!(got[2].pc, 24);
        assert_eq!(
            got[2].operand,
            Operand::LookupSwitch {
                default: 30,
                pairs: vec![(-5, 1), (100, 2)]
            }
        );
        assert_eq!(got[2].targets(), [54, 25, 26]);
    }

    #[test]
    fn damage() {
        for code in [
            &[0xCB][..],                                          // undefined
            &[0x10],                                              // bipush cut
            &[0xC4, 0x60],                                        // wide iadd
            &[0xC4, 0x15, 0],                                     // wide iload cut
            &[0xAA, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 1], // high < low
            &[
                0xAA, 0, 0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0, 0, 0x7F, 0xFF, 0xFF, 0xFF,
            ], // huge
            &[0xAB, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF], // negative count
            &[
                0xAB, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0,
                0,
            ], // keys out of order
        ] {
            let r: Vec<_> = decode(code).collect();
            assert!(r.last().unwrap().is_err(), "{code:?}");
        }
        assert_eq!(decode(&[]).count(), 0);
    }
}

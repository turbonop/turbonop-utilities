//! The types of `tables.rs`, which tools/riscv/extract.py writes from
//! LLVM's TableGen records.

/// A subtarget feature (an extension, `64bit`, ...): its name, as LLVM's
/// `-mattr` names it, and the features it implies (by index).
#[derive(Debug)]
pub struct Feature {
    pub name: &'static str,
    pub implies: &'static [u16],
}

/// How a condition combines its features.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CondKind {
    All,
    Any,
    None,
}

/// A condition on the features (an assembler predicate): all, any, or none
/// of `features`, and none of `not`.
#[derive(Debug)]
pub struct Cond {
    pub kind: CondKind,
    pub features: &'static [u16],
    pub not: &'static [u16],
}

/// A register file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum File {
    /// The integer registers (and their views as pairs, and as floating
    /// point registers in Zfinx).
    X,
    /// The floating point registers.
    F,
    /// The vector registers (and their groups, by their first).
    V,
}

/// A register class: its file, and which of its registers it has (bit n:
/// register n).
#[derive(Debug)]
pub struct Class {
    pub name: &'static str,
    pub file: File,
    pub members: u32,
}

/// How an operand's field decodes (as LLVM's decoder methods do).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decode {
    /// A register: the field plus the base.
    RegSimple(u32),
    /// A register encoded in no field: this one.
    RegSingle(u32),
    /// A vector register group: the field, a multiple of the group's size.
    RegGroup(u32),
    /// A register pair: the field plus the base, even.
    RegPair(u32),
    /// Zcmp's s0-s7: 0 and 1 are x8 and x9, 2 to 7 are x18 to x23.
    RegS07(u32),
    /// A vector mask operand: 0 is v0 (masked), 1 none (unmasked).
    VMask(u32),
    /// Zcmp's `cm.mvsa01` second register: as `RegS07`, unlike the first.
    Mvsa01(u32),
    /// An immediate encoded in no field: this value.
    Fixed(i64),
    /// The field as it is.
    Raw {
        bits: u32,
        arg: i64,
    },
    /// An unsigned field, at least `arg`.
    UImm {
        bits: u32,
        arg: i64,
    },
    /// A signed field of `bits` bits.
    SImm {
        bits: u32,
        arg: i64,
    },
    SImmNonZero {
        bits: u32,
        arg: i64,
    },
    UImmNonZero {
        bits: u32,
        arg: i64,
    },
    /// A signed value of `bits` bits whose low `arg` bits are zero (and
    /// aren't encoded).
    SImmLsl {
        bits: u32,
        arg: i64,
    },
    /// A shift amount: less than 32 on RV32.
    Log2XLen {
        bits: u32,
        arg: i64,
    },
    Log2XLenNonZero {
        bits: u32,
        arg: i64,
    },
    /// The field plus one, at least `arg`.
    UImmPlus1 {
        bits: u32,
        arg: i64,
    },
    /// `c.lui`'s: nonzero, sign-extended from 6 bits to 20.
    CLui {
        bits: u32,
        arg: i64,
    },
    /// A rounding mode: not 5 or 6.
    Frm {
        bits: u32,
        arg: i64,
    },
    /// Zcmp's register list: 4 (`{ra}`) and up.
    Rlist {
        bits: u32,
        arg: i64,
    },
}

/// How an operand prints (as LLVM's printer methods do).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Print {
    Operand,
    Branch,
    Csr,
    Fence,
    Frm,
    FrmLegacy,
    FpImm,
    ZeroOffsetMem,
    VTypeI,
    RegList,
    StackAdj,
    NegStackAdj,
    VMask,
    VScale,
    RegReg,
}

/// An operand kind: its class (registers), and how it decodes and prints.
#[derive(Debug)]
pub struct Kind {
    pub name: &'static str,
    pub class: Option<u16>,
    pub decode: Decode,
    pub print: Print,
}

/// An instruction's operand: its name (as its syntax names it), its kind,
/// the fields it's encoded in (instruction bit, field bit, length), its
/// value if the encoding fixes it, and the operand it's tied to (whose
/// fields it's decoded from) if it has none of its own.
#[derive(Debug)]
pub struct Op {
    pub name: &'static str,
    pub kind: u16,
    pub fields: &'static [(u8, u8, u8)],
    pub constant: Option<u32>,
    pub tied: Option<u8>,
}

/// An instruction: its LLVM name, size in bytes, decoder table, fixed
/// bits, the features it needs (an index into `PREDICATES`), how many
/// operands it writes, its syntax
/// (operands as `$name` or `${name}`), and its operands.
#[derive(Debug)]
pub struct Inst {
    pub name: &'static str,
    pub size: u8,
    pub space: u8,
    pub mask: u32,
    pub value: u32,
    pub pred: u16,
    /// How many of its operands (the first) it writes.
    pub defs: u8,
    pub asm: &'static str,
    pub ops: &'static [Op],
}

/// A register: in a file, or none.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Reg {
    None,
    X(u8),
    F(u8),
    V(u8),
}

/// What an immediate may be: signed or not, of `bits` bits, with its low
/// `zeros` bits zero, and nonzero or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImmRange {
    pub signed: bool,
    pub bits: u32,
    pub zeros: u32,
    pub nonzero: bool,
}

/// A condition on an instruction's operands (by index).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// Two operands are the same register.
    Tied(u8, u8),
    /// An operand's register is in a class.
    Class(u8, u16),
    /// An operand is this register.
    Reg(u8, Reg),
    /// An operand is this immediate.
    Imm(u8, i64),
    /// An operand is an immediate in this range.
    ImmFits(u8, ImmRange),
}

/// An alias's operand: the instruction operand it prints, and how, if
/// not as the instruction would.
#[derive(Clone, Copy, Debug)]
pub struct Ref {
    pub op: u8,
    pub print: Option<Print>,
}

/// An alias LLVM prints an instruction as, when its operands pass its
/// checks: its syntax (operands as `$0`, `$1`, ... of `refs`).
#[derive(Debug)]
pub struct Alias {
    pub inst: u16,
    pub pred: u16,
    pub asm: &'static str,
    pub checks: &'static [Check],
    pub refs: &'static [Ref],
}

/// An operand of a full instruction built from a compressed one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Build {
    From(u8),
    Reg(Reg),
    Imm(i64),
}

/// A compressed instruction's full form: when its operands pass the
/// checks, the full instruction, its operands built so.
#[derive(Debug)]
pub struct Uncompress {
    pub from: u16,
    pub to: u16,
    pub pred: u16,
    pub checks: &'static [Check],
    pub build: &'static [Build],
}

/// A CSR: its number, name, the features its name needs, and whether the
/// name is RV32's only.
#[derive(Debug)]
pub struct Csr {
    pub encoding: u16,
    pub name: &'static str,
    pub features: &'static [u16],
    pub rv32_only: bool,
}

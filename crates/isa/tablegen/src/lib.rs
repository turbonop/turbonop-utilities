//! Instruction decoding driven by tables generated from LLVM's TableGen
//! records (`llvm-tblgen --dump-json` of a target): each instruction's
//! fixed bits, the features it needs, its operands' fields and how they
//! decode, its syntax; the aliases LLVM prints instructions as; and, for
//! targets with compressed instructions, their full forms. A target's
//! crate (`hexcavator-riscv`, `hexcavator-loongarch`,
//! `hexcavator-powerpc`) holds its generated
//! [`Target`], how its features are named, and how its operands print.
//!
//! [`decode`] finds an encoding's instruction as LLVM's disassembler does:
//! the candidates its key bits select, in their decoder tables' order and
//! the most specific first, the first whose fixed bits match, whose
//! features are on, and whose operands decode. [`write`] writes an
//! instruction's syntax, or an alias's that applies, each operand as the
//! target says. Neither panics.

#![no_std]

extern crate alloc;

use alloc::string::String;

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

/// A register: its file (the target's numbering of them) and number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Reg {
    pub file: u8,
    pub n: u8,
}

impl Reg {
    /// No register (an optional operand left out: an unmasked vector
    /// operation's mask).
    pub const NONE: Reg = Reg {
        file: u8::MAX,
        n: 0,
    };

    /// Register `n` of `file`.
    pub const fn new(file: u8, n: u8) -> Reg {
        Reg { file, n }
    }

    /// Whether it is no register.
    pub fn is_none(self) -> bool {
        self.file == u8::MAX
    }
}

/// A register class: its file, which of its registers it has (bit n:
/// register n, for those below 64), and its registers in its order (the
/// order LLVM's decoders index some classes by).
#[derive(Debug)]
pub struct Class {
    pub name: &'static str,
    pub file: u8,
    pub members: u64,
    pub order: &'static [u8],
}

/// How an operand's field decodes (as LLVM's decoder methods do; each
/// target's extractor names its methods so).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decode {
    /// A register: the field plus the base.
    RegSimple(u32),
    /// A register encoded in no field: this one.
    RegSingle(u32),
    /// A register group: the field, a multiple of the group's size.
    RegGroup(u32),
    /// A register pair: the field plus the base, even.
    RegPair(u32),
    /// RISC-V's Zcmp s0-s7: 0 and 1 are x8 and x9, 2 to 7 x18 to x23.
    RegS07(u32),
    /// RISC-V's vector mask operand: 0 is v0 (masked), 1 none (unmasked).
    VMask(u32),
    /// RISC-V's `cm.mvsa01` second register: as `RegS07`, unlike the
    /// first.
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
    /// An unsigned field plus `arg`.
    UImmAdd {
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
    /// A shift amount: less than 32 without `64bit`.
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
    /// RISC-V's `c.lui`: nonzero, sign-extended from 6 bits to 20.
    CLui {
        bits: u32,
        arg: i64,
    },
    /// RISC-V's rounding mode: not 5 or 6.
    Frm {
        bits: u32,
        arg: i64,
    },
    /// RISC-V's Zcmp register list: 4 (`{ra}`) and up.
    Rlist {
        bits: u32,
        arg: i64,
    },
    /// A field that must be 0 (PowerPC's `immZero`).
    Zero {
        bits: u32,
        arg: i64,
    },
    /// PowerPC's hash store and check displacement: -8 to -512, by 8.
    RiHash {
        bits: u32,
        arg: i64,
    },
    /// PowerPC's one-hot condition register field (`mtocrf`'s): 0x80 is
    /// cr0, 0x01 cr7; the argument is the fields' register file.
    CrBitM(u32),
    /// A register pair by its even field: register field / 2, plus the
    /// base (PowerPC's VSX pairs).
    RegHalf(u32),
    /// The class's register the field indexes, in the class's order
    /// (MIPS's `getReg(Decoder, ClassID, RegNo)`); below this many, if it
    /// isn't 0 (MIPS's MSA control registers: 8 of the class's 32).
    RegIndex(u32),
    /// The register the field must be (MIPS's hardware register 29).
    RegOnly(u32),
    /// MIPS's coprocessor 0 register: a register and its select (`reg <<
    /// 5 | sel`, the select under 8), the class's `sel * 32 + reg`th.
    RegSelect(u32),
    /// An unsigned field shifted left `arg` bits.
    UImmLsl {
        bits: u32,
        arg: i64,
    },
    /// A branch's offset from the instruction: a signed value of `bits`
    /// bits whose low `arg` bits are zero (and aren't encoded), plus 4
    /// (MIPS's: from its delay slot).
    BranchPlus4 {
        bits: u32,
        arg: i64,
    },
    /// MIPS's `ins` size: the field (its last bit) less operand `arg`
    /// (its first), plus 1.
    InsSize {
        bits: u32,
        arg: i64,
    },
    /// The field, `arg` if it is 0 (microMIPS's 16-bit shifts: 0 is 8).
    ZeroIs {
        bits: u32,
        arg: i64,
    },
    /// The field, -1 if all its `bits` are set (microMIPS's `li16`, and
    /// `lbu16`'s offset).
    MaxIsMinus1 {
        bits: u32,
        arg: i64,
    },
    /// microMIPS's `andi16` immediate: one of 16 masks, by the field.
    Andi16 {
        bits: u32,
        arg: i64,
    },
    /// microMIPS's `addiur2` immediate: 1 (0), -1 (7), else the field
    /// times 4.
    Addiur2 {
        bits: u32,
        arg: i64,
    },
    /// microMIPS's `addiusp` immediate: a signed 9-bit field times 4,
    /// whose four values nearest 0 (which other instructions encode) are
    /// instead 256, 257, -258, and -257.
    Simm9Sp {
        bits: u32,
        arg: i64,
    },
    /// microMIPS's register lists (for `lwm`, `swm`): `arg` 0 the 32-bit
    /// instructions' (a count of $16 on, to $fp as the ninth, under 10
    /// and not 0; bit 4 $ra), 1 the 16-bit ones' (one to four of $16 on,
    /// and $ra). Decoded alike: the count of registers from $16, and bit 4
    /// for $ra.
    MipsRegList {
        bits: u32,
        arg: i64,
    },
    /// microMIPS's `movep` destinations: of the field's pair of
    /// registers, the first (`arg` 0) or the second (1).
    MoveP {
        bits: u32,
        arg: i64,
    },
}

/// microMIPS's `andi16` masks, by their encoding (LLVM's DecodeANDI16Imm).
const ANDI16: [i64; 16] = [
    128, 1, 2, 3, 4, 7, 8, 15, 16, 31, 32, 63, 64, 255, 32768, 65535,
];

/// microMIPS's `movep` register pairs, by their encoding (LLVM's
/// DecodeMovePRegPair).
const MOVEP: [(u64, u64); 8] = [
    (5, 6),
    (5, 7),
    (6, 7),
    (4, 21),
    (4, 22),
    (4, 5),
    (4, 6),
    (4, 7),
];

/// How an operand prints (as LLVM's printer methods do; the target's
/// crate prints each).
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
    /// LoongArch's atomic memory operand (`$rj`, printed bare).
    AtomicMem,
    /// A displacement and its base register (PowerPC's `8(1)`): this
    /// operand and the next.
    MemRegImm,
    /// A base and an index register (PowerPC's `0, 5`): this operand and
    /// the next.
    MemRegReg,
    /// An absolute branch target.
    AbsBranch,
    /// PowerPC's one-hot condition register field.
    CrBitM,
    /// An unsigned immediate of `bits` bits above `offset`, as MIPS's
    /// printer writes it (`printUImm<Bits, Offset>`).
    UImm {
        bits: u32,
        offset: u32,
    },
    /// MIPS's memory operand: the next operand (an offset), then this
    /// one (its base) in parentheses.
    Mem,
    /// MIPS's coprocessor 0 register: its number, then its select.
    Cop0,
    /// MIPS's floating point condition (`f`, `un`, `eq`, ...).
    Fcc,
    /// The instruction's operand this far from its last.
    FromEnd(u32),
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
/// bits, the features it needs (an index into the predicates), how many
/// operands it writes, its syntax (operands as `$name`, `${name}`, or
/// `${name:modifier}`), and its operands.
#[derive(Debug)]
pub struct Inst {
    pub name: &'static str,
    pub size: u8,
    pub space: u8,
    pub mask: u64,
    pub value: u64,
    pub pred: u16,
    pub defs: u8,
    pub asm: &'static str,
    pub ops: &'static [Op],
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

/// A target's tables.
#[derive(Debug)]
pub struct Target {
    pub features: &'static [Feature],
    pub predicates: &'static [&'static [Cond]],
    pub classes: &'static [Class],
    pub kinds: &'static [Kind],
    pub instructions: &'static [Inst],
    pub aliases: &'static [Alias],
    pub uncompress: &'static [Uncompress],
    /// The instructions each value of a 16-bit encoding's key bits can be,
    /// in the order they're tried, and the key's bits.
    pub buckets16: &'static [&'static [u16]],
    pub key16: &'static [u8],
    /// The same of 32-bit encodings.
    pub buckets32: &'static [&'static [u16]],
    pub key32: &'static [u8],
    /// The same of 64-bit encodings (PowerPC's prefixed instructions: the
    /// prefix the high word).
    pub buckets64: &'static [&'static [u16]],
    pub key64: &'static [u8],
    /// Each decoder table's features (an index into the predicates): its
    /// instructions decode only with them on (MIPS's tables, each tried
    /// with some features).
    pub spaces: &'static [u16],
}

/// The most operands an instruction has.
pub const MAX_OPERANDS: usize = 8;

const FEATURE_WORDS: usize = 16;

/// A set of a target's features.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Features {
    bits: [u64; FEATURE_WORDS],
}

impl Features {
    /// None.
    pub fn none() -> Self {
        Self {
            bits: [0; FEATURE_WORDS],
        }
    }

    /// The features named (as LLVM names them), and what they imply.
    /// Names the target doesn't know are left out.
    pub fn of(t: &Target, names: &[&str]) -> Self {
        let mut f = Self::none();
        for n in names {
            f.add_named(t, n);
        }
        f
    }

    /// Adds a feature by its name, and what it implies.
    pub fn add_named(&mut self, t: &Target, name: &str) {
        if let Some(i) = t.features.iter().position(|x| x.name == name) {
            self.add(t, i);
        }
    }

    /// Adds these features.
    pub fn extend(&mut self, other: &Features) {
        for (a, b) in self.bits.iter_mut().zip(&other.bits) {
            *a |= b;
        }
    }

    fn add(&mut self, t: &Target, i: usize) {
        if i >= FEATURE_WORDS * 64 || self.has(i) {
            return;
        }
        self.bits[i / 64] |= 1 << (i % 64);
        if let Some(f) = t.features.get(i) {
            for &j in f.implies {
                self.add(t, usize::from(j));
            }
        }
    }

    /// Whether feature `i` (by index) is on.
    pub fn has(&self, i: usize) -> bool {
        i < FEATURE_WORDS * 64 && self.bits[i / 64] >> (i % 64) & 1 != 0
    }

    /// Whether a feature (by name) is on.
    pub fn contains(&self, t: &Target, name: &str) -> bool {
        t.features
            .iter()
            .position(|x| x.name == name)
            .is_some_and(|i| self.has(i))
    }

    /// Whether predicate `p` holds.
    pub fn satisfies(&self, t: &Target, p: u16) -> bool {
        let Some(conds) = t.predicates.get(usize::from(p)) else {
            return false;
        };
        conds.iter().all(|c| {
            let on = |f: &u16| self.has(usize::from(*f));
            let base = match c.kind {
                CondKind::All => c.features.iter().all(on),
                CondKind::Any => c.features.iter().any(on),
                CondKind::None => !c.features.iter().any(on),
            };
            base && !c.not.iter().any(on)
        })
    }
}

/// An operand of a decoded instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operand {
    Reg(Reg),
    Imm(i64),
}

impl Operand {
    /// Its register, if it's one.
    pub fn reg(self) -> Option<Reg> {
        match self {
            Operand::Reg(r) => Some(r),
            Operand::Imm(_) => None,
        }
    }

    /// Its immediate, if it's one.
    pub fn imm(self) -> Option<i64> {
        match self {
            Operand::Imm(v) => Some(v),
            Operand::Reg(_) => None,
        }
    }
}

/// A decoded instruction: which one, its size in bytes, and its operands
/// (in LLVM's order: what it writes, then what it reads).
#[derive(Clone, Copy, Debug)]
pub struct Instruction {
    target: &'static Target,
    index: u16,
    pub size: u8,
    ops: [Operand; MAX_OPERANDS],
    count: u8,
}

impl PartialEq for Instruction {
    fn eq(&self, other: &Self) -> bool {
        core::ptr::eq(self.target, other.target)
            && self.index == other.index
            && self.size == other.size
            && self.operands() == other.operands()
    }
}

impl Eq for Instruction {}

impl Instruction {
    /// Its description.
    pub fn inst(&self) -> &'static Inst {
        &self.target.instructions[usize::from(self.index)]
    }

    /// Its index among its target's instructions.
    pub fn index(&self) -> u16 {
        self.index
    }

    /// Its target.
    pub fn target(&self) -> &'static Target {
        self.target
    }

    /// Its LLVM name (`ADDI`, `C_ADDI`, `ADD_W`).
    pub fn name(&self) -> &'static str {
        self.inst().name
    }

    /// Its mnemonic as LLVM's syntax writes it (not an alias's).
    pub fn mnemonic(&self) -> &'static str {
        let asm = self.inst().asm;
        asm.split(['\t', ' ']).next().unwrap_or(asm)
    }

    /// Its operands.
    pub fn operands(&self) -> &[Operand] {
        &self.ops[..usize::from(self.count)]
    }

    /// Its operand named `name` (as its syntax names it).
    pub fn operand(&self, name: &str) -> Option<Operand> {
        let i = self.inst().ops.iter().position(|o| o.name == name)?;
        self.operands().get(i).copied()
    }
}

/// The instruction `w` (of `size` bytes, 2, 4, or 8) is, with `features`
/// on, if any; `rv64` says whether shift amounts may be 6 bits, `rve`
/// whether integer registers are 16.
pub fn decode(
    t: &'static Target,
    w: u64,
    size: u8,
    features: &Features,
    rv64: bool,
    rve: bool,
) -> Option<Instruction> {
    decode_from(t, w, size, features, 0, rv64, rve)
}

/// [`decode`], trying only the decoder tables from `first` on: where a
/// target's own decoding of an instruction fails, as LLVM goes on to its
/// next table (MIPS's R6 branch groups).
pub fn decode_from(
    t: &'static Target,
    w: u64,
    size: u8,
    features: &Features,
    first: u8,
    rv64: bool,
    rve: bool,
) -> Option<Instruction> {
    let (key_bits, buckets) = match size {
        2 => (t.key16, t.buckets16),
        8 => (t.key64, t.buckets64),
        _ => (t.key32, t.buckets32),
    };
    let mut key = 0usize;
    for (j, &b) in key_bits.iter().enumerate() {
        key |= (((w >> b) & 1) as usize) << j;
    }
    for &c in *buckets.get(key)? {
        let inst = &t.instructions[usize::from(c)];
        if inst.size != size
            || inst.space < first
            || w & inst.mask != inst.value
            || !features.satisfies(t, inst.pred)
        {
            continue;
        }
        if let Some(&space) = t.spaces.get(usize::from(inst.space))
            && !features.satisfies(t, space)
        {
            continue;
        }
        if let Some(i) = decode_operands(t, c, inst, w, rv64, rve) {
            return Some(i);
        }
    }
    None
}

/// `w`'s operands decoded as the instruction named `name` (LLVM's name),
/// whatever its fixed bits: for what a target's decoder chooses between by
/// hand (LoongArch's `ud` and `amswap.w`, one encoding).
pub fn decode_as(
    t: &'static Target,
    name: &str,
    w: u64,
    rv64: bool,
    rve: bool,
) -> Option<Instruction> {
    let index = t.instructions.iter().position(|i| i.name == name)?;
    let inst = &t.instructions[index];
    decode_operands(t, u16::try_from(index).ok()?, inst, w, rv64, rve)
}

/// An instruction of `t` (by its LLVM name) of `size` bytes with these
/// operands, as a target's own decoding builds it (MIPS's `dext`, whose
/// operands LLVM's decoder computes).
pub fn build(
    t: &'static Target,
    name: &str,
    size: u8,
    operands: &[Operand],
) -> Option<Instruction> {
    let index = t.instructions.iter().position(|i| i.name == name)?;
    if operands.len() > MAX_OPERANDS {
        return None;
    }
    let mut ops = [Operand::Imm(0); MAX_OPERANDS];
    ops[..operands.len()].copy_from_slice(operands);
    Some(Instruction {
        target: t,
        index: u16::try_from(index).ok()?,
        size,
        ops,
        count: u8::try_from(operands.len()).ok()?,
    })
}

/// A field's value: its pieces gathered.
fn field(w: u64, fields: &[(u8, u8, u8)]) -> u64 {
    let mut v = 0u64;
    for &(at, to, len) in fields {
        let m = if len >= 64 {
            u64::MAX
        } else {
            (1u64 << len) - 1
        };
        let piece = w.checked_shr(u32::from(at)).unwrap_or(0) & m;
        v |= piece.checked_shl(u32::from(to)).unwrap_or(0);
    }
    v
}

fn sext(v: u64, bits: u32) -> i64 {
    if bits == 0 || bits >= 64 {
        return v.cast_signed();
    }
    let shift = 64 - bits;
    (v << shift).cast_signed() >> shift
}

fn decode_operands(
    t: &'static Target,
    index: u16,
    inst: &Inst,
    w: u64,
    rv64: bool,
    rve: bool,
) -> Option<Instruction> {
    let mut ops = [Operand::Imm(0); MAX_OPERANDS];
    if inst.ops.len() > MAX_OPERANDS {
        return None;
    }
    for (i, op) in inst.ops.iter().enumerate() {
        let kind = t.kinds.get(usize::from(op.kind))?;
        // Its value: its fields', its fixed value, or its tied operand's
        // fields'.
        let source = match (op.fields, op.constant, op.tied) {
            (f, _, _) if !f.is_empty() => Some(field(w, f)),
            (_, Some(c), _) => Some(u64::from(c)),
            (_, None, Some(tie)) => {
                let other = inst.ops.get(usize::from(tie))?;
                match (other.fields, other.constant) {
                    (f, _) if !f.is_empty() => Some(field(w, f)),
                    (_, c) => c.map(u64::from),
                }
            }
            _ => None,
        };
        ops[i] = operand(t, kind, source, &ops[..i], rv64, rve)?;
    }
    Some(Instruction {
        target: t,
        index,
        size: inst.size,
        ops,
        count: u8::try_from(inst.ops.len()).ok()?,
    })
}

/// `v`, if it has no more than `bits` bits (as LLVM's decoder methods
/// check: a field may be wider than its operand).
fn fits_in(v: u64, bits: u32) -> Option<u64> {
    (bits == 0 || bits >= 64 || v >> bits == 0).then_some(v)
}

/// Whether register `r` is in class `c`.
pub fn in_class(t: &Target, r: Reg, c: u16) -> bool {
    let Some(class) = t.classes.get(usize::from(c)) else {
        return false;
    };
    !r.is_none()
        && class.file == r.file
        && if r.n < 64 {
            class.members >> r.n & 1 != 0
        } else {
            class.order.contains(&r.n)
        }
}

/// Zcmp's s0-s7, by their number: x8, x9, x18 to x23.
fn s07(f: u64) -> Option<u8> {
    u8::try_from(if f < 2 { 8 + f } else { 16 + f }).ok()
}

/// An operand of `kind` from its field's value `v` (`None`: it has none).
fn operand(
    t: &Target,
    kind: &Kind,
    v: Option<u64>,
    before: &[Operand],
    rv64: bool,
    rve: bool,
) -> Option<Operand> {
    let class = kind
        .class
        .and_then(|c| t.classes.get(usize::from(c)).map(|k| (c, k)));
    let reg = |n: u64| -> Option<Operand> {
        let (c, k) = class?;
        let r = Reg::new(k.file, u8::try_from(n).ok().filter(|&n| n < 64)?);
        // (RVE has 16 integer registers.)
        if rve && k.file == 0 && n >= 16 {
            return None;
        }
        in_class(t, r, c).then_some(Operand::Reg(r))
    };
    let file = class.map_or(0, |(_, k)| k.file);
    Some(match kind.decode {
        Decode::RegSimple(base) => reg(u64::from(base) + v?)?,
        Decode::RegSingle(n) => reg(u64::from(n))?,
        Decode::RegGroup(lmul) => {
            let f = v?;
            if lmul == 0 || f % u64::from(lmul) != 0 {
                return None;
            }
            reg(f)?
        }
        Decode::RegPair(base) => {
            let n = u64::from(base) + v?;
            if n % 2 != 0 {
                return None;
            }
            reg(n)?
        }
        Decode::RegOnly(n) => {
            if v? != u64::from(n) {
                return None;
            }
            reg(u64::from(n))?
        }
        Decode::UImmLsl { bits, arg } => {
            Operand::Imm(i64::try_from(fits_in(v?, bits)? << arg).ok()?)
        }
        Decode::BranchPlus4 { bits, arg } => Operand::Imm(sext(v? << arg, bits) + 4),
        Decode::InsSize { arg, .. } => {
            let pos = before.get(usize::try_from(arg).ok()?)?.imm()?;
            Operand::Imm(sext(
                (i64::try_from(v?).ok()? - pos + 1).cast_unsigned() & 0xFFFF,
                16,
            ))
        }
        Decode::RegSelect(_) => {
            let (_, k) = class?;
            let f = v?;
            let (n, sel) = (f >> 5, f & 31);
            if n >= 32 || sel >= 8 {
                return None;
            }
            let n = *k.order.get(usize::try_from(sel * 32 + n).ok()?)?;
            Operand::Reg(Reg::new(k.file, n))
        }
        Decode::RegIndex(limit) => {
            let (_, k) = class?;
            let f = v?;
            if limit > 0 && f >= u64::from(limit) {
                return None;
            }
            let n = *k.order.get(usize::try_from(f).ok()?)?;
            Operand::Reg(Reg::new(k.file, n))
        }
        Decode::RegHalf(base) => {
            let f = v?;
            if f % 2 != 0 {
                return None;
            }
            reg(u64::from(base) + f / 2)?
        }
        Decode::CrBitM(file) => {
            // (One bit set, of 8: 0x80 is field 0.)
            let f = v?;
            let zeros = f.trailing_zeros();
            if zeros >= 8 {
                return None;
            }
            Operand::Reg(Reg::new(
                u8::try_from(file).ok()?,
                u8::try_from(7 - zeros).ok()?,
            ))
        }
        Decode::Zero { .. } => {
            if v? != 0 {
                return None;
            }
            Operand::Imm(0)
        }
        Decode::RiHash { .. } => {
            let f = (v? & 0x3F) + 64;
            Operand::Imm(sext(f, 7) * 8)
        }
        Decode::RegS07(_) | Decode::Mvsa01(_) => {
            let f = v?;
            if f >= 8 || (rve && f > 1) {
                return None;
            }
            let r = Operand::Reg(Reg::new(file, s07(f)?));
            if matches!(kind.decode, Decode::Mvsa01(_)) && before.first() == Some(&r) {
                return None;
            }
            r
        }
        Decode::VMask(_) => match v.unwrap_or(0) {
            0 => Operand::Reg(Reg::new(file, 0)),
            1 => Operand::Reg(Reg::NONE),
            _ => return None,
        },
        Decode::Fixed(c) => Operand::Imm(c),
        Decode::Raw { .. } => Operand::Imm(i64::try_from(v?).ok()?),
        Decode::UImm { bits, arg } => {
            let f = i64::try_from(fits_in(v?, bits)?).ok()?;
            if f < arg {
                return None;
            }
            Operand::Imm(f)
        }
        Decode::UImmAdd { arg, .. } => Operand::Imm(i64::try_from(v?).ok()? + arg),
        Decode::SImm { bits, .. } => Operand::Imm(sext(fits_in(v?, bits)?, bits)),
        Decode::SImmNonZero { bits, .. } => {
            let f = v?;
            if f == 0 {
                return None;
            }
            Operand::Imm(sext(f, bits))
        }
        Decode::UImmNonZero { .. } => {
            let f = v?;
            if f == 0 {
                return None;
            }
            Operand::Imm(i64::try_from(f).ok()?)
        }
        Decode::SImmLsl { bits, arg } => Operand::Imm(sext(v? << arg, bits)),
        Decode::Log2XLen { .. } | Decode::Log2XLenNonZero { .. } => {
            let f = v?;
            if (!rv64 && f >= 32)
                || (matches!(kind.decode, Decode::Log2XLenNonZero { .. }) && f == 0)
            {
                return None;
            }
            Operand::Imm(i64::try_from(f).ok()?)
        }
        Decode::UImmPlus1 { arg, .. } => {
            let f = i64::try_from(v?).ok()? + 1;
            if f < arg {
                return None;
            }
            Operand::Imm(f)
        }
        Decode::CLui { .. } => {
            let f = v?;
            if f == 0 {
                return None;
            }
            Operand::Imm(sext(f, 6) & 0xFFFFF)
        }
        Decode::Frm { .. } => {
            let f = v?;
            if f == 5 || f == 6 {
                return None;
            }
            Operand::Imm(i64::try_from(f).ok()?)
        }
        Decode::Rlist { .. } => {
            let f = v?;
            if f < 4 || (rve && f >= 7) {
                return None;
            }
            Operand::Imm(i64::try_from(f).ok()?)
        }
        Decode::ZeroIs { arg, .. } => match v? {
            0 => Operand::Imm(arg),
            f => Operand::Imm(i64::try_from(f).ok()?),
        },
        Decode::MaxIsMinus1 { bits, .. } => {
            let f = v?;
            if bits < 64 && f == (1 << bits) - 1 {
                Operand::Imm(-1)
            } else {
                Operand::Imm(i64::try_from(f).ok()?)
            }
        }
        Decode::Andi16 { .. } => Operand::Imm(*ANDI16.get(usize::try_from(v?).ok()?)?),
        Decode::Addiur2 { .. } => Operand::Imm(match v? {
            0 => 1,
            7 => -1,
            f => i64::try_from(f).ok()? << 2,
        }),
        Decode::Simm9Sp { .. } => Operand::Imm(
            4 * match v? {
                0 => 256,
                1 => 257,
                510 => -258,
                511 => -257,
                f => sext(f & 0x1FF, 9),
            },
        ),
        Decode::MipsRegList { arg, .. } => {
            let f = v?;
            let (count, ra) = if arg == 1 {
                ((f & 3) + 1, true)
            } else {
                // (Empty, and counts of 10 to 15, are reserved.)
                if f == 0 || f & 0xF > 9 {
                    return None;
                }
                (f & 0xF, f & 0x10 != 0)
            };
            Operand::Imm(i64::try_from(count | if ra { 0x10 } else { 0 }).ok()?)
        }
        Decode::MoveP { arg, .. } => {
            let (a, b) = *MOVEP.get(usize::try_from(v?).ok()?)?;
            reg(if arg == 0 { a } else { b })?
        }
    })
}

/// Whether an instruction's operands pass `checks`.
pub fn passes(t: &Target, ops: &[Operand], checks: &[Check]) -> bool {
    checks.iter().all(|c| match *c {
        Check::Tied(a, b) => {
            ops.get(usize::from(a)).is_some() && ops.get(usize::from(a)) == ops.get(usize::from(b))
        }
        Check::Class(a, class) => {
            matches!(ops.get(usize::from(a)), Some(Operand::Reg(r)) if in_class(t, *r, class))
        }
        Check::Reg(a, r) => ops.get(usize::from(a)) == Some(&Operand::Reg(r)),
        Check::Imm(a, v) => ops.get(usize::from(a)) == Some(&Operand::Imm(v)),
        Check::ImmFits(a, range) => match ops.get(usize::from(a)) {
            Some(Operand::Imm(v)) => fits(*v, range),
            _ => false,
        },
    })
}

fn fits(v: i64, r: ImmRange) -> bool {
    if r.nonzero && v == 0 {
        return false;
    }
    if r.zeros > 0 && v & ((1 << r.zeros) - 1) != 0 {
        return false;
    }
    let bits = r.bits.clamp(1, 63);
    if r.signed {
        let lim = 1i64 << (bits - 1);
        (-lim..lim).contains(&v)
    } else {
        (0..1i64 << bits).contains(&v)
    }
}

/// A compressed instruction's full form, if it has one.
pub fn uncompress(i: &Instruction, features: &Features) -> Option<Instruction> {
    let t = i.target;
    for u in t.uncompress.iter().filter(|u| u.from == i.index) {
        if !features.satisfies(t, u.pred) || !passes(t, i.operands(), u.checks) {
            continue;
        }
        let mut ops = [Operand::Imm(0); MAX_OPERANDS];
        if u.build.len() > MAX_OPERANDS {
            return None;
        }
        for (k, b) in u.build.iter().enumerate() {
            ops[k] = match *b {
                Build::From(s) => *i.operands().get(usize::from(s))?,
                Build::Reg(r) => Operand::Reg(r),
                Build::Imm(v) => Operand::Imm(v),
            };
        }
        // (Its size stays the compressed one's: it's still that long.)
        return Some(Instruction {
            target: t,
            index: u.to,
            size: i.size,
            ops,
            count: u8::try_from(u.build.len()).ok()?,
        });
    }
    None
}

/// Writes `i` as LLVM's printer does: as the first alias (by priority)
/// whose features are on and whose checks it passes, else as its own
/// syntax; each operand (its index, how the alias or its kind says to
/// print it, and its modifier: `${name:modifier}`'s, else empty) by
/// `operand`.
pub fn write(
    out: &mut String,
    i: &Instruction,
    features: &Features,
    mut operand: impl FnMut(&mut String, usize, Print, &str),
) {
    let t = i.target;
    for a in t.aliases.iter().filter(|a| a.inst == i.index) {
        if !features.satisfies(t, a.pred) || !passes(t, i.operands(), a.checks) {
            continue;
        }
        write_asm(out, a.asm, |out, name, modifier| {
            let Some(r) = name.parse::<usize>().ok().and_then(|n| a.refs.get(n)) else {
                return;
            };
            let print = r.print.unwrap_or_else(|| {
                // (An operand the alias prints as the instruction does.)
                i.inst()
                    .ops
                    .get(usize::from(r.op))
                    .and_then(|o| t.kinds.get(usize::from(o.kind)))
                    .map_or(Print::Operand, |k| k.print)
            });
            operand(out, usize::from(r.op), print, modifier);
        });
        return;
    }
    let inst = i.inst();
    write_asm(out, inst.asm, |out, name, modifier| {
        let Some(op) = inst.ops.iter().position(|o| o.name == name) else {
            return;
        };
        let print = t
            .kinds
            .get(usize::from(inst.ops[op].kind))
            .map_or(Print::Operand, |k| k.print);
        operand(out, op, print, modifier);
    });
}

/// Writes `asm`, each operand (`$name`, `${name}`, `${name:modifier}`) by
/// `operand`.
fn write_asm(out: &mut String, asm: &str, mut operand: impl FnMut(&mut String, &str, &str)) {
    let b = asm.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'$' {
            let next = asm[i..].find('$').map_or(b.len(), |n| i + n);
            out.push_str(&asm[i..next]);
            i = next;
            continue;
        }
        i += 1;
        // (`$$`: a `$`, MIPS's `div $$zero, ...`.)
        if b.get(i) == Some(&b'$') {
            out.push('$');
            i += 1;
            continue;
        }
        let (name, end) = if b.get(i) == Some(&b'{') {
            let close = asm[i..].find('}').map_or(b.len(), |n| i + n);
            (&asm[(i + 1).min(close)..close], (close + 1).min(b.len()))
        } else {
            let end = asm[i..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .map_or(b.len(), |n| i + n);
            (&asm[i..end], end)
        };
        let (name, modifier) = name.split_once(':').unwrap_or((name, ""));
        operand(out, name, modifier);
        i = end;
    }
}

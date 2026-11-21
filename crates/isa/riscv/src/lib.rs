//! RISC-V instruction decoding: the standard extensions LLVM's
//! disassembler decodes (RV32 and RV64; I, M, A, F, D, Q, C and the Zc*
//! extensions, B's, the scalar crypto ones, V and its Zv* extensions, the
//! hypervisor's, Zfa, Zfh, Zicond, ...), from LLVM's TableGen records (see
//! `tools/riscv/extract.py`, which writes `tables.rs`).
//!
//! [`decode`] finds the instruction a 16- or 32-bit encoding is, as LLVM
//! does: its decoder tables in their order, the instructions whose fixed
//! bits match and whose features are on, the most specific first, the
//! first whose operands decode. [`format`] writes it as LLVM's
//! disassembler does (compressed instructions as their full forms, and
//! aliases: `li`, `mv`, `ret`, `j`, ...). Neither panics.

#![no_std]

extern crate alloc;

mod tables;
pub mod tables_types;

pub mod format;

use tables_types::{Check, CondKind, Decode, ImmRange};
pub use tables_types::{Csr, File, Inst, Op, Reg};

/// The most operands an instruction has.
pub const MAX_OPERANDS: usize = 8;

/// The number of features known (vendors' and experimental ones too).
const FEATURE_WORDS: usize = 16;

/// A set of features (extensions, `64bit`): what an image's code may use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Features {
    bits: [u64; FEATURE_WORDS],
}

/// The standard extensions [`Features::standard`] enables (LLVM's names):
/// every ratified one but those that conflict with these (Zfinx and its
/// kin, which take F's encodings; Zcmp and Zcmt, which take Zcd's, and Zce,
/// which implies them; Zilsd and Zclsd, RV32's alone, Zclsd taking Zcf's;
/// RVE).
const STANDARD: &[&str] = &[
    "m",
    "a",
    "f",
    "d",
    "q",
    "c",
    "v",
    "h",
    "zicsr",
    "zifencei",
    "zicntr",
    "zihpm",
    "zihintpause",
    "zihintntl",
    "zicbom",
    "zicbop",
    "zicboz",
    "zicond",
    "zimop",
    "zcmop",
    "zawrs",
    "zacas",
    "zabha",
    "zalasr",
    "zama16b",
    "zfa",
    "zfh",
    "zfbfmin",
    "zba",
    "zbb",
    "zbc",
    "zbs",
    "zbkb",
    "zbkc",
    "zbkx",
    "zknd",
    "zkne",
    "zknh",
    "zksed",
    "zksh",
    "zkt",
    "zkr",
    "zca",
    "zcb",
    "zcd",
    "zvbb",
    "zvbc",
    "zvkg",
    "zvkned",
    "zvknha",
    "zvknhb",
    "zvksed",
    "zvksh",
    "zvkt",
    "zvfh",
    "zvfbfmin",
    "zvfbfwma",
    "svinval",
    "svnapot",
    "svpbmt",
    "smrnmi",
    "smctr",
    "ssctr",
    "zicfiss",
    "sha",
    "shcounterenw",
    "shgatpa",
    "ssqosid",
    "zvkb",
];

impl Features {
    /// No features (RV32I).
    pub fn none() -> Self {
        Self {
            bits: [0; FEATURE_WORDS],
        }
    }

    /// The features named (as LLVM names them), and what they imply.
    /// Names it doesn't know are left out.
    pub fn of(names: &[&str]) -> Self {
        let mut f = Self::none();
        for n in names {
            if let Some(i) = tables::FEATURES.iter().position(|x| x.name == *n) {
                f.add(i);
            }
        }
        f.complete();
        f
    }

    /// Every standard extension that doesn't conflict with another (see
    /// [`STANDARD`]), for RV64 (`rv64`) or RV32.
    pub fn standard(rv64: bool) -> Self {
        let mut f = Self::of(STANDARD);
        if rv64 {
            f.extend(&Self::of(&["64bit"]));
        }
        f
    }

    /// The features an ISA string names (`rv64gc`, an ELF image's
    /// `.riscv.attributes` arch: `rv64i2p1_m2p0_a2p1_f2p2_d2p2_c2p0_zicsr2p0`):
    /// its base (`rv32` or `rv64`; `g` is `imafd_zicsr_zifencei`), each
    /// single-letter extension, and each multi-letter one, versions left
    /// out.
    pub fn from_arch(arch: &str) -> Self {
        let lower = arch.to_ascii_lowercase();
        let mut names: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
        let rest = if let Some(r) = lower.strip_prefix("rv64") {
            names.push("64bit".into());
            r
        } else {
            lower.strip_prefix("rv32").unwrap_or(&lower)
        };
        let mut parts = rest.split('_');
        // Single letters, each perhaps with a version (`i2p1`).
        if let Some(first) = parts.next() {
            for c in first.chars().filter(char::is_ascii_alphabetic) {
                if c == 'p' {
                    continue;
                }
                if c == 'g' {
                    for n in ["i", "m", "a", "f", "d", "zicsr", "zifencei"] {
                        names.push(n.into());
                    }
                } else {
                    names.push(alloc::string::String::from(c));
                }
            }
        }
        for p in parts {
            let name: alloc::string::String = p
                .trim_end_matches(|c: char| c.is_ascii_digit() || c == 'p')
                .into();
            // (A version's `p` between its numbers: `2p0`.)
            if name.len() > 1 {
                names.push(name);
            } else if let Some(c) = name.chars().next() {
                names.push(alloc::string::String::from(c));
            }
        }
        let refs: alloc::vec::Vec<&str> = names.iter().map(alloc::string::String::as_str).collect();
        Self::of(&refs)
    }

    /// Adds these features.
    pub fn extend(&mut self, other: &Features) {
        for (a, b) in self.bits.iter_mut().zip(&other.bits) {
            *a |= b;
        }
        self.complete();
    }

    /// What LLVM's ISA strings imply beyond the features' own
    /// implications: C with F on RV32 is Zcf too, C with D is Zcd.
    fn complete(&mut self) {
        let c = self.contains("c");
        let rv64 = self.rv64();
        let mut more = alloc::vec::Vec::new();
        if c && self.contains("f") && !rv64 {
            more.push("zcf");
        }
        if c && self.contains("d") {
            more.push("zcd");
        }
        for n in more {
            if let Some(i) = tables::FEATURES.iter().position(|x| x.name == n) {
                self.add(i);
            }
        }
    }

    fn add(&mut self, i: usize) {
        if i >= FEATURE_WORDS * 64 || self.has(i) {
            return;
        }
        self.bits[i / 64] |= 1 << (i % 64);
        for &j in tables::FEATURES[i].implies {
            self.add(usize::from(j));
        }
    }

    fn has(&self, i: usize) -> bool {
        i < FEATURE_WORDS * 64 && self.bits[i / 64] >> (i % 64) & 1 != 0
    }

    /// Whether a feature (by name) is on.
    pub fn contains(&self, name: &str) -> bool {
        tables::FEATURES
            .iter()
            .position(|x| x.name == name)
            .is_some_and(|i| self.has(i))
    }

    /// Whether the code is RV64's.
    pub fn rv64(&self) -> bool {
        self.contains("64bit")
    }

    fn rve(&self) -> bool {
        self.contains("e")
    }

    /// Whether predicate `p` holds.
    fn satisfies(&self, p: u16) -> bool {
        let Some(conds) = tables::PREDICATES.get(usize::from(p)) else {
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instruction {
    index: u16,
    pub size: u8,
    ops: [Operand; MAX_OPERANDS],
    count: u8,
}

impl Instruction {
    /// Its description.
    pub fn inst(&self) -> &'static Inst {
        &tables::INSTRUCTIONS[usize::from(self.index)]
    }

    /// Its LLVM name (`ADDI`, `C_ADDI`, `VADD_VV`).
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

    /// Its operand named `name` (as its syntax names it: `rd`, `rs1`,
    /// `imm12`, `offset`).
    pub fn operand(&self, name: &str) -> Option<Operand> {
        let i = self.inst().ops.iter().position(|o| o.name == name)?;
        self.operands().get(i).copied()
    }
}

/// The instruction at the start of `bytes` (little-endian), with
/// `features` on: 16 bits if its low two bits aren't 11, else 32. `None`
/// if it isn't one (or `bytes` is too short); longer encodings (48 bits
/// and up) aren't decoded.
pub fn decode(bytes: &[u8], features: &Features) -> Option<Instruction> {
    let b0 = *bytes.first()?;
    if b0 & 3 != 3 {
        let w = u32::from(u16::from_le_bytes([b0, *bytes.get(1)?]));
        let key = (w & 3) | ((w >> 13) & 7) << 2;
        return find(w, tables::BUCKETS16.get(key as usize)?, 2, features);
    }
    let w = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    let key = (w & 0x7F) | ((w >> 12) & 7) << 7;
    find(w, tables::BUCKETS32.get(key as usize)?, 4, features)
}

/// The size of the instruction whose first byte is `b0` (as its length
/// encoding says: 2, 4, 6, 8, or more), if it says.
pub fn length(b0: u8, b1: u8) -> Option<usize> {
    if b0 & 3 != 3 {
        return Some(2);
    }
    if b0 & 0x1C != 0x1C {
        return Some(4);
    }
    if b0 & 0x3F == 0x1F {
        return Some(6);
    }
    if b0 & 0x7F == 0x3F {
        return Some(8);
    }
    let nnn = (b1 >> 4) & 7;
    (nnn != 7).then(|| 10 + 2 * usize::from(nnn))
}

fn find(w: u32, candidates: &[u16], size: u8, features: &Features) -> Option<Instruction> {
    for &c in candidates {
        let inst = &tables::INSTRUCTIONS[usize::from(c)];
        if inst.size != size || w & inst.mask != inst.value || !features.satisfies(inst.pred) {
            continue;
        }
        if let Some(i) = decode_operands(c, inst, w, features) {
            return Some(i);
        }
    }
    None
}

/// A field's value: its pieces gathered.
fn field(w: u32, fields: &[(u8, u8, u8)]) -> u32 {
    let mut v = 0u32;
    for &(at, to, len) in fields {
        let m = if len >= 32 {
            u32::MAX
        } else {
            (1u32 << len) - 1
        };
        v |= ((w >> at) & m) << to;
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

fn decode_operands(index: u16, inst: &Inst, w: u32, features: &Features) -> Option<Instruction> {
    let mut ops = [Operand::Imm(0); MAX_OPERANDS];
    if inst.ops.len() > MAX_OPERANDS {
        return None;
    }
    for (i, op) in inst.ops.iter().enumerate() {
        let kind = tables::KINDS.get(usize::from(op.kind))?;
        // Its value: its fields', its fixed value, or its tied operand's
        // fields'.
        let source = match (op.fields, op.constant, op.tied) {
            (f, _, _) if !f.is_empty() => Some(field(w, f)),
            (_, Some(c), _) => Some(c),
            (_, None, Some(t)) => {
                let other = inst.ops.get(usize::from(t))?;
                match (other.fields, other.constant) {
                    (f, _) if !f.is_empty() => Some(field(w, f)),
                    (_, c) => c,
                }
            }
            _ => None,
        };
        ops[i] = operand(kind, source, &ops[..i], features)?;
    }
    Some(Instruction {
        index,
        size: inst.size,
        ops,
        count: u8::try_from(inst.ops.len()).ok()?,
    })
}

/// Whether register `r` is in class `c`.
fn in_class(r: Reg, c: u16) -> bool {
    let Some(class) = tables::CLASSES.get(usize::from(c)) else {
        return false;
    };
    let (file, n) = match r {
        Reg::X(n) => (File::X, n),
        Reg::F(n) => (File::F, n),
        Reg::V(n) => (File::V, n),
        Reg::None => return false,
    };
    class.file == file && n < 32 && class.members >> n & 1 != 0
}

fn reg_of(file: File, n: u32) -> Option<Reg> {
    let n = u8::try_from(n).ok().filter(|&n| n < 32)?;
    Some(match file {
        File::X => Reg::X(n),
        File::F => Reg::F(n),
        File::V => Reg::V(n),
    })
}

/// An operand of `kind` from its field's value `v` (`None`: it has none).
fn operand(
    kind: &tables_types::Kind,
    v: Option<u32>,
    before: &[Operand],
    features: &Features,
) -> Option<Operand> {
    let class = kind
        .class
        .and_then(|c| tables::CLASSES.get(usize::from(c)).map(|k| (c, k)));
    let reg = |n: u32| -> Option<Operand> {
        let (c, k) = class?;
        let r = reg_of(k.file, n)?;
        // (RVE has 16 integer registers.)
        if k.file == File::X && features.rve() && n >= 16 {
            return None;
        }
        in_class(r, c).then_some(Operand::Reg(r))
    };
    Some(match kind.decode {
        Decode::RegSimple(base) => reg(base + v?)?,
        Decode::RegSingle(n) => reg(n)?,
        Decode::RegGroup(lmul) => {
            let f = v?;
            if lmul == 0 || f % lmul != 0 {
                return None;
            }
            reg(f)?
        }
        Decode::RegPair(base) => {
            let n = base + v?;
            if n % 2 != 0 {
                return None;
            }
            reg(n)?
        }
        Decode::RegS07(_) => {
            let f = v?;
            if f >= 8 || (features.rve() && f > 1) {
                return None;
            }
            Operand::Reg(Reg::X(s07(f)?))
        }
        Decode::Mvsa01(_) => {
            let f = v?;
            if f >= 8 || (features.rve() && f > 1) {
                return None;
            }
            let r = Operand::Reg(Reg::X(s07(f)?));
            if before.first() == Some(&r) {
                return None;
            }
            r
        }
        Decode::VMask(_) => match v.unwrap_or(0) {
            0 => Operand::Reg(Reg::V(0)),
            1 => Operand::Reg(Reg::None),
            _ => return None,
        },
        Decode::Fixed(c) => Operand::Imm(c),
        Decode::Raw { .. } => Operand::Imm(i64::from(v?)),
        Decode::UImm { arg, .. } => {
            let f = i64::from(v?);
            if f < arg {
                return None;
            }
            Operand::Imm(f)
        }
        Decode::SImm { bits, .. } => Operand::Imm(sext(u64::from(v?), bits)),
        Decode::SImmNonZero { bits, .. } => {
            let f = v?;
            if f == 0 {
                return None;
            }
            Operand::Imm(sext(u64::from(f), bits))
        }
        Decode::UImmNonZero { .. } => {
            let f = v?;
            if f == 0 {
                return None;
            }
            Operand::Imm(i64::from(f))
        }
        Decode::SImmLsl { bits, arg } => Operand::Imm(sext(u64::from(v?) << arg, bits)),
        Decode::Log2XLen { .. } | Decode::Log2XLenNonZero { .. } => {
            let f = v?;
            if (!features.rv64() && f >= 32)
                || (matches!(kind.decode, Decode::Log2XLenNonZero { .. }) && f == 0)
            {
                return None;
            }
            Operand::Imm(i64::from(f))
        }
        Decode::UImmPlus1 { arg, .. } => {
            let f = i64::from(v?) + 1;
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
            Operand::Imm(sext(u64::from(f), 6) & 0xFFFFF)
        }
        Decode::Frm { .. } => {
            let f = v?;
            if f == 5 || f == 6 {
                return None;
            }
            Operand::Imm(i64::from(f))
        }
        Decode::Rlist { .. } => {
            let f = v?;
            if f < 4 || (features.rve() && f >= 7) {
                return None;
            }
            Operand::Imm(i64::from(f))
        }
    })
}

/// Zcmp's s0-s7, by their number: x8, x9, x18 to x23.
fn s07(f: u32) -> Option<u8> {
    u8::try_from(if f < 2 { 8 + f } else { 16 + f }).ok()
}

/// Whether an instruction's operands pass `checks`.
fn passes(ops: &[Operand], checks: &[Check]) -> bool {
    checks.iter().all(|c| match *c {
        Check::Tied(a, b) => {
            ops.get(usize::from(a)).is_some() && ops.get(usize::from(a)) == ops.get(usize::from(b))
        }
        Check::Class(a, class) => {
            matches!(ops.get(usize::from(a)), Some(Operand::Reg(r)) if in_class(*r, class))
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
    let bits = r.bits.min(63);
    if r.signed {
        let lim = 1i64 << (bits - 1);
        (-lim..lim).contains(&v)
    } else {
        (0..1i64 << bits).contains(&v)
    }
}

/// A compressed instruction's full form, as LLVM prints it (and as it
/// runs): `c.addi a0, 1` is `addi a0, a0, 1`. `None` for an instruction
/// that isn't compressed (or has no full form).
pub fn uncompress(i: &Instruction, features: &Features) -> Option<Instruction> {
    for u in tables::UNCOMPRESS.iter().filter(|u| u.from == i.index) {
        if !features.satisfies(u.pred) || !passes(i.operands(), u.checks) {
            continue;
        }
        let mut ops = [Operand::Imm(0); MAX_OPERANDS];
        if u.build.len() > MAX_OPERANDS {
            return None;
        }
        for (k, b) in u.build.iter().enumerate() {
            ops[k] = match *b {
                tables_types::Build::From(s) => *i.operands().get(usize::from(s))?,
                tables_types::Build::Reg(r) => Operand::Reg(r),
                tables_types::Build::Imm(v) => Operand::Imm(v),
            };
        }
        // (Its size stays the compressed one's: it's still two bytes.)
        return Some(Instruction {
            index: u.to,
            size: i.size,
            ops,
            count: u8::try_from(u.build.len()).ok()?,
        });
    }
    None
}

#[cfg(test)]
mod tests;

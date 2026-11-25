//! PowerPC instruction decoding: what LLVM's disassembler decodes for the
//! CPU llvm-objdump takes PowerPC code to be for (`future`: 32- and 64-bit
//! instructions, Altivec, VSX, ISA 3.1's prefixed instructions, MMA, and
//! what follows them; not SPE's or BookE's), from LLVM's TableGen records
//! (see `tools/powerpc/extract.py`, which writes `tables.rs`, and
//! `turbonop-tablegen`, which decodes by them).
//!
//! [`decode`] finds the instruction at an address, big- or little-endian:
//! a prefixed (8-byte) one first, as LLVM does, then a 4-byte one.
//! [`format`] writes it as llvm-objdump does (aliases: `li`, `mr`, `blr`,
//! `bt`, `mflr`, ...; and those LLVM's printer writes by hand: `slwi`,
//! `srwi`, `sldi`, `dcbt`, `dcbf`). Neither panics.

#![no_std]

extern crate alloc;

mod tables;

pub mod format;

pub use turbonop_tablegen::{Inst, Instruction, Op, Operand, Reg};

/// The register files (as the tables number them).
pub mod files {
    /// The GPRs (32- and 64-bit alike; `r0` read as 0 too).
    pub const R: u8 = 0;
    /// The 64 VSX registers: the FPRs 0 to 31, Altivec's 32 to 63.
    pub const VS: u8 = 1;
    /// The condition register's fields (`cr0` to `cr7`).
    pub const CR: u8 = 2;
    /// The condition register's bits (0 to 31).
    pub const CRBIT: u8 = 3;
    /// FPR pairs, by their first.
    pub const FP: u8 = 4;
    /// VSX register pairs (`vsp0`: 0 and 1).
    pub const VSP: u8 = 5;
    /// MMA's accumulators.
    pub const ACC: u8 = 6;
    /// The wide accumulators, and their high halves.
    pub const WACC: u8 = 7;
    pub const WACC_HI: u8 = 8;
    /// The dense math registers, their row pairs, pairs, and rows.
    pub const DMR: u8 = 9;
    pub const DMRROWP: u8 = 10;
    pub const DMRP: u8 = 11;
    pub const DMRROW: u8 = 12;
}

/// GPR `n` (`r0` to `r31`).
pub const fn r(n: u8) -> Reg {
    Reg::new(files::R, n)
}

/// A GPR's number, if `reg` is one.
pub fn gpr(reg: Reg) -> Option<u8> {
    (reg.file == files::R).then_some(reg.n)
}

/// An operand's GPR's number, if it's one.
pub fn gpr_of(o: Operand) -> Option<u8> {
    o.reg().and_then(gpr)
}

/// The most operands an instruction has.
pub const MAX_OPERANDS: usize = turbonop_tablegen::MAX_OPERANDS;

/// The features code may use, and whether it is 64-bit code (whose
/// branch targets don't wrap at 32 bits).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Features {
    set: turbonop_tablegen::Features,
    ppc64: bool,
}

impl Features {
    /// The features of LLVM's `future` CPU (as llvm-objdump takes PowerPC
    /// code), for 64-bit code (`ppc64`) or 32-bit.
    pub fn standard(ppc64: bool) -> Self {
        let mut f = Self::of(tables::FUTURE, ppc64);
        if ppc64 {
            f.set.add_named(&tables::TARGET, "64bit");
        }
        f
    }

    /// The features named (as LLVM names them), and what they imply.
    /// Names it doesn't know are left out.
    pub fn of(names: &[&str], ppc64: bool) -> Self {
        Self {
            set: turbonop_tablegen::Features::of(&tables::TARGET, names),
            ppc64,
        }
    }

    /// Whether a feature (by name) is on.
    pub fn contains(&self, name: &str) -> bool {
        self.set.contains(&tables::TARGET, name)
    }

    /// Whether the code is 64-bit.
    pub fn ppc64(&self) -> bool {
        self.ppc64
    }
}

/// The instruction at the start of `bytes`, its words big-endian (`big`)
/// or little-endian, with `features` on: a prefixed instruction (a prefix
/// word, then its suffix) if the prefixed instructions are on and it is
/// one, else a word's. `None` if it isn't one (or `bytes` is too short).
pub fn decode(bytes: &[u8], big: bool, features: &Features) -> Option<Instruction> {
    let word = |at: usize| -> Option<u64> {
        let b: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(u64::from(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        }))
    };
    let t = &tables::TARGET;
    let first = word(0)?;
    // (A prefix's primary opcode is 1.)
    if first >> 26 == 1
        && features.contains("prefix-instrs")
        && let Some(second) = word(4)
        && let Some(i) =
            turbonop_tablegen::decode(t, first << 32 | second, 8, &features.set, false, false)
    {
        return Some(i);
    }
    turbonop_tablegen::decode(t, first, 4, &features.set, false, false)
}

/// Where a branch goes, if `i` is one with a target (`b`, `bl`, `bc`,
/// `bdnz`, ...; not `blr` or `bctr`): its address plus its offset, or its
/// absolute target (`ba`, `bla`, `bca`).
pub fn branch_target(i: &Instruction, address: u64, features: &Features) -> Option<u64> {
    let t = i.target();
    i.inst().ops.iter().enumerate().find_map(|(k, op)| {
        let kind = t.kinds.get(usize::from(op.kind))?;
        let v = i.operands().get(k)?.imm()?;
        let to = match kind.print {
            turbonop_tablegen::Print::Branch => address.wrapping_add_signed(v),
            turbonop_tablegen::Print::AbsBranch => v.cast_unsigned(),
            _ => return None,
        };
        Some(if features.ppc64() {
            to
        } else {
            to & 0xFFFF_FFFF
        })
    })
}

/// A displacement memory operand's displacement and base register (`8(1)`:
/// 8 and r1; a load's, a store's, a prefixed one's), if `i` has one.
pub fn memory(i: &Instruction) -> Option<(i64, Reg)> {
    let t = i.target();
    let k = i.inst().ops.iter().position(|op| {
        t.kinds
            .get(usize::from(op.kind))
            .is_some_and(|kind| kind.print == turbonop_tablegen::Print::MemRegImm)
    })?;
    let ops = i.operands();
    Some((ops.get(k)?.imm()?, ops.get(k + 1)?.reg()?))
}

/// The register class of `i`'s operand `k`, by name (`GPRC`, `VSRC`, ...;
/// empty for an immediate).
pub fn class_of(i: &Instruction, k: usize) -> &'static str {
    let t = i.target();
    i.inst()
        .ops
        .get(k)
        .and_then(|o| t.kinds.get(usize::from(o.kind)))
        .and_then(|kind| kind.class)
        .and_then(|c| t.classes.get(usize::from(c)))
        .map_or("", |c| c.name)
}

#[cfg(test)]
mod tests;

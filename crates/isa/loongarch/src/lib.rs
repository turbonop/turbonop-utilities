//! LoongArch instruction decoding: what LLVM's disassembler decodes (LA32
//! and LA64's base, the F and D floating point instructions, the LSX and
//! LASX vector extensions, LVZ's virtualization instructions, LBT's binary
//! translation ones, and LA v1.1's and v1.2's additions), from LLVM's
//! TableGen records (see `tools/loongarch/extract.py`, which writes
//! `tables.rs`, and `turbonop-tablegen`, which decodes by them).
//!
//! [`decode`] finds the instruction a 32-bit word is, as LLVM does; LA64's
//! own instructions (`ld.d`, `add.d`, ...) only with [`Features::la64`]
//! (LLVM decodes the others' with none of their features on).
//! [`format`] writes it as LLVM's disassembler does (aliases: `nop`,
//! `move`, `ret`, `jr`, `bltz`, ...). Neither panics.

#![no_std]

#[cfg(test)]
use turbonop_global_alloc as _;

extern crate alloc;

mod tables;

pub mod format;

pub use turbonop_tablegen::{Inst, Instruction, Op, Operand, Reg};

/// The integer registers' file.
pub const R: u8 = 0;
/// The floating point registers' file (32- and 64-bit views alike).
pub const F: u8 = 1;
/// LSX's 128-bit vector registers' file.
pub const VR: u8 = 2;
/// LASX's 256-bit vector registers' file.
pub const XR: u8 = 3;
/// The condition flag registers' file (`$fcc0` to `$fcc7`).
pub const FCC: u8 = 4;
/// The floating point control and status registers' file.
pub const FCSR: u8 = 5;
/// LBT's scratch registers' file.
pub const SCR: u8 = 6;

/// Integer register `n` (`$r0` to `$r31`).
pub const fn r(n: u8) -> Reg {
    Reg::new(R, n)
}

/// The return address register, `$ra` (`$r1`).
pub const RA: Reg = r(1);
/// The stack pointer, `$sp` (`$r3`).
pub const SP: Reg = r(3);

/// An integer register's number, if `reg` is one.
pub fn gpr(reg: Reg) -> Option<u8> {
    (reg.file == R).then_some(reg.n)
}

/// An operand's integer register's number, if it's one.
pub fn gpr_of(o: Operand) -> Option<u8> {
    o.reg().and_then(gpr)
}

/// The most operands an instruction has.
pub const MAX_OPERANDS: usize = turbonop_tablegen::MAX_OPERANDS;

/// A set of features: what an image's code may use. Of LLVM's, only
/// `64bit` changes what decodes (its disassembler checks no other).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Features(turbonop_tablegen::Features);

impl Features {
    /// LA32's.
    pub fn la32() -> Self {
        Self::of(&["32bit", "f", "d", "lsx", "lasx", "lvz", "lbt"])
    }

    /// LA64's: LA32's and LA64's own instructions.
    pub fn la64() -> Self {
        let mut f = Self::la32();
        f.0.extend(&Self::of(&["64bit"]).0);
        f
    }

    /// The features named (as LLVM names them), and what they imply.
    /// Names it doesn't know are left out.
    pub fn of(names: &[&str]) -> Self {
        Self(turbonop_tablegen::Features::of(&tables::TARGET, names))
    }

    /// Whether a feature (by name) is on.
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains(&tables::TARGET, name)
    }

    /// Whether the code is LA64's.
    pub fn la64_on(&self) -> bool {
        self.contains("64bit")
    }
}

/// The instruction at the start of `bytes` (a little-endian word), with
/// `features` on; `None` if it isn't one (or `bytes` is too short).
pub fn decode(bytes: &[u8], features: &Features) -> Option<Instruction> {
    let w = u64::from(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?));
    let t = &tables::TARGET;
    let i = turbonop_tablegen::decode(t, w, 4, &features.0, false, false)?;
    // `ud` is `amswap.w` with rk 1 and rd rj: otherwise it's `amswap.w`
    // (as LLVM's DecodeAMOrUDInstruction decides, whatever the features).
    if i.name() == "UD" && (w >> 5) & 31 != w & 31 {
        return turbonop_tablegen::decode_as(t, "AMSWAP_W", w, false, false);
    }
    Some(i)
}

/// Where a branch goes, if `i` is one with a target (`b`, `bl`, `beq`,
/// `beqz`, `bceqz`, ...; not `jirl`): its address plus its offset.
pub fn branch_target(i: &Instruction, address: u64) -> Option<u64> {
    let inst = i.inst();
    let t = i.target();
    inst.ops.iter().enumerate().find_map(|(k, op)| {
        let kind = t.kinds.get(usize::from(op.kind))?;
        if kind.print != turbonop_tablegen::Print::Branch {
            return None;
        }
        let off = i.operands().get(k)?.imm()?;
        Some(address.wrapping_add_signed(off))
    })
}

#[cfg(test)]
mod tests;

//! MIPS instruction decoding: what LLVM's disassembler decodes in MIPS's
//! standard encoding (MIPS I to MIPS64 Release 6, with MSA, the DSP ASE,
//! the virtualization and microcontroller ASEs, and Cavium's Octeon
//! instructions), from LLVM's TableGen records (see
//! `tools/mips/extract.py`, which writes `tables.rs`, and
//! `hexcavator-tablegen`, which decodes by them). microMIPS and MIPS16
//! code, whose encodings are others', isn't decoded.
//!
//! [`decode`] finds the instruction a word is, big- or little-endian, as
//! LLVM does: its decoder tables in the order it tries them, each with the
//! features it needs (Release 6's, 64-bit's, Octeon's); and, as LLVM's
//! decoder does by hand, Release 6's compact branches by their fields, and
//! `dext` and `dins` by their sizes. [`format`] writes it as LLVM does
//! (aliases: `nop`, `move`, `b`, `beqz`, `jr`, `not`, ...). Neither panics.

#![no_std]

extern crate alloc;

mod tables;

pub mod format;

pub use hexcavator_tablegen::{Inst, Instruction, Op, Operand, Reg};

/// The register files (as the tables number them).
pub mod files {
    /// The GPRs (32- and 64-bit alike).
    pub const GPR: u8 = 0;
    /// The FPRs (single, double, and paired alike).
    pub const FPR: u8 = 1;
    /// MSA's vector registers.
    pub const MSA: u8 = 2;
    /// The DSP accumulators, and their high and low halves.
    pub const AC: u8 = 3;
    pub const HI: u8 = 4;
    pub const LO: u8 = 5;
    /// The floating point condition codes.
    pub const FCC: u8 = 6;
    /// The floating point control registers.
    pub const FCR: u8 = 7;
    /// Coprocessor 0's registers, by select * 32 + register.
    pub const COP0: u8 = 8;
    /// Coprocessors 2's and 3's.
    pub const COP2: u8 = 9;
    pub const COP3: u8 = 10;
    /// The hardware registers (`rdhwr`'s).
    pub const HWR: u8 = 11;
    /// MSA's control registers.
    pub const MSACTRL: u8 = 12;
}

/// GPR `n`.
pub const fn r(n: u8) -> Reg {
    Reg::new(files::GPR, n)
}

/// A GPR's number, if `reg` is one.
pub fn gpr(reg: Reg) -> Option<u8> {
    (reg.file == files::GPR).then_some(reg.n)
}

/// An operand's GPR's number, if it's one.
pub fn gpr_of(o: Operand) -> Option<u8> {
    o.reg().and_then(gpr)
}

/// The most operands an instruction has.
pub const MAX_OPERANDS: usize = hexcavator_tablegen::MAX_OPERANDS;

/// The features code may use (an ISA level, its ASEs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Features(hexcavator_tablegen::Features);

/// ELF's `e_flags`: the ISA level, the machine, and the ASEs.
const EF_MIPS_ARCH: u32 = 0xF000_0000;
const EF_MIPS_MACH: u32 = 0x00FF_0000;
const EF_MIPS_MACH_OCTEON: u32 = 0x008B_0000;
const EF_MIPS_MICROMIPS: u32 = 0x0200_0000;
const EF_MIPS_ARCH_ASE_M16: u32 = 0x0400_0000;

impl Features {
    /// The features named (as LLVM names them: `mips32r2`, `msa`,
    /// `cnmips`, ...), and what they imply.
    pub fn of(names: &[&str]) -> Self {
        Self(hexcavator_tablegen::Features::of(&tables::TARGET, names))
    }

    /// The features llvm-objdump takes an ELF image's code to have: its
    /// default CPU's (`mips32`, or `mips64` for 64-bit code), and those
    /// its `e_flags` name (the ISA level, Octeon's, microMIPS, MIPS16).
    pub fn from_elf(flags: u32, mips64: bool) -> Self {
        let mut names = alloc::vec![if mips64 { "mips64" } else { "mips32" }];
        let arch = match flags & EF_MIPS_ARCH {
            0x1000_0000 => Some("mips2"),
            0x2000_0000 => Some("mips3"),
            0x3000_0000 => Some("mips4"),
            0x4000_0000 => Some("mips5"),
            0x5000_0000 => Some("mips32"),
            0x6000_0000 => Some("mips64"),
            0x7000_0000 => Some("mips32r2"),
            0x8000_0000 => Some("mips64r2"),
            0x9000_0000 => Some("mips32r6"),
            0xA000_0000 => Some("mips64r6"),
            _ => None,
        };
        names.extend(arch);
        if flags & EF_MIPS_MACH == EF_MIPS_MACH_OCTEON {
            names.push("cnmips");
        }
        if flags & EF_MIPS_MICROMIPS != 0 {
            names.push("micromips");
        }
        if flags & EF_MIPS_ARCH_ASE_M16 != 0 {
            names.push("mips16");
        }
        Self::of(&names)
    }

    /// Whether a feature (by name) is on.
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains(&tables::TARGET, name)
    }
}

/// The table a decoded instruction is from, as an index (to go on past).
fn space_of(i: &Instruction) -> u8 {
    i.inst().space
}

/// The instruction of word `w` at the start of `bytes` (big-endian if
/// `big`), with `features` on; `None` if it isn't one, or is microMIPS or
/// MIPS16 code (which isn't decoded), or `bytes` is too short.
pub fn decode(bytes: &[u8], big: bool, features: &Features) -> Option<Instruction> {
    if features.contains("micromips") || features.contains("mips16") {
        return None;
    }
    let b: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    let w = if big {
        u32::from_be_bytes(b)
    } else {
        u32::from_le_bytes(b)
    };
    let t = &tables::TARGET;
    let mut first = 0u8;
    loop {
        let i =
            hexcavator_tablegen::decode_from(t, u64::from(w), 4, &features.0, first, false, false)?;
        match by_hand(&i, w) {
            Some(Some(j)) => return Some(j),
            Some(None) => first = space_of(&i) + 1,
            None => return Some(i),
        }
    }
}

/// What LLVM's decoder does by hand for `i` (decoded from `w`): `Some` of
/// the instruction it is instead, or of `None` if it isn't one here (LLVM
/// goes on to its next table); `None` if it decodes as the tables say.
fn by_hand(i: &Instruction, w: u32) -> Option<Option<Instruction>> {
    let t = &tables::TARGET;
    let rs = (w >> 21) & 31;
    let rt = (w >> 16) & 31;
    let as_ = |name: &str| hexcavator_tablegen::decode_as(t, name, u64::from(w), false, false);
    // Release 6's compact branches: each group's encoding, its fields
    // compared (LLVM's Decode*GroupBranch).
    let group = match i.name() {
        "BOVC" | "BEQZALC" | "BEQC" | "BEQC64" => Some(if rs >= rt {
            "BOVC"
        } else if rs != 0 {
            "BEQC"
        } else {
            "BEQZALC"
        }),
        "BNVC" | "BNEZALC" | "BNEC" | "BNEC64" => Some(if rs >= rt {
            "BNVC"
        } else if rs != 0 {
            "BNEC"
        } else {
            "BNEZALC"
        }),
        "BLEZC" | "BGEZC" | "BGEC" | "BLEZC64" | "BGEZC64" | "BGEC64" => {
            if rt == 0 {
                return Some(None);
            }
            Some(if rs == 0 {
                "BLEZC"
            } else if rs == rt {
                "BGEZC"
            } else {
                "BGEC"
            })
        }
        "BGTZC" | "BLTZC" | "BLTC" | "BGTZC64" | "BLTZC64" | "BLTC64" => {
            if rt == 0 {
                return Some(None);
            }
            Some(if rs == 0 {
                "BGTZC"
            } else if rs == rt {
                "BLTZC"
            } else {
                "BLTC"
            })
        }
        "BLEZALC" | "BGEZALC" | "BGEUC" | "BGEUC64" => {
            if rt == 0 {
                return Some(None);
            }
            Some(if rs == 0 {
                "BLEZALC"
            } else if rs == rt {
                "BGEZALC"
            } else {
                "BGEUC"
            })
        }
        "BGTZALC" | "BLTZALC" | "BLTUC" | "BLTUC64" => Some(if rt == 0 {
            "BGTZ"
        } else if rs == 0 {
            "BGTZALC"
        } else if rs == rt {
            "BLTZALC"
        } else {
            "BLTUC"
        }),
        _ => None,
    };
    if let Some(name) = group {
        return Some(as_(name));
    }
    // `dext` and `dins` and their forms for high bits: always written as
    // `dext` and `dins`, their position and size computed (LLVM's
    // DecodeDEXT and DecodeDINS).
    let msbd = i64::from((w >> 11) & 31);
    let lsb = i64::from((w >> 6) & 31);
    let (name, pos, size) = match i.name() {
        "DEXT" => ("DEXT", lsb, msbd + 1),
        "DEXTM" => ("DEXT", lsb, msbd + 33),
        "DEXTU" => ("DEXT", lsb + 32, msbd + 1),
        "DINS" => ("DINS", lsb, msbd + 1 - lsb),
        "DINSM" => ("DINS", lsb, msbd + 33 - lsb),
        "DINSU" => ("DINS", lsb + 32, msbd + 33 - (lsb + 32)),
        _ => return None,
    };
    Some(hexcavator_tablegen::build(
        t,
        name,
        4,
        &[
            Operand::Reg(r(u8::try_from(rt).ok()?)),
            Operand::Reg(r(u8::try_from(rs).ok()?)),
            Operand::Imm(pos),
            Operand::Imm(size),
        ],
    ))
}

/// Whether `i` has a delay slot: the instruction after it runs before it
/// takes effect (a branch's or jump's but Release 6's compact ones).
pub fn has_delay_slot(i: &Instruction) -> bool {
    tables::DELAY_SLOT.binary_search(&i.index()).is_ok()
}

/// Where a branch or jump goes, if `i` is one with a target: a branch's
/// address plus its offset (from its delay slot), a jump's region (its
/// delay slot's top 4 bits) and its 28 bits.
pub fn branch_target(i: &Instruction, address: u64) -> Option<u64> {
    let t = i.target();
    i.inst().ops.iter().enumerate().find_map(|(k, op)| {
        let kind = t.kinds.get(usize::from(op.kind))?;
        let v = i.operands().get(k)?.imm()?;
        match kind.print {
            hexcavator_tablegen::Print::Branch => Some(address.wrapping_add_signed(v)),
            hexcavator_tablegen::Print::AbsBranch => {
                Some((address.wrapping_add(4) & !0x0FFF_FFFF) | v.cast_unsigned())
            }
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests;

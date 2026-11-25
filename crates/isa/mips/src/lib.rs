//! MIPS instruction decoding: what LLVM's disassembler decodes in MIPS's
//! standard encoding (MIPS I to MIPS64 Release 6, with MSA, the DSP ASE,
//! the virtualization and microcontroller ASEs, and Cavium's Octeon
//! instructions) and in microMIPS's (its 16- and 32-bit instructions, and
//! Release 6's), from LLVM's TableGen records (see `tools/mips/extract.py`,
//! which writes `tables.rs` and `micromips.rs`, and `turbonop-tablegen`,
//! which decodes by them). MIPS16 code, which LLVM doesn't decode, isn't.
//!
//! [`decode`] finds the instruction a word is, big- or little-endian, as
//! LLVM does: its decoder tables in the order it tries them, each with the
//! features it needs (Release 6's, 64-bit's, Octeon's); and, as LLVM's
//! decoder does by hand, Release 6's compact branches by their fields, and
//! `dext` and `dins` by their sizes. With `micromips` on, it decodes
//! microMIPS code ([`decode_micromips`]). [`format`] writes it as LLVM does
//! (aliases: `nop`, `move`, `b`, `beqz`, `jr`, `not`, ...). Neither panics.

#![no_std]

extern crate alloc;

mod micromips;
mod tables;

pub mod format;

pub use turbonop_tablegen::{Inst, Instruction, Op, Operand, Reg};

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
pub const MAX_OPERANDS: usize = turbonop_tablegen::MAX_OPERANDS;

/// The features code may use (an ISA level, its ASEs).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Features(turbonop_tablegen::Features);

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
        Self(turbonop_tablegen::Features::of(&tables::TARGET, names))
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

/// The instruction at the start of `bytes` (big-endian if `big`), with
/// `features` on: a word's, or with `micromips` on, a microMIPS
/// instruction's ([`decode_micromips`]); `None` if it isn't one, or is
/// MIPS16 code (which isn't decoded), or `bytes` is too short.
pub fn decode(bytes: &[u8], big: bool, features: &Features) -> Option<Instruction> {
    if features.contains("micromips") {
        return decode_micromips(bytes, big, features);
    }
    if features.contains("mips16") {
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
            turbonop_tablegen::decode_from(t, u64::from(w), 4, &features.0, first, false, false)?;
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
    let as_ = |name: &str| turbonop_tablegen::decode_as(t, name, u64::from(w), false, false);
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
    Some(turbonop_tablegen::build(
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

/// The microMIPS instruction at the start of `bytes` (big-endian if
/// `big`), with `features` on, as LLVM decodes microMIPS code: a 16-bit
/// instruction, if the first halfword is one; else a 32-bit one, its
/// first halfword its high one. `None` if it is neither (LLVM goes on 2
/// bytes later), or `bytes` is too short.
pub fn decode_micromips(bytes: &[u8], big: bool, features: &Features) -> Option<Instruction> {
    let half = |k: usize| -> Option<u64> {
        let b: [u8; 2] = bytes.get(k..k + 2)?.try_into().ok()?;
        Some(u64::from(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        }))
    };
    let t = &micromips::TARGET;
    let high = half(0)?;
    if let Some(i) = turbonop_tablegen::decode_from(t, high, 2, &features.0, 0, false, false) {
        return Some(i);
    }
    let w = high << 16 | half(2)?;
    let mut first = 0u8;
    loop {
        let i = turbonop_tablegen::decode_from(t, w, 4, &features.0, first, false, false)?;
        match micromips_by_hand(&i, w) {
            Some(Some(j)) => return Some(j),
            Some(None) => first = space_of(&i) + 1,
            None => return Some(i),
        }
    }
}

/// What LLVM's microMIPS decoder does by hand for `i` (decoded from the
/// 32 bits `w`): Release 6's compact branches, by their fields (its
/// Decode*GroupBranchMMR6), their offsets in halfwords or words as it
/// has them. As [`by_hand`] says.
fn micromips_by_hand(i: &Instruction, w: u64) -> Option<Option<Instruction>> {
    let t = &micromips::TARGET;
    let rt = u8::try_from((w >> 21) & 31).ok()?;
    let rs = u8::try_from((w >> 16) & 31).ok()?;
    let offset = i64::from((w & 0xFFFF) as u16 as i16);
    let (half, word) = (Operand::Imm(offset * 2 + 4), Operand::Imm(offset * 4 + 4));
    let (rt_, rs_) = (Operand::Reg(r(rt)), Operand::Reg(r(rs)));
    let build =
        |name: &str, ops: &[Operand]| Some(Some(turbonop_tablegen::build(t, name, 4, ops)?));
    let name = i.name();
    let either = |a: &str, b: &str, c: &str| name == a || name == b || name == c;
    if either("BOVC_MMR6", "BEQC_MMR6", "BEQZALC_MMR6")
        || either("BNVC_MMR6", "BNEC_MMR6", "BNEZALC_MMR6")
    {
        let eq = name.starts_with("BOVC") || name.starts_with("BEQ");
        return if rs >= rt {
            build(
                if eq { "BOVC_MMR6" } else { "BNVC_MMR6" },
                &[rt_, rs_, half],
            )
        } else if rs != 0 {
            build(
                if eq { "BEQC_MMR6" } else { "BNEC_MMR6" },
                &[rs_, rt_, word],
            )
        } else {
            build(
                if eq { "BEQZALC_MMR6" } else { "BNEZALC_MMR6" },
                &[rt_, half],
            )
        };
    }
    // (Each group: its form without rs, with rs equal to rt, and with
    // both; none if rt is 0.)
    let groups: [(&str, &str, &str, bool); 4] = [
        ("BGTZC_MMR6", "BLTZC_MMR6", "BLTC_MMR6", false),
        ("BLEZC_MMR6", "BGEZC_MMR6", "BGEC_MMR6", false),
        ("BLEZALC_MMR6", "BGEZALC_MMR6", "BGEUC_MMR6", true),
        ("BGTZALC_MMR6", "BLTZALC_MMR6", "BLTUC_MMR6", true),
    ];
    let &(none, same, both, linked) = groups.iter().find(|g| either(g.0, g.1, g.2))?;
    // (A linking one's single register offset is in halfwords.)
    let one = if linked { half } else { word };
    if rt == 0 {
        Some(None)
    } else if rs == 0 {
        build(none, &[rt_, one])
    } else if rs == rt {
        build(same, &[if none == "BGTZALC_MMR6" { rs_ } else { rt_ }, one])
    } else {
        build(both, &[rs_, rt_, word])
    }
}

/// Whether `i` is a microMIPS instruction (from [`decode_micromips`]).
pub fn is_micromips(i: &Instruction) -> bool {
    core::ptr::eq(i.target(), &micromips::TARGET)
}

/// How many bytes `i` is: 4, or a 16-bit microMIPS instruction's 2.
pub fn size(i: &Instruction) -> usize {
    usize::from(i.inst().size)
}

/// Whether `i` has a delay slot: the instruction after it runs before it
/// takes effect (a branch's or jump's but Release 6's compact ones).
pub fn has_delay_slot(i: &Instruction) -> bool {
    let slots = if is_micromips(i) {
        micromips::DELAY_SLOT
    } else {
        tables::DELAY_SLOT
    };
    slots.binary_search(&i.index()).is_ok()
}

/// Where a branch or jump goes, if `i` is one with a target: a branch's
/// address plus its offset (from its delay slot, or the instruction after
/// it), a jump's region (its delay slot's top bits) and its 28 bits (a
/// microMIPS jump's 27).
pub fn branch_target(i: &Instruction, address: u64) -> Option<u64> {
    use turbonop_tablegen::Decode;
    let t = i.target();
    let micro = is_micromips(i);
    i.inst().ops.iter().enumerate().find_map(|(k, op)| {
        let kind = t.kinds.get(usize::from(op.kind))?;
        let v = i.operands().get(k)?.imm()?;
        match kind.print {
            // (A microMIPS branch's offset LLVM has from the branch, but
            // the instruction after it is what it is from: but those
            // whose offsets LLVM gives from 4 on, from the next 32 bits.)
            turbonop_tablegen::Print::Branch => {
                let next = if micro && !matches!(kind.decode, Decode::BranchPlus4 { .. }) {
                    size(i) as u64
                } else {
                    0
                };
                Some(address.wrapping_add_signed(v).wrapping_add(next))
            }
            turbonop_tablegen::Print::AbsBranch => {
                let bits = match kind.decode {
                    Decode::UImmLsl { bits, arg } => bits + u32::try_from(arg).ok()?,
                    _ => 28,
                };
                let region = !((1u64 << bits) - 1);
                Some((address.wrapping_add(4) & region) | v.cast_unsigned())
            }
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests;

// (Tests allocate with mimalloc, as the programs do.)
#[cfg(all(test, not(target_family = "wasm")))]
#[global_allocator]
static TEST_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

//! MIPS instructions as text, as LLVM's disassembler writes them: as the
//! alias LLVM prints it as if any applies (its TableGen aliases, `nop`,
//! `li`, ...; then those its printer knows by hand: `b`, `beqz`, `bnez`,
//! `bal`, `jr`, `jalr`, `not`, `move`); `rdhwr` inside `.set push` and
//! `.set pop`, as LLVM writes it; registers as `$` and LLVM's names for
//! them (`$sp`, `$4`, `$f0`); immediates in hex (llvm-objdump's default);
//! branch targets as addresses.

use alloc::string::String;
use core::fmt::Write as _;

use hexcavator_tablegen::Print;

use crate::{Features, Instruction, Operand, Reg, files, tables};

/// A register's name, as LLVM writes it (without its `$`).
pub fn reg_name(r: Reg) -> String {
    if r.file == files::COP0 {
        return alloc::format!("{}", r.n % 32);
    }
    tables::REG_NAMES
        .get(usize::from(r.file))
        .and_then(|names| names.get(usize::from(r.n)))
        .map_or_else(|| alloc::format!("{}", r.n), |&n| n.into())
}

/// The floating point conditions' names (MipsFCCToString's).
const FCC: [&str; 16] = [
    "f", "un", "eq", "ueq", "olt", "ult", "ole", "ule", "sf", "ngle", "seq", "ngl", "lt", "nge",
    "le", "ngt",
];

/// The instruction as LLVM's disassembler writes it, at `address`.
pub fn format(i: &Instruction, address: u64, features: &Features) -> String {
    let mut out = String::new();
    let rdhwr = matches!(i.name(), "RDHWR" | "RDHWR64");
    if rdhwr {
        out.push_str("\t.set\tpush\n\t.set\tmips32r2\n");
    }
    // (LLVM's TableGen aliases first, then its printer's own; else the
    // instruction's syntax.)
    if has_alias(i, features) || !by_hand(&mut out, i, address, features) {
        hexcavator_tablegen::write(&mut out, i, &features.0, |out, op, print, _| {
            operand(out, i, op, print, address, features);
        });
    }
    if rdhwr {
        out.push_str("\n\t.set\tpop");
    }
    out
}

/// Whether one of LLVM's TableGen aliases applies to `i`.
fn has_alias(i: &Instruction, features: &Features) -> bool {
    let t = i.target();
    t.aliases.iter().any(|a| {
        usize::from(a.inst) == usize::from(i.index())
            && features.0.satisfies(t, a.pred)
            && hexcavator_tablegen::passes(t, i.operands(), a.checks)
    })
}

/// The aliases LLVM's printer knows by hand (MipsInstPrinter::printAlias).
fn by_hand(out: &mut String, i: &Instruction, address: u64, features: &Features) -> bool {
    let reg = |k: usize| i.operands().get(k).and_then(|o| o.reg());
    let is = |k: usize, r: Reg| reg(k) == Some(r);
    let zero = crate::r(0);
    let ra = crate::r(31);
    let fcc0 = Reg::new(files::FCC, 0);
    let alias = |out: &mut String, name: &str, ops: &[(usize, bool)]| {
        out.push('\t');
        out.push_str(name);
        out.push('\t');
        for (n, &(k, branch)) in ops.iter().enumerate() {
            if n > 0 {
                out.push_str(", ");
            }
            let print = if branch {
                Print::Branch
            } else {
                Print::Operand
            };
            operand(out, i, k, print, address, features);
        }
        true
    };
    match i.name() {
        "BEQ" | "BEQ_MM" => {
            if is(0, zero) && is(1, zero) {
                alias(out, "b", &[(2, true)])
            } else if is(1, zero) {
                alias(out, "beqz", &[(0, false), (2, true)])
            } else {
                false
            }
        }
        "BEQ64" => is(1, zero) && alias(out, "beqz", &[(0, false), (2, true)]),
        "BNE" | "BNE_MM" | "BNE64" => is(1, zero) && alias(out, "bnez", &[(0, false), (2, true)]),
        "BGEZAL" => is(0, zero) && alias(out, "bal", &[(1, true)]),
        "BC1T" => is(0, fcc0) && alias(out, "bc1t", &[(1, true)]),
        "BC1F" => is(0, fcc0) && alias(out, "bc1f", &[(1, true)]),
        "JALR" | "JALR64" => {
            if is(0, zero) {
                alias(out, "jr", &[(1, false)])
            } else if is(0, ra) {
                alias(out, "jalr", &[(1, false)])
            } else {
                false
            }
        }
        "NOR" | "NOR_MM" | "NOR_MMR6" | "NOR64" => {
            is(2, zero) && alias(out, "not", &[(0, false), (1, false)])
        }
        "OR" | "ADDu" => is(2, zero) && alias(out, "move", &[(0, false), (1, false)]),
        _ => false,
    }
}

/// An operand as LLVM's printer writes it.
fn operand(
    out: &mut String,
    i: &Instruction,
    op: usize,
    print: Print,
    address: u64,
    features: &Features,
) {
    let ops = i.operands();
    let plain = |out: &mut String, k: usize| match ops.get(k) {
        Some(Operand::Reg(r)) => {
            out.push('$');
            out.push_str(&reg_name(*r));
        }
        Some(Operand::Imm(v)) => hex(out, *v),
        None => {}
    };
    let imm = ops.get(op).and_then(|o| o.imm()).unwrap_or(0);
    match print {
        Print::Branch => {
            let mut target = address.wrapping_add_signed(imm);
            // (As LLVM's printer: 32 bits, wherever MIPS32 is on.)
            if features.contains("mips32") {
                target &= 0xFFFF_FFFF;
            }
            let _ = write!(out, "{target:#x}");
        }
        Print::AbsBranch => {
            let _ = write!(out, "{:#x}", imm.cast_unsigned());
        }
        Print::UImm { bits, offset } => {
            let offset = u64::from(offset);
            let mask = if bits >= 64 {
                u64::MAX
            } else {
                (1u64 << bits) - 1
            };
            let v = (imm.cast_unsigned().wrapping_sub(offset) & mask).wrapping_add(offset);
            hex(out, v.cast_signed());
        }
        Print::Mem => {
            plain(out, op + 1);
            out.push('(');
            plain(out, op);
            out.push(')');
        }
        Print::Cop0 => {
            if let Some(Operand::Reg(r)) = ops.get(op) {
                let _ = write!(out, "${}, {}", r.n % 32, r.n / 32);
            }
        }
        Print::Fcc => out.push_str(FCC[(imm & 15) as usize]),
        Print::FromEnd(n) => plain(out, ops.len().saturating_sub(n as usize)),
        // microMIPS's register lists: a count of registers from $16 (the
        // ninth $fp), and $ra (bit 4).
        Print::RegList => {
            let count = u8::try_from(imm & 0xF).unwrap_or(0);
            let mut regs: alloc::vec::Vec<u8> = (0..count.min(9))
                .map(|k| if k == 8 { 30 } else { 16 + k })
                .collect();
            if imm & 0x10 != 0 {
                regs.push(31);
            }
            for (n, r) in regs.into_iter().enumerate() {
                if n > 0 {
                    out.push_str(", ");
                }
                out.push('$');
                out.push_str(&reg_name(crate::r(r)));
            }
        }
        _ => plain(out, op),
    }
}

/// An immediate as llvm-objdump writes it (in hex: `0x10`, `-0x8`).
fn hex(out: &mut String, v: i64) {
    if v < 0 {
        let _ = write!(out, "-{:#x}", v.unsigned_abs());
    } else {
        let _ = write!(out, "{v:#x}");
    }
}

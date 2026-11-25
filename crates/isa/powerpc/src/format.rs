//! PowerPC instructions as text, as LLVM's disassembler writes them (as
//! llvm-objdump does, its registers as numbers): as the alias LLVM prints
//! it as if any applies (`li`, `mr`, `blr`, `bt`, `mflr`, ...), and the
//! shifts and cache hints LLVM's printer writes by hand (`slwi`, `srwi`,
//! `sldi`, `dcbt`, `dcbf`, ...); registers as their numbers (a VSX
//! operand's Altivec register as 32 and up); immediates in decimal; branch
//! targets as addresses.

use alloc::string::String;
use core::fmt::Write as _;

use turbonop_tablegen::Print;

use crate::{Features, Instruction, Operand, Reg, files};

/// The register classes whose operands number Altivec's registers 32 to
/// 63 (VSX's view), not 0 to 31.
const VSX_CLASSES: [&str; 3] = ["VSRC", "VSFRC", "VSSRC"];

/// A register as LLVM writes it for `class` (its operand's register
/// class), its prefix stripped as llvm-objdump does.
pub fn reg_name(r: Reg, class: &str) -> String {
    use files::*;
    let n = u32::from(r.n);
    match r.file {
        VS if n >= 32 && !VSX_CLASSES.contains(&class) => alloc::format!("{}", n - 32),
        VSP => alloc::format!("{}", 2 * n),
        _ if r.is_none() => String::new(),
        _ => alloc::format!("{n}"),
    }
}

/// The instruction as LLVM's disassembler writes it, at `address`.
pub fn format(i: &Instruction, address: u64, features: &Features) -> String {
    let mut out = String::new();
    if by_hand(&mut out, i, features) {
        return out;
    }
    turbonop_tablegen::write(&mut out, i, &features.set, |out, op, print, _| {
        print_operand(out, i, op, print, address, features);
    });
    out
}

/// An operand as LLVM writes it: a register as its number (for its
/// operand's class), an immediate in decimal.
fn operand(out: &mut String, i: &Instruction, k: usize) {
    match i.operands().get(k) {
        Some(Operand::Reg(r)) => out.push_str(&reg_name(*r, crate::class_of(i, k))),
        Some(Operand::Imm(v)) => {
            let _ = write!(out, "{v}");
        }
        None => {}
    }
}

/// A base register as LLVM writes it: `r0` as 0 (which it reads as).
fn base(out: &mut String, i: &Instruction, k: usize) {
    match i.operands().get(k) {
        Some(Operand::Reg(r)) if r.file == files::R && r.n == 0 => out.push('0'),
        _ => operand(out, i, k),
    }
}

fn print_operand(
    out: &mut String,
    i: &Instruction,
    op: usize,
    print: Print,
    address: u64,
    features: &Features,
) {
    match print {
        Print::Branch | Print::AbsBranch => {
            if let Some(to) = crate::branch_target(i, address, features) {
                let _ = write!(out, "{to:#x}");
            }
        }
        Print::CrBitM => {
            if let Some(Operand::Reg(r)) = i.operands().get(op) {
                let _ = write!(out, "{}", 0x80u32 >> r.n.min(7));
            }
        }
        Print::MemRegImm => {
            operand(out, i, op);
            out.push('(');
            base(out, i, op + 1);
            out.push(')');
        }
        Print::MemRegReg => {
            base(out, i, op);
            out.push_str(", ");
            operand(out, i, op + 1);
        }
        _ => operand(out, i, op),
    }
}

/// The instructions LLVM's printer writes by hand: `slwi`, `srwi`,
/// `sldi`, and the cache hints `dcbt`, `dcbtst`, `dcbf`.
fn by_hand(out: &mut String, i: &Instruction, features: &Features) -> bool {
    let imm = |k: usize| i.operands().get(k).and_then(|o| o.imm()).unwrap_or(0);
    let reg = |out: &mut String, k: usize| {
        if let Some(Operand::Reg(r)) = i.operands().get(k) {
            out.push_str(&reg_name(*r, crate::class_of(i, k)));
        }
    };
    match i.name() {
        "RLWINM" => {
            let (sh, mb, me) = (imm(2), imm(3), imm(4));
            let (name, sh) = if sh <= 31 && mb == 0 && me == 31 - sh {
                ("slwi", sh)
            } else if sh <= 31 && mb == 32 - sh && me == 31 {
                ("srwi", 32 - sh)
            } else {
                return false;
            };
            let _ = write!(out, "\t{name} ");
            reg(out, 0);
            out.push_str(", ");
            reg(out, 1);
            let _ = write!(out, ", {sh}");
            true
        }
        "RLDICR" | "RLDICR_32" => {
            let (sh, me) = (imm(2), imm(3));
            if 63 - sh != me {
                return false;
            }
            out.push_str("\tsldi ");
            reg(out, 0);
            out.push_str(", ");
            reg(out, 1);
            let _ = write!(out, ", {sh}");
            true
        }
        "DCBT" | "DCBTST" => {
            let th = imm(0);
            out.push_str("\tdcbt");
            if i.name() == "DCBTST" {
                out.push_str("st");
            }
            if th == 16 {
                out.push('t');
            }
            out.push(' ');
            let booke = features.contains("booke");
            if booke && th != 0 && th != 16 {
                let _ = write!(out, "{th}, ");
            }
            reg(out, 1);
            out.push_str(", ");
            reg(out, 2);
            if !booke && th != 0 && th != 16 {
                let _ = write!(out, ", {th}");
            }
            true
        }
        "DCBF" => {
            let l = imm(0);
            if !matches!(l, 0 | 1 | 3 | 4 | 6) {
                return false;
            }
            out.push_str("\tdcb");
            if l != 6 {
                out.push('f');
            }
            out.push_str(match l {
                1 => "l",
                3 => "lp",
                4 => "ps",
                6 => "stps",
                _ => "",
            });
            out.push(' ');
            reg(out, 1);
            out.push_str(", ");
            reg(out, 2);
            true
        }
        _ => false,
    }
}

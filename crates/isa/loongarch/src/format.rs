//! LoongArch instructions as text, as LLVM's disassembler writes them:
//! as the alias LLVM prints it as if any applies (`nop`, `move`, `ret`,
//! `jr`, `bltz`, ...); registers by their ABI names, `$`-prefixed;
//! immediates in hex (llvm-objdump's default), branches' as their offsets
//! (as LLVM writes them; [`crate::branch_target`] is where they go).

use alloc::string::String;
use core::fmt::Write as _;

use crate::{F, FCC, FCSR, Features, Instruction, Operand, R, Reg, SCR, VR, XR};

/// The integer registers' ABI names (`$r21` has none).
const R_NAMES: [&str; 32] = [
    "zero", "ra", "tp", "sp", "a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7", "t0", "t1", "t2",
    "t3", "t4", "t5", "t6", "t7", "t8", "r21", "fp", "s0", "s1", "s2", "s3", "s4", "s5", "s6",
    "s7", "s8",
];

/// A register's name (`a0`, `fa1`, `vr2`, `fcc0`), without its `$`; none's
/// is empty.
pub fn reg_name(r: Reg) -> String {
    let n = r.n;
    match r.file {
        R => R_NAMES.get(usize::from(n)).copied().unwrap_or("?").into(),
        F => match n {
            0..8 => alloc::format!("fa{n}"),
            8..24 => alloc::format!("ft{}", n - 8),
            _ => alloc::format!("fs{}", n - 24),
        },
        VR => alloc::format!("vr{n}"),
        XR => alloc::format!("xr{n}"),
        FCC => alloc::format!("fcc{n}"),
        FCSR => alloc::format!("fcsr{n}"),
        SCR => alloc::format!("scr{n}"),
        _ => String::new(),
    }
}

/// The instruction as LLVM's disassembler writes it. (`address` is
/// unused, branches being written as offsets; it's for a like signature
/// with the other instruction sets'.)
pub fn format(i: &Instruction, address: u64, features: &Features) -> String {
    let _ = address;
    let mut out = String::new();
    hexcavator_tablegen::write(&mut out, i, &features.0, |out, op, _, _| {
        // (Every LoongArch operand prints as itself: registers by name,
        // immediates and branch offsets in hex.)
        let Some(&o) = i.operands().get(op) else {
            return;
        };
        match o {
            Operand::Reg(r) => {
                out.push('$');
                out.push_str(&reg_name(r));
            }
            Operand::Imm(v) => hex(out, v),
        }
    });
    // (`ertn`'s syntax ends in a tab, as instructions without operands'
    // may: LLVM's text doesn't.)
    out.truncate(out.trim_end().len());
    out
}

/// An immediate as llvm-objdump writes it (in hex: `0x10`, `-0x8`).
fn hex(out: &mut String, v: i64) {
    if v < 0 {
        let _ = write!(out, "-{:#x}", v.unsigned_abs());
    } else {
        let _ = write!(out, "{v:#x}");
    }
}

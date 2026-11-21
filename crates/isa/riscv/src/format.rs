//! RISC-V instructions as text, as LLVM's disassembler writes them: a
//! compressed instruction as its full form, then as the alias LLVM prints
//! it as if any applies (`li`, `mv`, `ret`, `j`, `beqz`, ...); registers by
//! their ABI names; immediates in hex (llvm-objdump's default); branch
//! targets as addresses;
//! CSRs, fences, rounding modes, and vector types by name.

use alloc::string::String;
use core::fmt::Write as _;

use hexcavator_tablegen::Print;

use crate::tables;
use crate::{Csr, F, Features, Instruction, Operand, Reg, V, X};

const X_NAMES: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
    "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
    "t5", "t6",
];

const F_NAMES: [&str; 32] = [
    "ft0", "ft1", "ft2", "ft3", "ft4", "ft5", "ft6", "ft7", "fs0", "fs1", "fa0", "fa1", "fa2",
    "fa3", "fa4", "fa5", "fa6", "fa7", "fs2", "fs3", "fs4", "fs5", "fs6", "fs7", "fs8", "fs9",
    "fs10", "fs11", "ft8", "ft9", "ft10", "ft11",
];

const ROUNDING: [&str; 8] = ["rne", "rtz", "rdn", "rup", "rmm", "frm5", "frm6", "dyn"];

/// Zfa's `fli` immediates, by their encoding, as LLVM writes them.
const FP_IMMS: [&str; 32] = [
    "-1.0",
    "min",
    "1.52587890625e-05",
    "3.0517578125e-05",
    "0.00390625",
    "0.0078125",
    "0.0625",
    "0.125",
    "0.25",
    "0.3125",
    "0.375",
    "0.4375",
    "0.5",
    "0.625",
    "0.75",
    "0.875",
    "1.0",
    "1.25",
    "1.5",
    "1.75",
    "2.0",
    "2.5",
    "3.0",
    "4.0",
    "8.0",
    "16.0",
    "128.0",
    "256.0",
    "32768.0",
    "65536.0",
    "inf",
    "nan",
];

/// A register's ABI name (`a0`, `fs1`, `v8`); none's is empty.
pub fn reg_name(r: Reg) -> String {
    match r.file {
        X => X_NAMES.get(usize::from(r.n)).copied().unwrap_or("?").into(),
        F => F_NAMES.get(usize::from(r.n)).copied().unwrap_or("?").into(),
        V => alloc::format!("v{}", r.n),
        _ => String::new(),
    }
}

/// The instruction as LLVM's disassembler writes it, at `address`
/// (branch targets are addresses), with `features` on (which names CSRs,
/// and which aliases and full forms apply).
pub fn format(i: &Instruction, address: u64, features: &Features) -> String {
    let full = crate::uncompress(i, features);
    let mi = full.as_ref().unwrap_or(i);
    let mut out = String::new();
    hexcavator_tablegen::write(&mut out, mi, &features.0, |out, op, print, _| {
        print_operand(out, mi, op, print, address, features);
    });
    out
}

fn print_operand(
    out: &mut String,
    i: &Instruction,
    op: usize,
    print: Print,
    address: u64,
    features: &Features,
) {
    let Some(&o) = i.operands().get(op) else {
        return;
    };
    let imm = o.imm().unwrap_or(0);
    match print {
        Print::Operand => match o {
            Operand::Reg(r) => out.push_str(&reg_name(r)),
            Operand::Imm(v) => hex(out, v),
        },
        Print::Branch => {
            let mut target = address.wrapping_add_signed(imm);
            if !features.rv64() {
                target &= 0xFFFF_FFFF;
            }
            let _ = write!(out, "{target:#x}");
        }
        Print::Csr => match csr_name(imm, features) {
            Some(n) => out.push_str(n),
            None => hex(out, imm),
        },
        Print::Fence => {
            for (bit, c) in [(8, 'i'), (4, 'o'), (2, 'r'), (1, 'w')] {
                if imm & bit != 0 {
                    out.push(c);
                }
            }
            if imm == 0 {
                out.push('0');
            }
        }
        Print::Frm => {
            // (The dynamic mode, the default, goes unsaid.)
            if imm != 7 {
                out.push_str(", ");
                out.push_str(ROUNDING[(imm & 7) as usize]);
            }
        }
        Print::FrmLegacy => {
            if imm != 0 {
                out.push_str(", ");
                out.push_str(ROUNDING[(imm & 7) as usize]);
            }
        }
        Print::FpImm => out.push_str(FP_IMMS[(imm & 31) as usize]),
        Print::ZeroOffsetMem => {
            out.push('(');
            if let Operand::Reg(r) = o {
                out.push_str(&reg_name(r));
            }
            out.push(')');
        }
        Print::VTypeI => vtype(out, imm),
        Print::RegList => reg_list(out, imm),
        Print::StackAdj | Print::NegStackAdj => {
            let rlist = i.operands().first().and_then(|o| o.imm()).unwrap_or(4);
            let mut adj = imm + stack_adj_base(rlist, features.rv64());
            if print == Print::NegStackAdj {
                adj = -adj;
            }
            let _ = write!(out, "{adj}");
        }
        Print::VMask => {
            if let Operand::Reg(r) = o
                && !r.is_none()
            {
                out.push_str(", ");
                out.push_str(&reg_name(r));
                out.push_str(".t");
            }
        }
        Print::VScale => {
            out.push_str(", ");
            if let Operand::Reg(r) = o {
                out.push_str(&reg_name(r));
            }
            out.push_str(".scale");
        }
        // (Other targets'.)
        Print::AtomicMem
        | Print::MemRegImm
        | Print::MemRegReg
        | Print::AbsBranch
        | Print::CrBitM
        | Print::UImm { .. }
        | Print::Mem
        | Print::Cop0
        | Print::Fcc
        | Print::FromEnd(_) => {
            if let Operand::Reg(r) = o {
                out.push_str(&reg_name(r));
            }
        }
        Print::RegReg => {
            if let Some(Operand::Reg(offset)) = i.operands().get(op + 1) {
                out.push_str(&reg_name(*offset));
            }
            out.push('(');
            if let Operand::Reg(r) = o {
                out.push_str(&reg_name(r));
            }
            out.push(')');
        }
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

/// A CSR's name, as LLVM's printer chooses it: of those with the number
/// whose features are on (an RV32-only name only on RV32), one that
/// needs features over one that doesn't.
pub fn csr_name(number: i64, features: &Features) -> Option<&'static str> {
    let n = u16::try_from(number).ok()?;
    let first = tables::CSRS.partition_point(|c| c.encoding < n);
    let range = tables::CSRS[first..].iter().take_while(|c| c.encoding == n);
    let ok = |c: &Csr| {
        !(c.rv32_only && features.rv64())
            && c.features.iter().all(|&f| features.has(usize::from(f)))
    };
    let specific = range.clone().any(|c| !c.features.is_empty() && ok(c));
    range
        .clone()
        .find(|c| ok(c) && (!specific || !c.features.is_empty()))
        .map(|c| c.name)
}

/// `vtype`'s text (`e32, m1, ta, ma`), or its number if it's reserved.
fn vtype(out: &mut String, v: i64) {
    let sew = 1i64 << (((v >> 3) & 7) + 3);
    let lmul = v & 7;
    let alt = v & 0x100 != 0;
    let valid = sew <= 64 && lmul != 4 && (!alt || sew < 32);
    if !valid || (v >> 9) != 0 {
        hex(out, v);
        return;
    }
    let _ = write!(out, "e{sew}");
    if alt {
        out.push_str("alt");
    }
    if lmul < 4 {
        let _ = write!(out, ", m{}", 1 << lmul);
    } else {
        let _ = write!(out, ", mf{}", 1 << (8 - lmul));
    }
    out.push_str(if v & 0x40 != 0 { ", ta" } else { ", tu" });
    out.push_str(if v & 0x80 != 0 { ", ma" } else { ", mu" });
}

/// Zcmp's register list (`{ra, s0-s11}`).
fn reg_list(out: &mut String, v: i64) {
    out.push_str("{ra");
    if v >= 5 {
        out.push_str(", s0");
    }
    if v >= 6 {
        out.push('-');
        if v == 6 {
            out.push_str("s1");
        }
    }
    if v == 7 {
        out.push_str("s2");
    }
    if v >= 8 {
        // (s3 to s9, then s11: s10 is saved with it.)
        let mut last = 19 + (v - 8);
        if v == 15 {
            last += 1;
        }
        out.push_str(&reg_name(crate::x(u8::try_from(last).unwrap_or(0))));
    }
    out.push('}');
}

fn stack_adj_base(rlist: i64, rv64: bool) -> i64 {
    let mut regs = rlist - 4 + 1;
    if rlist == 15 {
        regs += 1;
    }
    let size = if rv64 { 8 } else { 4 };
    (regs * size + 15) / 16 * 16
}

//! RISC-V instruction decoding: the standard extensions LLVM's
//! disassembler decodes (RV32 and RV64; I, M, A, F, D, Q, C and the Zc*
//! extensions, B's, the scalar crypto ones, V and its Zv* extensions, the
//! hypervisor's, Zfa, Zfh, Zicond, ...), from LLVM's TableGen records (see
//! `tools/riscv/extract.py`, which writes `tables.rs`, and
//! `turbonop-tablegen`, which decodes by them).
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

pub mod format;

pub use turbonop_tablegen::{Inst, Instruction, Op, Operand, Reg};

/// The integer registers' file.
pub const X: u8 = 0;
/// The floating point registers' file.
pub const F: u8 = 1;
/// The vector registers' file (groups by their first).
pub const V: u8 = 2;

/// Integer register `n` (`x0` to `x31`).
pub const fn x(n: u8) -> Reg {
    Reg::new(X, n)
}

/// Floating point register `n`.
pub const fn f(n: u8) -> Reg {
    Reg::new(F, n)
}

/// Vector register `n`.
pub const fn v(n: u8) -> Reg {
    Reg::new(V, n)
}

/// An integer register's number, if `r` is one.
pub fn gpr(r: Reg) -> Option<u8> {
    (r.file == X).then_some(r.n)
}

/// An operand's integer register's number, if it's one.
pub fn gpr_of(o: Operand) -> Option<u8> {
    o.reg().and_then(gpr)
}

/// A CSR: its number, name, the features its name needs, and whether the
/// name is RV32's only.
#[derive(Debug)]
pub struct Csr {
    pub encoding: u16,
    pub name: &'static str,
    pub features: &'static [u16],
    pub rv32_only: bool,
}

/// The most operands an instruction has.
pub const MAX_OPERANDS: usize = turbonop_tablegen::MAX_OPERANDS;

/// A set of features (extensions, `64bit`): what an image's code may use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Features(turbonop_tablegen::Features);

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
        Self(turbonop_tablegen::Features::none())
    }

    /// The features named (as LLVM names them), and what they imply.
    /// Names it doesn't know are left out.
    pub fn of(names: &[&str]) -> Self {
        let mut f = Self(turbonop_tablegen::Features::of(&tables::TARGET, names));
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
            // (A version's `p` between its numbers: `2p0`.)
            let name: alloc::string::String = p
                .trim_end_matches(|c: char| c.is_ascii_digit() || c == 'p')
                .into();
            if !name.is_empty() {
                names.push(name);
            }
        }
        let refs: alloc::vec::Vec<&str> = names.iter().map(alloc::string::String::as_str).collect();
        Self::of(&refs)
    }

    /// Adds these features.
    pub fn extend(&mut self, other: &Features) {
        self.0.extend(&other.0);
        self.complete();
    }

    /// What LLVM's ISA strings imply beyond the features' own
    /// implications: C with F on RV32 is Zcf too, C with D is Zcd.
    fn complete(&mut self) {
        let c = self.contains("c");
        let rv64 = self.rv64();
        if c && self.contains("f") && !rv64 {
            self.0.add_named(&tables::TARGET, "zcf");
        }
        if c && self.contains("d") {
            self.0.add_named(&tables::TARGET, "zcd");
        }
    }

    /// Whether a feature (by name) is on.
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains(&tables::TARGET, name)
    }

    /// Whether the code is RV64's.
    pub fn rv64(&self) -> bool {
        self.contains("64bit")
    }

    fn rve(&self) -> bool {
        self.contains("e")
    }

    fn has(&self, i: usize) -> bool {
        self.0.has(i)
    }
}

/// The instruction at the start of `bytes` (little-endian), with
/// `features` on: 16 bits if its low two bits aren't 11, else 32. `None`
/// if it isn't one (or `bytes` is too short); longer encodings (48 bits
/// and up) aren't decoded.
pub fn decode(bytes: &[u8], features: &Features) -> Option<Instruction> {
    let b0 = *bytes.first()?;
    let (w, size) = if b0 & 3 == 3 {
        (u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?), 4)
    } else {
        (u32::from(u16::from_le_bytes([b0, *bytes.get(1)?])), 2)
    };
    turbonop_tablegen::decode(
        &tables::TARGET,
        u64::from(w),
        size,
        &features.0,
        features.rv64(),
        features.rve(),
    )
}

/// The size of the instruction whose first bytes are `b0` and `b1` (as
/// its length encoding says: 2, 4, 6, 8, or more), if it says.
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

/// A compressed instruction's full form, as LLVM prints it (and as it
/// runs): `c.addi a0, 1` is `addi a0, a0, 1`. `None` for an instruction
/// that isn't compressed (or has no full form).
pub fn uncompress(i: &Instruction, features: &Features) -> Option<Instruction> {
    turbonop_tablegen::uncompress(i, &features.0)
}

#[cfg(test)]
mod tests;

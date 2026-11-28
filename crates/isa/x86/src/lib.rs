//! An x86 and x86-64 instruction decoder.
//!
//! [`decode`] turns bytes into an [`Instruction`]: a compact, `Copy` value
//! holding the mnemonic, up to five [`Operand`]s, and the prefixes and
//! EVEX decorations that matter. Nothing is allocated, and an instruction
//! refers to nothing outside itself, so callers can decode on demand and
//! discard the results.
//!
//! The opcode tables live in `tables/*.txt`, in a compact notation based on
//! the Intel SDM's opcode maps; `tools/isa-gen` compiles them.
//!
//! [`format`] renders instructions in the MASM-style syntax of Microsoft's
//! `dumpbin /disasm`.
//!
//! [`Instruction::accesses`] says what an instruction reads and writes: each
//! operand's [`Rw`], its implicit operands (`rdx` for `mul`, the stack for
//! `push`), and its [`Flags`]; [`Instruction::written_registers`] is the
//! complete set of registers it may write, as a [`RegSet`]. The data comes
//! from `tables/access.rules` and is checked against Intel XED.

#![no_std]
#![warn(missing_docs)]

#[cfg(test)]
use turbonop_global_alloc as _;

extern crate alloc;

mod access;
mod decode;
pub mod format;
mod instruction;
mod operand;
mod register;
#[allow(dead_code)]
mod spec;

#[allow(clippy::all)]
mod tables {
    use crate::spec::{AccessSpec, Entry, Imp, ImpKind, Loc, OpSpec, Size, Tuple, attr};
    include!("generated/tables.rs");

    impl Mnemonic {
        /// The mnemonic's name, in lowercase.
        pub fn name(self) -> &'static str {
            MNEMONIC_NAMES[self as usize]
        }
    }
}

pub use access::{Access, Accesses, FlagAccess, Flags, RegSet, Rw};
pub use decode::{DecodeError, MAX_LENGTH, Mode, decode};
pub use instruction::{Encoding, Flow, Instruction, Rounding, prefix};
pub use operand::{Immediate, MemSize, Memory, Operand};
pub use register::{RegClass, Register};
pub use tables::Mnemonic;

/// The number of opcode table forms (for tests and table tools).
#[doc(hidden)]
pub fn form_count() -> usize {
    tables::ORIGINS.len()
}

/// Where opcode table form `form` came from: `file:line mnemonic operands`
/// (for tests and table tools).
#[doc(hidden)]
pub fn form_origin(form: u16) -> Option<&'static str> {
    tables::ORIGINS.get(usize::from(form)).copied()
}

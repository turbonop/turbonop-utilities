//! Operands, in a form common to every instruction set.

use crate::format::Ref;
use crate::memory::MemoryAccess;
use crate::register::Access;

/// A constant operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Immediate {
    /// Its value, as the instruction uses it (expanded, shifted, sign- or
    /// zero-extended to 64 bits as the instruction does).
    pub value: i64,
    /// How many bits of the encoding hold it (its range, for a rewriter);
    /// 0 if not known.
    pub bits: u8,
    /// Whether the encoding's bits are signed.
    pub signed: bool,
}

/// An explicit operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Operand<R> {
    /// A register, and how it's accessed.
    Register(R, Access),
    /// A constant that isn't an address.
    Immediate(Immediate),
    /// A code address: a branch's or call's target (resolved: the address,
    /// not an offset).
    Target(u64),
    /// A data address, computed from the instruction's address (ARM64's
    /// `adr` and `adrp`, RISC-V's `auipc`, x86's RIP-relative `lea` is a
    /// [`Memory`](Operand::Memory)): moving the instruction moves it.
    Address(u64),
    /// Memory.
    Memory(MemoryAccess<R>),
    /// An entity named by an index or token (a function, a type, a
    /// field, a local): what it is, the caller's tables say.
    Ref(Ref),
    /// Something else (a condition code, a shift's kind, an arrangement,
    /// a system register's encoding): the instruction set's own operand
    /// says.
    Other,
}

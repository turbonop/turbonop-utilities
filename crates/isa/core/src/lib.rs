//! The interface every instruction set's decoder implements, so that tools
//! (sweeps, control-flow graphs, cross-references, fingerprints, listings,
//! and later decompilers, lifters, and rewriters) are written once, over
//! any instruction set, rather than once per instruction set.
//!
//! # Shape
//!
//! [`Isa`] is the core every instruction set implements: decoding, an
//! instruction's length and [`Opcode`], how control leaves it
//! ([`ControlFlow`]), and its [`Attributes`]. A value of an `Isa` is a
//! *configured* decoder (its mode, features, byte order), built once and
//! asked about many instructions. Capabilities beyond the core are
//! separate traits, which an instruction set implements when they mean
//! something for it:
//!
//! - [`Registers`]: the registers each instruction reads and writes,
//!   explicitly and implicitly, as the instruction set's own typed
//!   [`Register`]s.
//! - [`Memory`]: the memory each instruction reads and writes, its
//!   address as base, index, displacement ([`MemoryAccess`]).
//! - [`Operands`]: its operands, in a common typed form ([`Operand`]),
//!   each instruction set's richer form staying its own crate's.
//! - [`Format`]: its text, as a stream of classified tokens
//!   ([`TokenSink`]), names supplied by the caller ([`Resolver`]).
//!
//! A tool asks for the capabilities it needs (`fn xrefs<I: Isa +
//! Memory>`), and works with every instruction set that has them.
//!
//! # Performance
//!
//! Everything here is monomorphized: generic tools are compiled once per
//! instruction set, with no dynamic dispatch per instruction. A caller
//! that picks the instruction set at run time does so once per buffer,
//! not per instruction. Nothing here allocates; per-instruction lists
//! (accesses, operands) are iterators. Opcodes, registers, and features
//! are numbers: no string is compared at run time; strings are made only
//! as text is formatted.
//!
//! # Instructions and addresses
//!
//! An instruction is a small `Copy` value that holds its bytes as they
//! were encoded, and reads its operands from them when asked: decoding
//! and re-encoding it gives the same bytes, and a caller may reinterpret
//! them. It doesn't hold its address: what depends on where it is (a
//! branch's target, a PC-relative address) is asked with the address.
//!
//! # Context
//!
//! Some instruction sets decode differently by what precedes an
//! instruction: WebAssembly's branches name enclosing blocks, whose ends
//! are known only from the whole function; a Thumb IT block makes the
//! instructions after it conditional. [`Isa::Context`] is what a scan of
//! a function's code finds, made once ([`Isa::context`]) and passed to
//! each query. For an instruction set that needs none, it is `()`: free.

#![no_std]
#![warn(missing_docs)]

extern crate alloc;

mod attributes;
mod error;
mod flow;
mod format;
mod memory;
mod operand;
mod register;

pub use attributes::Attributes;
pub use error::DecodeError;
pub use flow::{ControlFlow, Delay, FlowKind, Target};
pub use format::{IndexSpace, Reference, Resolver, TokenKind, TokenSink, Unresolved};
pub use memory::{Base, Extend, Index, MemoryAccess, Offset, Update};
pub use operand::{Immediate, Operand};
pub use register::{Access, BitSet, Bits, OtherBits, Register, RegisterAccess, RegisterSet};

use core::fmt::Debug;
use core::hash::Hash;

/// An instruction set's opcode: which instruction (and which encoding
/// of it) an instruction is. A number (an enum), never a string; its
/// [`mnemonic`](Opcode::mnemonic) is for text alone.
pub trait Opcode: Copy + Eq + Ord + Hash + Debug + Send + Sync + 'static {
    /// The number of opcodes ([`index`](Opcode::index) is less).
    const COUNT: u32;

    /// Its number: dense, from 0, for tables indexed by opcode.
    fn index(self) -> u32;

    /// Its name, as text shows it.
    fn mnemonic(self) -> &'static str;
}

/// An instruction set's decoder, configured (mode, features, byte order):
/// the core every instruction set implements.
pub trait Isa: Clone + Send + Sync + 'static {
    /// A decoded instruction: a small `Copy` value holding its bytes.
    type Instruction: Copy + Eq + Hash + Debug + Send + Sync + 'static;

    /// Which instruction an instruction is.
    type Opcode: Opcode;

    /// What a scan of a function finds that decoding needs (see the
    /// crate's documentation); `()` for most instruction sets.
    type Context: Clone + Debug + Default + Send + Sync;

    /// The fewest bytes an instruction has.
    const MIN_LENGTH: usize;

    /// The most bytes an instruction has.
    const MAX_LENGTH: usize;

    /// The alignment instructions start at: where to try next when bytes
    /// don't decode.
    const ALIGNMENT: usize;

    /// The context of the function whose code is `code`, at `address`:
    /// for an instruction set whose `Context` is `()`, nothing (and no
    /// work). A best effort: what doesn't decode is passed over.
    fn context(&self, code: &[u8], address: u64) -> Self::Context;

    /// The instruction at the start of `bytes` (more may follow), which is
    /// at `address`.
    ///
    /// # Errors
    ///
    /// [`DecodeError`]: `bytes` ends before the instruction does, or no
    /// instruction is encoded so (or one is, but not in this
    /// configuration).
    fn decode(
        &self,
        cx: &Self::Context,
        bytes: &[u8],
        address: u64,
    ) -> Result<Self::Instruction, DecodeError>;

    /// The instruction's bytes, as encoded.
    fn bytes(i: &Self::Instruction) -> &[u8];

    /// Its length in bytes.
    #[inline]
    fn len(i: &Self::Instruction) -> usize {
        Self::bytes(i).len()
    }

    /// Which instruction it is.
    fn opcode(i: &Self::Instruction) -> Self::Opcode;

    /// How control leaves it, at `address`.
    fn flow(&self, cx: &Self::Context, i: &Self::Instruction, address: u64) -> ControlFlow;

    /// The targets of a multi-way jump ([`Target::Table`]: a `switch`, a
    /// `br_table`), each given to `f`; nothing for any other instruction.
    #[inline]
    fn table_targets(
        &self,
        _cx: &Self::Context,
        _i: &Self::Instruction,
        _address: u64,
        _f: &mut dyn FnMut(u64),
    ) {
    }

    /// What it does beyond its registers, memory, and control flow.
    fn attributes(&self, i: &Self::Instruction) -> Attributes;
}

/// The registers instructions read and write.
pub trait Registers: Isa {
    /// A register, as the instruction set names it.
    type Register: Register;

    /// A set of registers, by storage: [`BitSet`] of enough words.
    type RegisterSet: RegisterSet<Self::Register>;

    /// Every register `i` (at `address`) reads or writes, explicit
    /// operands and implicit ones (flags, the stack pointer, a call's
    /// link register), each once.
    fn register_accesses(
        &self,
        cx: &Self::Context,
        i: &Self::Instruction,
        address: u64,
    ) -> impl Iterator<Item = RegisterAccess<Self::Register>>;

    /// The registers `i` may read, and those it may write.
    #[inline]
    fn register_sets(
        &self,
        cx: &Self::Context,
        i: &Self::Instruction,
        address: u64,
    ) -> (Self::RegisterSet, Self::RegisterSet) {
        let mut reads = Self::RegisterSet::default();
        let mut writes = Self::RegisterSet::default();
        for a in self.register_accesses(cx, i, address) {
            if a.access.may_read() {
                reads.insert(a.register);
            }
            if a.access.may_write() {
                writes.insert(a.register);
            }
        }
        (reads, writes)
    }
}

/// The memory instructions read and write.
pub trait Memory: Registers {
    /// Every access to memory `i` (at `address`) makes: its operands' and
    /// its implicit ones (a push's, a call's, a string instruction's).
    fn memory_accesses(
        &self,
        cx: &Self::Context,
        i: &Self::Instruction,
        address: u64,
    ) -> impl Iterator<Item = MemoryAccess<Self::Register>>;
}

/// Instructions' operands, in a form common to every instruction set.
pub trait Operands: Registers {
    /// `i`'s (at `address`) explicit operands, in the order the
    /// instruction set's manual gives them.
    fn operands(
        &self,
        cx: &Self::Context,
        i: &Self::Instruction,
        address: u64,
    ) -> impl Iterator<Item = Operand<Self::Register>>;
}

/// Instructions as text.
pub trait Format: Isa {
    /// The syntaxes the text can be in (`Default`: the one most used, or
    /// most like the platform's own tools').
    type Syntax: Copy + Eq + Debug + Default + Send + Sync;

    /// Writes `i` (at `address`) to `out` in `syntax`: its mnemonic, then
    /// its operands; names (of addresses, of indices) from `names`.
    fn format<S: TokenSink + ?Sized, N: Resolver + ?Sized>(
        &self,
        cx: &Self::Context,
        i: &Self::Instruction,
        address: u64,
        syntax: Self::Syntax,
        names: &N,
        out: &mut S,
    );
}

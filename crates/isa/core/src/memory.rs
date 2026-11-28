//! Instructions' accesses of memory: where (base, index, displacement)
//! and how much.

use crate::register::Access;

/// What an address is computed from, besides its index and displacement.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Base<R> {
    /// Nothing: the address is the displacement (an absolute address).
    None,
    /// A register.
    Register(R),
    /// The instruction's address (by the instruction set's rule: its own,
    /// the next instruction's, ARM's PC): the displacement is resolved,
    /// the address the access is of. Moving the instruction moves it.
    Pc,
    /// A value on the operand stack (a stack machine's), this far from
    /// its top (0: the top).
    Stack(u8),
}

/// How an index register's value is widened before it is scaled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Extend {
    /// Used whole.
    #[default]
    None,
    /// Its low this many bits, zero-extended (ARM64's `uxtw`).
    Zero(u8),
    /// Its low this many bits, sign-extended (`sxtw`).
    Sign(u8),
}

/// An address's index: a register, widened, then shifted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Index<R> {
    /// The register.
    pub register: R,
    /// How it's widened.
    pub extend: Extend,
    /// How far it's shifted left (scale 2<sup>shift</sup>).
    pub shift: u8,
}

/// What a base register is updated by, besides the access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Offset<R> {
    /// A constant.
    Immediate(i64),
    /// A register's value.
    Register(R),
}

/// How an access updates its base register (write-back).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Update<R> {
    /// It doesn't.
    None,
    /// To the address, before the access (pre-indexed: `[x1, #8]!`).
    Pre,
    /// By the offset, after the access (post-indexed: `[x1], #8`).
    Post(Offset<R>),
}

/// An instruction's access of memory.
///
/// The address is `base + (index extended << shift) + displacement`
/// (for a [`Base::Pc`] access, `displacement` alone).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MemoryAccess<R> {
    /// How it's accessed (read, written, both, or neither: `lea`, a
    /// prefetch).
    pub access: Access,
    /// The base.
    pub base: Base<R>,
    /// The index, if there is one.
    pub index: Option<Index<R>>,
    /// The displacement (the address itself, for [`Base::None`] and
    /// [`Base::Pc`]).
    pub displacement: i64,
    /// A segment (x86's `fs`, `gs`), if one is named or implied.
    pub segment: Option<R>,
    /// How its base is updated.
    pub update: Update<R>,
    /// How many bytes, if a constant number (not: `rep movs`, a gather,
    /// `xsave`).
    pub size: Option<u32>,
    /// Which memory (WebAssembly's memory index, an address space); 0 if
    /// the instruction set has one.
    pub space: u32,
    /// Whether no operand names it (a push's, a call's, `movs`'s).
    pub implicit: bool,
}

impl<R> MemoryAccess<R> {
    /// An explicit access of `size` bytes at `base + displacement`.
    #[inline]
    pub const fn new(access: Access, base: Base<R>, displacement: i64, size: Option<u32>) -> Self {
        MemoryAccess {
            access,
            base,
            index: None,
            displacement,
            segment: None,
            update: Update::None,
            size,
            space: 0,
            implicit: false,
        }
    }

    /// The address, if it's a constant (absolute, or PC-relative), and
    /// no segment is added to it.
    #[inline]
    #[allow(clippy::cast_sign_loss)] // An address's bits.
    pub fn address(&self) -> Option<u64> {
        match (&self.base, &self.index, &self.segment) {
            (Base::None | Base::Pc, None, None) => Some(self.displacement as u64),
            _ => None,
        }
    }
}

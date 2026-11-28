//! What an instruction does beyond its registers, memory, and control
//! flow.

use core::ops::{BitOr, BitOrAssign};

/// What an instruction does or is beyond its registers, memory, and
/// control flow: what an optimizer, a lifter, or a rewriter must not
/// reorder, remove, or assume.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Attributes(u16);

impl Attributes {
    /// None.
    pub const NONE: Attributes = Attributes(0);
    /// It changes or depends on state beyond its registers and memory
    /// (system registers, caches, the TLB, hints): it can't be reordered
    /// or removed.
    pub const SYSTEM: Attributes = Attributes(1 << 0);
    /// It orders memory accesses (a fence, a barrier, acquire or release
    /// semantics).
    pub const BARRIER: Attributes = Attributes(1 << 1);
    /// Its memory access is atomic (a locked or exclusive access, an
    /// atomic read-modify-write).
    pub const ATOMIC: Attributes = Attributes(1 << 2);
    /// It runs only at a privileged level.
    pub const PRIVILEGED: Attributes = Attributes(1 << 3);
    /// It may raise a floating-point exception (or set its sticky flags).
    pub const FP_EXCEPTION: Attributes = Attributes(1 << 4);
    /// The architecture calls its encoding UNPREDICTABLE (or its
    /// should-be bits don't hold): a processor may do other than its
    /// text says.
    pub const UNPREDICTABLE: Attributes = Attributes(1 << 5);
    /// A no-op (a hint that does nothing, padding).
    pub const NOP: Attributes = Attributes(1 << 6);
    /// Its effects aren't all known here: what [`Registers`](crate::Registers)
    /// and [`Memory`](crate::Memory) report of it is incomplete.
    pub const UNMODELED: Attributes = Attributes(1 << 7);

    /// Whether it has every attribute of `other`.
    #[inline]
    pub const fn contains(self, other: Attributes) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether it has any attribute of `other`.
    #[inline]
    pub const fn intersects(self, other: Attributes) -> bool {
        self.0 & other.0 != 0
    }

    /// Whether it has none.
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Its bits.
    #[inline]
    pub const fn bits(self) -> u16 {
        self.0
    }
}

impl BitOr for Attributes {
    type Output = Attributes;

    #[inline]
    fn bitor(self, rhs: Attributes) -> Attributes {
        Attributes(self.0 | rhs.0)
    }
}

impl BitOrAssign for Attributes {
    #[inline]
    fn bitor_assign(&mut self, rhs: Attributes) {
        self.0 |= rhs.0;
    }
}

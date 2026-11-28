//! Registers, sets of them, and how instructions access them.

use core::fmt::Debug;
use core::hash::Hash;
use core::marker::PhantomData;

/// Which bits of its storage a register is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Bits {
    /// The first (least significant) bit.
    pub offset: u32,
    /// How many.
    pub width: u32,
}

impl Bits {
    /// `width` bits from bit 0.
    #[inline]
    pub const fn low(width: u32) -> Bits {
        Bits { offset: 0, width }
    }

    /// Whether these and `other` share a bit.
    #[inline]
    pub const fn overlaps(self, other: Bits) -> bool {
        self.offset < other.offset + other.width && other.offset < self.offset + self.width
    }
}

/// A register, as an instruction set names it.
///
/// Registers alias: `eax` is the low 32 bits of `rax`, `w0` of `x0`,
/// `s0` of `v0`. What holds a register's bits is its *storage* (`rax`'s,
/// `x0`'s, `v0`'s), numbered from 0 for each instruction set; a register
/// is some bits of one. Flags are registers too (x86's each flag,
/// ARM's NZCV): an instruction's effects on them are accesses like any
/// other.
pub trait Register: Copy + Eq + Hash + Debug + Send + Sync + 'static {
    /// How many storages the instruction set has: each
    /// [`storage`](Register::storage) is less.
    const STORAGE_COUNT: u16;

    /// The storage it is part of; `None` for a register that has none (a
    /// zero register, whose writes are discarded).
    fn storage(self) -> Option<u16>;

    /// Which bits of its storage it is.
    fn bits(self) -> Bits;

    /// The register that is all of storage `s` (`rax`, `x0`, `z0`), if
    /// there is one.
    fn from_storage(s: u16) -> Option<Self>;
}

/// How an instruction accesses a register or memory: read, written,
/// both, or either only if a condition holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Access(u8);

impl Access {
    /// Neither (a memory operand whose address alone is used: `lea`).
    pub const NONE: Access = Access(0);
    /// Read.
    pub const READ: Access = Access(1 << 0);
    /// Written.
    pub const WRITE: Access = Access(1 << 1);
    /// Read and written.
    pub const READ_WRITE: Access = Access(1 << 0 | 1 << 1);
    /// Read only if a condition holds.
    pub const MAY_READ: Access = Access(1 << 2);
    /// Written only if a condition holds (`cmov`, a predicated store).
    pub const MAY_WRITE: Access = Access(1 << 3);
    /// Read, and written only if a condition holds.
    pub const READ_MAY_WRITE: Access = Access(1 << 0 | 1 << 3);

    /// Whether it is always read.
    #[inline]
    pub const fn reads(self) -> bool {
        self.0 & Access::READ.0 != 0
    }

    /// Whether it is always written.
    #[inline]
    pub const fn writes(self) -> bool {
        self.0 & Access::WRITE.0 != 0
    }

    /// Whether it may be read (always, or if a condition holds).
    #[inline]
    pub const fn may_read(self) -> bool {
        self.0 & (Access::READ.0 | Access::MAY_READ.0) != 0
    }

    /// Whether it may be written.
    #[inline]
    pub const fn may_write(self) -> bool {
        self.0 & (Access::WRITE.0 | Access::MAY_WRITE.0) != 0
    }

    /// Both accesses: each of either's.
    #[inline]
    #[must_use]
    pub const fn union(self, other: Access) -> Access {
        Access(self.0 | other.0)
    }
}

/// What a write of a register does to the rest of its storage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OtherBits {
    /// Keeps it (`ax`'s write of `rax`, legacy SSE's of a `ymm`).
    #[default]
    Preserved,
    /// Zeroes it (`eax`'s write of `rax`, `w0`'s of `x0`, VEX's of a
    /// `zmm`).
    Zeroed,
    /// Fills it with the written bits' top bit (RV64's and MIPS64's
    /// 32-bit operations).
    SignExtended,
    /// Leaves it undefined.
    Undefined,
}

/// An instruction's access of a register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegisterAccess<R> {
    /// The register.
    pub register: R,
    /// How it's accessed.
    pub access: Access,
    /// What a write does to the rest of its storage.
    pub other_bits: OtherBits,
    /// Whether no operand names it (flags, the stack pointer of a push,
    /// a call's link register).
    pub implicit: bool,
}

/// A set of registers, by storage: a register is in it if its storage
/// is (`eax` is in a set that has `rax`).
pub trait RegisterSet<R: Register>: Copy + Default + Eq + Hash + Debug + Send + Sync {
    /// Whether `r`'s storage is in it (never, for a register that has
    /// none).
    fn contains(&self, r: R) -> bool;

    /// Adds `r`'s storage (nothing, for a register that has none).
    fn insert(&mut self, r: R);

    /// Removes `r`'s storage.
    fn remove(&mut self, r: R);

    /// Whether it's empty.
    fn is_empty(&self) -> bool;

    /// How many storages it has.
    fn len(&self) -> usize;

    /// Each storage in either.
    #[must_use]
    fn union(&self, other: &Self) -> Self;

    /// Each storage in both.
    #[must_use]
    fn intersection(&self, other: &Self) -> Self;

    /// Each storage in this and not `other`.
    #[must_use]
    fn difference(&self, other: &Self) -> Self;

    /// Its storages, in order.
    fn storages(&self) -> impl Iterator<Item = u16>;

    /// Its storages as the registers that are all of them, in order.
    fn registers(&self) -> impl Iterator<Item = R> {
        self.storages().filter_map(R::from_storage)
    }
}

/// A set of `R`'s storages as `W` 64-bit words of bits: fixed size, no
/// allocation. An instruction set picks the fewest words that hold its
/// [`STORAGE_COUNT`](Register::STORAGE_COUNT) (checked as it compiles).
/// A set is of one instruction set's registers: one of x86's isn't one of
/// ARM64's.
pub struct BitSet<R, const W: usize> {
    words: [u64; W],
    of: PhantomData<fn() -> R>,
}

impl<R, const W: usize> Clone for BitSet<R, W> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<R, const W: usize> Copy for BitSet<R, W> {}

impl<R, const W: usize> PartialEq for BitSet<R, W> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.words == other.words
    }
}

impl<R, const W: usize> Eq for BitSet<R, W> {}

impl<R, const W: usize> Hash for BitSet<R, W> {
    #[inline]
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.words.hash(state);
    }
}

impl<R, const W: usize> Default for BitSet<R, W> {
    #[inline]
    fn default() -> Self {
        Self::EMPTY
    }
}

impl<R, const W: usize> Debug for BitSet<R, W> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_set().entries(self.storages()).finish()
    }
}

impl<R, const W: usize> BitSet<R, W> {
    /// No storages.
    pub const EMPTY: Self = BitSet {
        words: [0; W],
        of: PhantomData,
    };

    /// Whether it has storage `s`.
    #[inline]
    pub const fn contains_storage(&self, s: u16) -> bool {
        let s = s as usize;
        s < 64 * W && self.words[s / 64] >> (s % 64) & 1 != 0
    }

    /// Adds storage `s`.
    ///
    /// # Panics
    ///
    /// If `s` is out of range (it isn't, from a register: that's checked
    /// as the set's [`insert`](RegisterSet::insert) compiles).
    #[inline]
    pub fn insert_storage(&mut self, s: u16) {
        let s = s as usize;
        self.words[s / 64] |= 1 << (s % 64);
    }

    /// Removes storage `s`.
    #[inline]
    pub fn remove_storage(&mut self, s: u16) {
        let s = s as usize;
        if s < 64 * W {
            self.words[s / 64] &= !(1 << (s % 64));
        }
    }

    /// Whether it's empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|&w| w == 0)
    }

    /// How many storages it has.
    #[inline]
    pub fn len(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Each storage in either.
    #[inline]
    #[must_use]
    pub fn union(&self, other: &Self) -> Self {
        self.zip(other, |a, b| a | b)
    }

    /// Each storage in both.
    #[inline]
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        self.zip(other, |a, b| a & b)
    }

    /// Each storage in this and not `other`.
    #[inline]
    #[must_use]
    pub fn difference(&self, other: &Self) -> Self {
        self.zip(other, |a, b| a & !b)
    }

    /// Its storages, in order.
    pub fn storages(&self) -> impl Iterator<Item = u16> + use<R, W> {
        let words = self.words;
        (0..W).flat_map(move |k| {
            let mut w = words[k];
            core::iter::from_fn(move || {
                if w == 0 {
                    return None;
                }
                let bit = w.trailing_zeros();
                w &= w - 1;
                #[allow(clippy::cast_possible_truncation)] // < 64 * W, checked
                Some((k * 64) as u16 + bit as u16)
            })
        })
    }

    /// Its words.
    #[inline]
    pub const fn words(&self) -> &[u64; W] {
        &self.words
    }

    /// The set whose storages are the bits of `words`.
    #[inline]
    pub const fn from_words(words: [u64; W]) -> Self {
        BitSet {
            words,
            of: PhantomData,
        }
    }

    #[inline]
    fn zip(&self, other: &Self, f: impl Fn(u64, u64) -> u64) -> Self {
        let mut words = [0; W];
        for (k, w) in words.iter_mut().enumerate() {
            *w = f(self.words[k], other.words[k]);
        }
        BitSet {
            words,
            of: PhantomData,
        }
    }
}

/// [`RegisterSet`]'s methods, without it in scope.
impl<R: Register, const W: usize> BitSet<R, W> {
    /// Whether `r`'s storage is in it (never, for a register that has
    /// none).
    #[inline]
    pub fn contains(&self, r: R) -> bool {
        r.storage().is_some_and(|s| self.contains_storage(s))
    }

    /// Adds `r`'s storage (nothing, for a register that has none).
    #[inline]
    pub fn insert(&mut self, r: R) {
        const { assert!(R::STORAGE_COUNT as usize <= 64 * W, "too few words") };
        if let Some(s) = r.storage() {
            self.insert_storage(s);
        }
    }

    /// Removes `r`'s storage.
    #[inline]
    pub fn remove(&mut self, r: R) {
        if let Some(s) = r.storage() {
            self.remove_storage(s);
        }
    }

    /// Its storages as the registers that are all of them, in order.
    pub fn registers(&self) -> impl Iterator<Item = R> + use<R, W> {
        self.storages().filter_map(R::from_storage)
    }
}

impl<R: Register, const W: usize> FromIterator<R> for BitSet<R, W> {
    fn from_iter<I: IntoIterator<Item = R>>(iter: I) -> Self {
        let mut s = Self::EMPTY;
        for r in iter {
            s.insert(r);
        }
        s
    }
}

impl<R: Register, const W: usize> RegisterSet<R> for BitSet<R, W> {
    #[inline]
    fn contains(&self, r: R) -> bool {
        BitSet::contains(self, r)
    }

    #[inline]
    fn insert(&mut self, r: R) {
        BitSet::insert(self, r);
    }

    #[inline]
    fn remove(&mut self, r: R) {
        BitSet::remove(self, r);
    }

    #[inline]
    fn is_empty(&self) -> bool {
        BitSet::is_empty(self)
    }

    #[inline]
    fn len(&self) -> usize {
        BitSet::len(self)
    }

    #[inline]
    fn union(&self, other: &Self) -> Self {
        BitSet::union(self, other)
    }

    #[inline]
    fn intersection(&self, other: &Self) -> Self {
        BitSet::intersection(self, other)
    }

    #[inline]
    fn difference(&self, other: &Self) -> Self {
        BitSet::difference(self, other)
    }

    fn storages(&self) -> impl Iterator<Item = u16> {
        BitSet::storages(self)
    }
}

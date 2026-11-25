//! What an instruction reads and writes: its operands' accesses, its
//! implicit operands, and its flags.
//!
//! The data is per opcode table form, from `tables/access.rules`, and is
//! looked up only when asked for: decoding doesn't pay for it.
//!
//! It is complete, which is the point: every register, flag, and memory
//! operand an instruction may write is reported as written. A conditional
//! write (`cmov`, `cmpxchg`'s destination) is a write, and so is a flag left
//! undefined. Where the data is unsure, it over-approximates (an extra
//! write costs precision; a missing one gives wrong answers).
//!
//! What it doesn't track: the instruction pointer (every instruction
//! writes it; see [`Instruction::flow`]), the x87 instruction and data
//! pointers and opcode, MXCSR's sticky exception flags, the hidden parts
//! of segment registers apart from the segment registers themselves (a
//! write of the FS base is a write of `fs`), and the CET shadow stack's
//! memory (SSP itself is tracked). A system call's results are modeled as
//! writes of every general-purpose register, since the kernel's convention
//! isn't the instruction's.
//!
//! Memory operands say where an access starts and, where it is known, its
//! size: a `rep` string instruction's operand is one element of the range
//! it covers, and `enter`'s pushes (as many as its nesting level asks for)
//! have no size.

use crate::instruction::Instruction;
use crate::spec::{AccessSpec, Imp, ImpKind};
use crate::tables::{ACCESS, FLAG_SETS, IMPLICIT};
use crate::{MemSize, Memory, Mode, Operand, RegClass, Register};
use core::fmt;

/// The encoded accesses of `AccessSpec::ops` and `Imp::rw`.
pub(crate) mod rw {
    pub const READ: u8 = 1;
    pub const WRITE: u8 = 2;
    pub const READ_WRITE: u8 = 3;
    pub const COND_WRITE: u8 = 4;
    pub const READ_COND_WRITE: u8 = 5;
    /// Only with a `rep` prefix.
    pub const REP: u8 = 0x80;
}

/// How an operand is accessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rw {
    /// Neither read nor written: a memory operand whose address is all
    /// that is used (`lea`; its address registers are still read), a
    /// multi-byte NOP's operand, or an operand past the last.
    None,
    /// Read (immediates and branch targets too).
    Read,
    /// Written.
    Write,
    /// Read and written.
    ReadWrite,
    /// Written or not, depending on a condition (`cmov`): may-write.
    CondWrite,
    /// Read, and written or not.
    ReadCondWrite,
}

impl Rw {
    const fn from_code(code: u8) -> Self {
        match code & 7 {
            rw::READ => Rw::Read,
            rw::WRITE => Rw::Write,
            rw::READ_WRITE => Rw::ReadWrite,
            rw::COND_WRITE => Rw::CondWrite,
            rw::READ_COND_WRITE => Rw::ReadCondWrite,
            _ => Rw::None,
        }
    }

    /// Whether it reads.
    #[inline]
    pub const fn reads(self) -> bool {
        matches!(self, Rw::Read | Rw::ReadWrite | Rw::ReadCondWrite)
    }

    /// Whether it may write (conditional writes included).
    #[inline]
    pub const fn writes(self) -> bool {
        matches!(
            self,
            Rw::Write | Rw::ReadWrite | Rw::CondWrite | Rw::ReadCondWrite
        )
    }

    /// Whether a write is conditional.
    #[inline]
    pub const fn is_conditional(self) -> bool {
        matches!(self, Rw::CondWrite | Rw::ReadCondWrite)
    }
}

/// A set of RFLAGS bits, at their positions in RFLAGS.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Flags(pub u32);

impl Flags {
    /// Carry.
    pub const CF: Self = Self(1 << 0);
    /// Parity.
    pub const PF: Self = Self(1 << 2);
    /// Auxiliary carry.
    pub const AF: Self = Self(1 << 4);
    /// Zero.
    pub const ZF: Self = Self(1 << 6);
    /// Sign.
    pub const SF: Self = Self(1 << 7);
    /// Trap.
    pub const TF: Self = Self(1 << 8);
    /// Interrupt enable.
    pub const IF: Self = Self(1 << 9);
    /// Direction.
    pub const DF: Self = Self(1 << 10);
    /// Overflow.
    pub const OF: Self = Self(1 << 11);
    /// I/O privilege level (two bits).
    pub const IOPL: Self = Self(3 << 12);
    /// Nested task.
    pub const NT: Self = Self(1 << 14);
    /// Resume.
    pub const RF: Self = Self(1 << 16);
    /// Virtual-8086 mode.
    pub const VM: Self = Self(1 << 17);
    /// Alignment check (and SMAP's access control).
    pub const AC: Self = Self(1 << 18);
    /// Virtual interrupt.
    pub const VIF: Self = Self(1 << 19);
    /// Virtual interrupt pending.
    pub const VIP: Self = Self(1 << 20);
    /// CPUID available.
    pub const ID: Self = Self(1 << 21);
    /// The arithmetic status flags: OF, SF, ZF, AF, PF, and CF.
    pub const STATUS: Self = Self(0x8D5);
    /// No flags.
    pub const NONE: Self = Self(0);

    /// Whether it has no flags.
    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Whether it has all of `other`.
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether it has any of `other`.
    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl core::ops::BitOr for Flags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitAnd for Flags {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl fmt::Debug for Flags {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [(u32, &str); 17] = [
            (1 << 0, "cf"),
            (1 << 2, "pf"),
            (1 << 4, "af"),
            (1 << 6, "zf"),
            (1 << 7, "sf"),
            (1 << 8, "tf"),
            (1 << 9, "if"),
            (1 << 10, "df"),
            (1 << 11, "of"),
            (3 << 12, "iopl"),
            (1 << 14, "nt"),
            (1 << 16, "rf"),
            (1 << 17, "vm"),
            (1 << 18, "ac"),
            (1 << 19, "vif"),
            (1 << 20, "vip"),
            (1 << 21, "id"),
        ];
        let mut first = true;
        f.write_str("{")?;
        for (bit, name) in NAMES {
            if self.0 & bit != 0 {
                if !first {
                    f.write_str(" ")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        f.write_str("}")
    }
}

/// The flags an instruction reads and writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct FlagAccess {
    /// Read.
    pub read: Flags,
    /// Written, or left undefined: may-write.
    pub written: Flags,
    /// Left undefined (also in `written`).
    pub undefined: Flags,
}

/// A set of full registers ([`Register::full`]): `rax` stands for `al`,
/// `ah`, `ax`, `eax`, and `rax`; `zmm0` for `xmm0`, `ymm0`, and `zmm0`; and
/// `st(0)`..`st(7)` are the x87 registers relative to the top of the stack.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RegSet {
    bits: [u64; 4],
}

/// Where each class's registers start in a `RegSet` (and how many there
/// are); `Ip` has one, `rip`.
const SLOTS: [(u16, u16); 19] = [
    (0, 0),    // Gpr8: as Gpr64
    (0, 0),    // Gpr8High
    (0, 0),    // Gpr16
    (0, 0),    // Gpr32
    (0, 32),   // Gpr64
    (32, 8),   // Segment
    (40, 16),  // Control
    (56, 16),  // Debug
    (72, 8),   // St
    (80, 8),   // Mmx
    (0, 0),    // Xmm: as Zmm
    (0, 0),    // Ymm
    (88, 32),  // Zmm
    (120, 8),  // Mask
    (128, 4),  // Bound
    (132, 8),  // Tmm
    (140, 1),  // Ip
    (141, 8),  // Test
    (149, 14), // System
];
const SLOT_COUNT: u16 = 163;

impl RegSet {
    /// The empty set.
    pub const EMPTY: Self = Self { bits: [0; 4] };

    fn slot(r: Register) -> Option<u16> {
        let r = r.full();
        let (start, count) = SLOTS[r.class() as usize];
        let i = u16::from(r.index());
        (i < count).then_some(start + i)
    }

    fn register(slot: u16) -> Register {
        let mut found = Register::RAX;
        for (c, &(start, count)) in (0u8..).zip(SLOTS.iter()) {
            if count != 0 && (start..start + count).contains(&slot) {
                let index = u8::try_from(slot - start).unwrap_or(0);
                found = Register::new(RegClass::from_u8(c), index);
            }
        }
        found
    }

    /// Adds `r`'s full register.
    #[inline]
    pub fn insert(&mut self, r: Register) {
        if let Some(s) = Self::slot(r) {
            self.bits[usize::from(s / 64)] |= 1 << (s % 64);
        }
    }

    /// Whether it has `r`'s full register.
    #[inline]
    pub fn contains(&self, r: Register) -> bool {
        Self::slot(r).is_some_and(|s| self.bits[usize::from(s / 64)] & 1 << (s % 64) != 0)
    }

    /// Whether it is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.bits == [0; 4]
    }

    /// The number of registers.
    pub fn len(&self) -> usize {
        self.bits.iter().map(|b| b.count_ones() as usize).sum()
    }

    /// The registers in both.
    #[inline]
    pub fn intersection(&self, other: &Self) -> Self {
        let mut bits = self.bits;
        for (b, o) in bits.iter_mut().zip(other.bits) {
            *b &= o;
        }
        Self { bits }
    }

    /// The registers in either.
    #[inline]
    pub fn union(&self, other: &Self) -> Self {
        let mut bits = self.bits;
        for (b, o) in bits.iter_mut().zip(other.bits) {
            *b |= o;
        }
        Self { bits }
    }

    /// Its registers (full registers), in class order.
    pub fn iter(&self) -> impl Iterator<Item = Register> + '_ {
        (0..SLOT_COUNT)
            .filter(|&s| self.bits[usize::from(s / 64)] & 1 << (s % 64) != 0)
            .map(Self::register)
    }

    fn any_of(&self, class: RegClass) -> bool {
        let (start, count) = SLOTS[class as usize];
        (start..start + count).any(|s| self.bits[usize::from(s / 64)] & 1 << (s % 64) != 0)
    }

    fn insert_all(&mut self, class: RegClass) {
        let (start, count) = SLOTS[class as usize];
        for s in start..start + count {
            self.bits[usize::from(s / 64)] |= 1 << (s % 64);
        }
    }
}

impl fmt::Debug for RegSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// One thing an instruction accesses: an operand (explicit or implicit), a
/// register or memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    /// The register or memory (for an implicit one, as an operand would
    /// show it: a push writes `[rsp-8]`).
    pub operand: Operand,
    /// How.
    pub rw: Rw,
    /// Whether it is implicit rather than one of [`Instruction::operands`].
    pub implicit: bool,
}

/// The accesses of an instruction: its operands, then its implicit
/// operands. See [`Instruction::accesses`].
#[derive(Clone)]
pub struct Accesses<'a> {
    inst: &'a Instruction,
    implicit: &'static [Imp],
    /// Explicit operands, then implicit ones, then the opmask.
    next: usize,
    /// Within a group.
    sub: u8,
    mask_done: bool,
}

impl Instruction {
    #[inline]
    fn access_spec(&self) -> AccessSpec {
        ACCESS
            .get(usize::from(self.form))
            .copied()
            .unwrap_or(AccessSpec {
                ops: 0,
                implicit: 0,
                implicit_len: 0,
                flags: 0,
            })
    }

    /// How operand `index` is accessed ([`Rw::None`] past the last).
    pub fn operand_access(&self, index: usize) -> Rw {
        if index >= self.operands().len() {
            return Rw::None;
        }
        let code = u8::try_from(self.access_spec().ops >> (3 * index) & 7).unwrap_or(0);
        let rw = Rw::from_code(code);
        // EVEX merge-masking keeps the masked-off elements: a vector
        // destination is read too. (A mask destination's masked-off bits
        // are zeroed.)
        let vector =
            |r: Register| matches!(r.class(), RegClass::Xmm | RegClass::Ymm | RegClass::Zmm);
        if index == 0
            && rw == Rw::Write
            && self.mask != 0
            && !self.zeroing
            && matches!(self.ops[0], Operand::Reg(r) if vector(r))
        {
            return Rw::ReadWrite;
        }
        rw
    }

    /// Everything it reads and writes, register or memory: its operands
    /// (with [`Instruction::operand_access`]), then its implicit operands
    /// (`rdx` for `mul`, the stack for `push`, the EVEX opmask).
    pub fn accesses(&self) -> Accesses<'_> {
        let spec = self.access_spec();
        let start = usize::from(spec.implicit);
        let implicit = IMPLICIT
            .get(start..start + usize::from(spec.implicit_len))
            .unwrap_or(&[]);
        Accesses {
            inst: self,
            implicit,
            next: 0,
            sub: 0,
            mask_done: implicit.iter().any(|i| i.kind == ImpKind::MaskField),
        }
    }

    /// The flags it reads, writes, and leaves undefined.
    pub fn flags(&self) -> FlagAccess {
        let [read, written, undefined] = FLAG_SETS
            .get(usize::from(self.access_spec().flags))
            .copied()
            .unwrap_or([0; 3]);
        if self.no_flags() {
            // APX {nf}: the flags are left alone (what the instruction
            // reads it still reads).
            return FlagAccess {
                read: Flags(read),
                written: Flags::NONE,
                undefined: Flags::NONE,
            };
        }
        FlagAccess {
            read: Flags(read),
            written: Flags(written),
            undefined: Flags(undefined),
        }
    }

    /// The full registers it reads: those of its read operands, and those
    /// that form its memory operands' addresses.
    pub fn read_registers(&self) -> RegSet {
        let mut set = RegSet::EMPTY;
        for a in self.accesses() {
            match a.operand {
                Operand::Reg(r) if a.rw.reads() => set.insert(r),
                Operand::Mem(m) => {
                    for r in [m.segment, m.base, m.index].into_iter().flatten() {
                        set.insert(r);
                    }
                }
                _ => {}
            }
        }
        set
    }

    /// The full registers it may write. A write of part of a register is a
    /// write of the register (the rest of `rax` is kept by a write of `al`,
    /// and zeroed by one of `eax` in 64-bit mode), and the x87 and MMX
    /// registers alias: a write of `st(i)` is a write of some `mm` register,
    /// and a write of an `mm` register changes every `st(i)`.
    pub fn written_registers(&self) -> RegSet {
        let mut set = RegSet::EMPTY;
        for a in self.accesses() {
            if let Operand::Reg(r) = a.operand
                && a.rw.writes()
            {
                set.insert(r);
            }
        }
        let (st, mm) = (set.any_of(RegClass::St), set.any_of(RegClass::Mmx));
        if st {
            set.insert_all(RegClass::Mmx);
        }
        if mm {
            set.insert_all(RegClass::St);
        }
        set
    }

    /// Whether it may read memory.
    pub fn reads_memory(&self) -> bool {
        self.accesses()
            .any(|a| matches!(a.operand, Operand::Mem(_)) && a.rw.reads())
    }

    /// Whether it may write memory.
    pub fn writes_memory(&self) -> bool {
        self.accesses()
            .any(|a| matches!(a.operand, Operand::Mem(_)) && a.rw.writes())
    }

    /// The stack size in bytes: the mode's.
    fn stack_size(&self) -> u8 {
        match self.mode {
            Mode::Bits64 => 8,
            Mode::Bits32 => 4,
            Mode::Bits16 => 2,
        }
    }

    fn has_rep(&self) -> bool {
        self.prefixes & (crate::prefix::REP | crate::prefix::REPE | crate::prefix::REPNE) != 0
    }
}

impl Accesses<'_> {
    /// The memory at `base`, `disp`, of `bytes` bytes (0: unknown).
    fn memory(&self, base: Register, disp: i64, bytes: u16, segment: Option<Register>) -> Operand {
        let inst = self.inst;
        Operand::Mem(Memory {
            segment,
            base: Some(base),
            index: None,
            scale: 1,
            disp,
            disp_size: 0,
            size: MemSize::from_bytes(bytes),
            address_size: inst.address_size(),
            broadcast: 0,
            moffs: false,
        })
    }

    /// The segment override, if any, of the instruction's memory operand.
    fn segment(&self) -> Option<Register> {
        self.inst.operands().iter().find_map(|o| match o {
            Operand::Mem(m) => m.segment,
            _ => None,
        })
    }

    /// The operand `imp` stands for (the `sub`th of a group), if any.
    fn implicit(&self, imp: &Imp, sub: u8) -> Option<Operand> {
        let inst = self.inst;
        let gpr = |size: u8, index: u8| Operand::Reg(Register::gpr(size, index, true));
        let stack = Register::gpr(inst.stack_size(), 4, true);
        Some(match imp.kind {
            ImpKind::Reg => Operand::Reg(Register::new(RegClass::from_u8(imp.a), imp.b)),
            ImpKind::GprOs => gpr(inst.operand_size(), imp.a),
            ImpKind::GprAs => gpr(inst.address_size(), imp.a),
            ImpKind::GprSs => gpr(inst.stack_size(), imp.a),
            ImpKind::Group => {
                let class = RegClass::from_u8(imp.a);
                Operand::Reg(Register::new(class, sub))
            }
            ImpKind::Pair => match inst.operands().get(usize::from(imp.a)) {
                Some(Operand::Reg(r)) => Operand::Reg(Register::new(r.class(), r.index() ^ 1)),
                _ => return None,
            },
            ImpKind::MaskField => {
                if inst.mask == 0 {
                    return None;
                }
                Operand::Reg(Register::new(RegClass::Mask, inst.mask))
            }
            ImpKind::Push => {
                // (An unknown number of slots: from the first one down, of
                // no known size.)
                let slot = u16::from(inst.operand_size());
                let bytes = u16::from(imp.a) * slot;
                self.memory(stack, -i64::from(bytes.max(slot)), bytes, None)
            }
            ImpKind::Pop => {
                let bytes = u16::from(imp.a) * u16::from(inst.operand_size());
                let base = if imp.b == 0 {
                    stack
                } else {
                    Register::gpr(inst.stack_size(), imp.b, true)
                };
                self.memory(base, 0, bytes, None)
            }
            ImpKind::MemAt => {
                let base = if imp.a & 0x80 != 0 {
                    match inst.operands().get(usize::from(imp.a & 0x7F)) {
                        Some(Operand::Reg(r)) => *r,
                        _ => return None,
                    }
                } else {
                    Register::gpr(inst.address_size(), imp.a, true)
                };
                self.memory(base, 0, u16::from(imp.b), self.segment())
            }
        })
    }

    /// How many registers group `imp` has.
    fn group_len(&self, imp: &Imp) -> u8 {
        if imp.kind != ImpKind::Group {
            return 1;
        }
        let n = imp.b & 0x7F;
        if imp.b & 0x80 != 0 && self.inst.mode != Mode::Bits64 {
            n.min(8)
        } else {
            n
        }
    }
}

impl Iterator for Accesses<'_> {
    type Item = Access;

    fn next(&mut self) -> Option<Access> {
        let inst = self.inst;
        let explicit = inst.operands().len();
        while self.next < explicit {
            let i = self.next;
            self.next += 1;
            match inst.ops[i] {
                Operand::Reg(_) | Operand::Mem(_) => {
                    return Some(Access {
                        operand: inst.ops[i],
                        rw: inst.operand_access(i),
                        implicit: false,
                    });
                }
                _ => {}
            }
        }
        while let Some(imp) = self.implicit.get(self.next - explicit) {
            let sub = self.sub;
            if sub + 1 < self.group_len(imp) {
                self.sub += 1;
            } else {
                self.sub = 0;
                self.next += 1;
            }
            if imp.rw & rw::REP != 0 && !inst.has_rep() {
                continue;
            }
            if let Some(operand) = self.implicit(imp, sub) {
                return Some(Access {
                    operand,
                    rw: Rw::from_code(imp.rw),
                    implicit: true,
                });
            }
        }
        if !self.mask_done {
            self.mask_done = true;
            if inst.mask != 0 {
                return Some(Access {
                    operand: Operand::Reg(Register::new(RegClass::Mask, inst.mask)),
                    rw: Rw::Read,
                    implicit: true,
                });
            }
        }
        None
    }
}

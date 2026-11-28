//! ARM64 as [`turbonop_isa`] sees it: the decoder ([`Arm64`]), and its
//! instructions' control flow, attributes, registers, memory, operands,
//! and text.

use turbonop_isa::{
    Access as AccessKind, Attributes, Base, BitSet, ControlFlow, DecodeError, Extend, FlowKind,
    Format, Immediate, Index, Isa, Memory, MemoryAccess, Offset, Operand as IsaOperand, Operands,
    RegisterAccess, Registers, Resolver, Target, TokenKind, TokenSink, Update,
};

use crate::access::{Effects, Reg};
use crate::operand::{Access, AddrMode, Mem, OperandKind, ShiftKind};
use crate::{Instruction, Opcode, opcodes};

/// The A64 decoder: every encoding of the architecture, whatever the
/// features (as Microsoft's disassembler, and LLVM's with every feature
/// on, decode).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Arm64;

/// ARM64's text syntaxes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Syntax {
    /// Microsoft's disassembler's (`dumpbin /disasm`).
    #[default]
    Microsoft,
}

impl turbonop_isa::Opcode for Opcode {
    const COUNT: u32 = crate::tables::OPCODE_FIRST.len() as u32;

    #[inline]
    fn index(self) -> u32 {
        u32::from(self.0)
    }

    #[inline]
    fn mnemonic(self) -> &'static str {
        Opcode::mnemonic(self)
    }
}

/// Whether a word is in the system instruction space (SYS, SYSL, MSR,
/// MRS, and their pair forms), where op1 (bits 18:16) says the lowest
/// exception level that may run it: 3, EL0.
fn system_op1(w: u32) -> Option<u32> {
    matches!(w >> 22, 0x354 | 0x355).then_some(w >> 16 & 7)
}

impl Instruction {
    /// How control leaves it, at `address`.
    pub fn flow(&self, address: u64) -> ControlFlow {
        let e = self.effects();
        let target = |indirect: bool| match self.at(address).label() {
            Some(t) if !indirect => Target::Direct(t),
            _ => Target::Indirect,
        };
        let indirect = e.contains(Effects::INDIRECT);
        if e.contains(Effects::RETURN) {
            ControlFlow::RETURN
        } else if e.contains(Effects::CALL) {
            ControlFlow::new(FlowKind::Call, target(indirect))
        } else if e.contains(Effects::BRANCH) {
            ControlFlow::new(FlowKind::Jump, target(indirect))
                .when(!e.contains(Effects::NO_FALLTHROUGH))
        } else if e.contains(Effects::EXCEPTION) {
            let op = self.opcode();
            if [
                opcodes::SVC_EX_EXCEPTION,
                opcodes::HVC_EX_EXCEPTION,
                opcodes::SMC_EX_EXCEPTION,
            ]
            .contains(&op)
            {
                ControlFlow::SYSTEM_CALL
            } else {
                ControlFlow::TRAP
            }
        } else if e.contains(Effects::NO_FALLTHROUGH) {
            ControlFlow::TRAP
        } else {
            ControlFlow::NEXT
        }
    }

    /// What it does beyond its registers, memory, and control flow (each
    /// attribute one it *may* have: a superset).
    pub fn attributes(&self) -> Attributes {
        let e = self.effects();
        let op = self.opcode();
        let mut a = Attributes::NONE;
        if e.contains(Effects::SYSTEM) {
            a |= Attributes::SYSTEM;
        }
        if e.contains(Effects::FP_EXCEPTION) {
            a |= Attributes::FP_EXCEPTION;
        }
        if op.attrs() & 1 != 0 {
            a |= Attributes::ATOMIC;
        }
        if op.attrs() & 2 != 0 {
            a |= Attributes::BARRIER;
        }
        let privileged = system_op1(self.word()).is_some_and(|op1| op1 != 3)
            || [
                opcodes::HVC_EX_EXCEPTION,
                opcodes::SMC_EX_EXCEPTION,
                opcodes::ERET_64E_BRANCH_REG,
                opcodes::ERETAA_64E_BRANCH_REG,
                opcodes::ERETAB_64E_BRANCH_REG,
                opcodes::DCPS1_DC_EXCEPTION,
                opcodes::DCPS2_DC_EXCEPTION,
                opcodes::DCPS3_DC_EXCEPTION,
            ]
            .contains(&op);
        if privileged {
            a |= Attributes::PRIVILEGED;
        }
        if self.is_unpredictable() {
            a |= Attributes::UNPREDICTABLE;
        }
        if op == opcodes::NOP_HI_HINTS {
            a |= Attributes::NOP;
        }
        a
    }

    /// Every access of memory it (at `address`) makes: its memory
    /// operands', and a literal load's (`ldr x0, <label>`). Sizes aren't
    /// known yet (`None`): they come from the architecture's decode
    /// pseudocode, which the tables don't have.
    pub fn memory_accesses(&self, address: u64) -> impl Iterator<Item = MemoryAccess<Reg>> + use<> {
        let at = self.at(address);
        // (A load from a label is a literal's: PC-relative.)
        let literal = (self.reads_memory() && !self.effects().contains(Effects::BRANCH))
            .then(|| at.label())
            .flatten()
            .map(|t| {
                #[allow(clippy::cast_possible_wrap)] // An address's bits.
                MemoryAccess::new(AccessKind::READ, Base::Pc, t as i64, None)
            });
        at.memory_operands()
            .filter_map(|o| match o.kind {
                OperandKind::Mem(m) => Some(memory(&m, o.access)),
                _ => None,
            })
            .chain(literal)
    }

    /// Its operands (at `address`), in the common form: a register list's
    /// registers each an operand; a label a branch's
    /// [`Target`](IsaOperand::Target), a literal load's memory, or else
    /// (`adr`, `adrp`) an [`Address`](IsaOperand::Address); conditions,
    /// shifts, and the like [`Other`](IsaOperand::Other)
    /// ([`Instruction::operands`] has them).
    pub fn common_operands(&self, address: u64) -> impl Iterator<Item = IsaOperand<Reg>> + use<> {
        let e = self.effects();
        let ops = self.operands(address);
        let mut out = [None; 4 * crate::MAX_OPERANDS];
        let mut n = 0;
        let mut push = |o| {
            out[n] = Some(o);
            n += 1;
        };
        for o in &ops {
            match o.kind {
                OperandKind::Reg(r) => push(IsaOperand::Register(r.reg, access(o.access))),
                OperandKind::List(l) => {
                    for r in l.iter() {
                        push(IsaOperand::Register(r, access(o.access)));
                    }
                }
                OperandKind::Imm(v) => push(IsaOperand::Immediate(Immediate {
                    value: v,
                    bits: 0,
                    signed: v < 0,
                })),
                OperandKind::Label(t) if e.intersects(Effects::BRANCH | Effects::CALL) => {
                    push(IsaOperand::Target(t));
                }
                #[allow(clippy::cast_possible_wrap)] // An address's bits.
                OperandKind::Label(t) if e.contains(Effects::LOAD) => push(IsaOperand::Memory(
                    MemoryAccess::new(AccessKind::READ, Base::Pc, t as i64, None),
                )),
                OperandKind::Label(t) => push(IsaOperand::Address(t)),
                OperandKind::Mem(m) => push(IsaOperand::Memory(memory(&m, o.access))),
                _ => push(IsaOperand::Other),
            }
        }
        out.into_iter().flatten()
    }
}

fn access(a: Access) -> AccessKind {
    match (a.reads(), a.writes()) {
        (true, true) => AccessKind::READ_WRITE,
        (true, false) => AccessKind::READ,
        (false, true) => AccessKind::WRITE,
        (false, false) => AccessKind::NONE,
    }
}

/// A memory operand as an access (its size not known: see
/// [`Instruction::memory_accesses`]).
fn memory(m: &Mem, a: Access) -> MemoryAccess<Reg> {
    let index = |r: Reg| Index {
        register: r,
        extend: match m.index_shift.map(|s| s.0) {
            Some(ShiftKind::Uxtw) => Extend::Zero(32),
            Some(ShiftKind::Sxtw) => Extend::Sign(32),
            Some(ShiftKind::Uxtb) => Extend::Zero(8),
            Some(ShiftKind::Sxtb) => Extend::Sign(8),
            Some(ShiftKind::Uxth) => Extend::Zero(16),
            Some(ShiftKind::Sxth) => Extend::Sign(16),
            _ => Extend::None,
        },
        shift: m.index_shift.map_or(0, |s| s.1),
    };
    let post = m.mode == AddrMode::PostIndex;
    MemoryAccess {
        access: access(a),
        base: Base::Register(m.base),
        // (Post-indexed: the access is at the base; the index or offset is
        // what it's advanced by.)
        index: if post { None } else { m.index.map(index) },
        displacement: if post { 0 } else { m.offset },
        segment: None,
        update: match m.mode {
            AddrMode::Offset => Update::None,
            AddrMode::PreIndex => Update::Pre,
            AddrMode::PostIndex => Update::Post(match m.index {
                Some(r) => Offset::Register(r),
                None => Offset::Immediate(m.offset),
            }),
        },
        size: None,
        vector_length_units: m.mul_vl,
        space: 0,
        implicit: false,
    }
}

impl Isa for Arm64 {
    type Instruction = Instruction;
    type Opcode = Opcode;
    type Context = ();

    const MIN_LENGTH: usize = 4;
    const MAX_LENGTH: usize = 4;
    const ALIGNMENT: usize = 4;

    #[inline]
    fn context(&self, _: &[u8], _: u64) {}

    #[inline]
    fn decode(&self, _: &(), bytes: &[u8], _: u64) -> Result<Instruction, DecodeError> {
        let word = bytes
            .first_chunk::<4>()
            .ok_or(DecodeError::Truncated { needed: 4 })?;
        crate::decode(u32::from_le_bytes(*word))
    }

    #[inline]
    fn bytes(i: &Instruction) -> &[u8] {
        i.bytes()
    }

    #[inline]
    fn len(_: &Instruction) -> usize {
        4
    }

    #[inline]
    fn opcode(i: &Instruction) -> Opcode {
        i.opcode()
    }

    #[inline]
    fn flow(&self, _: &(), i: &Instruction, address: u64) -> ControlFlow {
        i.flow(address)
    }

    #[inline]
    fn attributes(&self, i: &Instruction) -> Attributes {
        i.attributes()
    }
}

impl Registers for Arm64 {
    type Register = Reg;
    type RegisterSet = BitSet<Reg, 2>;

    #[inline]
    fn register_accesses(
        &self,
        _: &(),
        i: &Instruction,
        _: u64,
    ) -> impl Iterator<Item = RegisterAccess<Reg>> {
        i.register_accesses()
    }
}

impl Memory for Arm64 {
    #[inline]
    fn memory_accesses(
        &self,
        _: &(),
        i: &Instruction,
        address: u64,
    ) -> impl Iterator<Item = MemoryAccess<Reg>> {
        i.memory_accesses(address)
    }
}

impl Operands for Arm64 {
    #[inline]
    fn operands(
        &self,
        _: &(),
        i: &Instruction,
        address: u64,
    ) -> impl Iterator<Item = IsaOperand<Reg>> {
        i.common_operands(address)
    }
}

impl Format for Arm64 {
    type Syntax = Syntax;

    /// The text as a [`TokenKind::Mnemonic`] token and [`TokenKind::Text`]
    /// (the formatter doesn't yet say what its pieces are).
    fn format<S: TokenSink + ?Sized, N: Resolver + ?Sized>(
        &self,
        _: &(),
        i: &Instruction,
        address: u64,
        _: Syntax,
        names: &N,
        out: &mut S,
    ) {
        /// `names` as a `dyn Resolver` (it may be unsized).
        struct Dyn<'a, N: ?Sized>(&'a N);
        impl<N: Resolver + ?Sized> Resolver for Dyn<'_, N> {
            fn resolve(&self, r: turbonop_isa::Ref, out: &mut dyn TokenSink) -> bool {
                self.0.resolve(r, out)
            }
        }
        let mut text = alloc::string::String::with_capacity(48);
        crate::format_into(i, address, &Dyn(names), &mut text);
        let (mnemonic, rest) = text.split_once(' ').unwrap_or((&text, ""));
        out.token(TokenKind::Mnemonic, mnemonic);
        if !rest.is_empty() {
            out.token(TokenKind::Text, " ");
            out.token(TokenKind::Text, rest);
        }
    }
}

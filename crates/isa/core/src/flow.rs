//! How control leaves an instruction.

/// What kind of transfer of control an instruction is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FlowKind {
    /// None: control goes on to the next instruction.
    Next,
    /// A jump (a branch that isn't a call or a return).
    Jump,
    /// A call: the callee runs, and returns to the next instruction.
    Call,
    /// A return (from a function, or from an exception).
    Return,
    /// A system call (or hypervisor or monitor call): the system runs,
    /// and returns to the next instruction.
    SystemCall,
    /// A trap that doesn't return here: a breakpoint (`int3`, `brk`), an
    /// undefined instruction (`ud2`, `udf`), a halt, an abort.
    Trap,
}

/// Where a transfer of control goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    /// Nowhere named: control goes on, or returns, or traps.
    None,
    /// The address the instruction names.
    Direct(u64),
    /// An address computed at run time (from a register, from memory,
    /// from the stack).
    Indirect,
    /// One of a table's addresses, by an index computed at run time (a
    /// `switch`, a `br_table`): [`Isa::table_targets`](crate::Isa::table_targets).
    Table,
}

/// Which instructions after a transfer of control run before it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Delay {
    /// None: control transfers at once.
    None,
    /// The next instruction (its delay slot) runs first.
    Slot,
    /// The next instruction runs first only if the branch is taken (it is
    /// annulled if not: MIPS's branch-likely, SPARC's annulled branches).
    SlotIfTaken,
}

/// How control leaves an instruction: its kind, whether a condition
/// decides it, where to, and its delay slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ControlFlow {
    /// What kind of transfer it is.
    pub kind: FlowKind,
    /// Whether it happens only if a condition holds (and control goes on
    /// to the next instruction if not).
    pub conditional: bool,
    /// Where it goes.
    pub target: Target,
    /// Its delay slot.
    pub delay: Delay,
}

impl ControlFlow {
    /// No transfer: on to the next instruction.
    pub const NEXT: ControlFlow = ControlFlow {
        kind: FlowKind::Next,
        conditional: false,
        target: Target::None,
        delay: Delay::None,
    };

    /// A return.
    pub const RETURN: ControlFlow = ControlFlow::new(FlowKind::Return, Target::None);

    /// A trap.
    pub const TRAP: ControlFlow = ControlFlow::new(FlowKind::Trap, Target::None);

    /// A system call.
    pub const SYSTEM_CALL: ControlFlow = ControlFlow::new(FlowKind::SystemCall, Target::None);

    /// An unconditional transfer of `kind` to `target`, no delay slot.
    #[inline]
    pub const fn new(kind: FlowKind, target: Target) -> ControlFlow {
        ControlFlow {
            kind,
            conditional: false,
            target,
            delay: Delay::None,
        }
    }

    /// This, but only if a condition holds.
    #[inline]
    #[must_use]
    pub const fn when(self, conditional: bool) -> ControlFlow {
        ControlFlow {
            conditional,
            ..self
        }
    }

    /// This, after `delay`.
    #[inline]
    #[must_use]
    pub const fn delayed(self, delay: Delay) -> ControlFlow {
        ControlFlow { delay, ..self }
    }

    /// Whether control may go on to the next instruction (after the
    /// delay slot, if any): if it doesn't transfer, or the transfer
    /// returns (a call), or a condition may not hold.
    #[inline]
    pub const fn falls_through(self) -> bool {
        self.conditional
            || matches!(
                self.kind,
                FlowKind::Next | FlowKind::Call | FlowKind::SystemCall
            )
    }

    /// Whether it ends a basic block: it may transfer control anywhere
    /// but to the next instruction (a call doesn't: control returns).
    #[inline]
    pub const fn ends_block(self) -> bool {
        matches!(
            self.kind,
            FlowKind::Jump | FlowKind::Return | FlowKind::Trap
        )
    }

    /// The address it names, if it names one.
    #[inline]
    pub const fn direct_target(self) -> Option<u64> {
        match self.target {
            Target::Direct(a) => Some(a),
            _ => None,
        }
    }
}

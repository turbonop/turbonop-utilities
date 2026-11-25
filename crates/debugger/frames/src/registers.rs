//! A frame's registers, each with where its value came from.

/// Where a register's value in a frame came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    /// The target's record of the thread (its context, or the exception's).
    Context,
    /// Read from the stack, where the callee saved it: at this address.
    Saved(u64),
    /// The callee's canonical frame address: the caller's stack pointer.
    Cfa,
    /// The callee's return address: the caller's PC.
    ReturnAddress,
    /// A signal (or exception) context on the stack, at this address: the
    /// interrupted code's registers.
    SignalContext(u64),
    /// A frame-pointer chain's record, at this address.
    FrameRecord(u64),
    /// A stack slot a scan found, at this address.
    Scanned(u64),
    /// Restored by the module's frame data (x86's FrameData program).
    FrameData,
}

/// What's known of a register in a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RegState {
    /// Its value (its low `bits` bits: a callee may preserve only part of
    /// a register, as AAPCS64's V8..V15), and where it came from.
    Known {
        value: u128,
        bits: u16,
        from: Source,
    },
    /// The calling convention lets the callee change it, and nothing says
    /// what it was: the caller's value is lost.
    Clobbered,
}

impl RegState {
    pub fn known(value: u128, from: Source) -> RegState {
        RegState::Known {
            value,
            bits: 128,
            from,
        }
    }
}

/// A frame's registers by DWARF number. A register absent is unknown: the
/// target didn't record it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrameRegisters {
    regs: Vec<(u16, RegState)>,
}

impl FrameRegisters {
    pub fn new() -> FrameRegisters {
        FrameRegisters::default()
    }

    /// From a target's registers, each known from its context.
    pub fn from_context(registers: &turbonop_target::Registers) -> FrameRegisters {
        FrameRegisters {
            regs: registers
                .iter()
                .map(|(n, v)| (n, RegState::known(v, Source::Context)))
                .collect(),
        }
    }

    pub fn get(&self, register: u16) -> Option<RegState> {
        let i = self.regs.binary_search_by_key(&register, |r| r.0).ok()?;
        Some(self.regs[i].1)
    }

    /// Its value, if known (its low 64 bits).
    pub fn value(&self, register: u16) -> Option<u64> {
        match self.get(register)? {
            #[allow(clippy::cast_possible_truncation)]
            RegState::Known { value, .. } => Some(value as u64),
            RegState::Clobbered => None,
        }
    }

    pub fn set(&mut self, register: u16, state: Option<RegState>) {
        match (self.regs.binary_search_by_key(&register, |r| r.0), state) {
            (Ok(i), Some(s)) => self.regs[i].1 = s,
            (Ok(i), None) => {
                self.regs.remove(i);
            }
            (Err(i), Some(s)) => self.regs.insert(i, (register, s)),
            (Err(_), None) => {}
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, RegState)> + '_ {
        self.regs.iter().copied()
    }

    /// The known values, as the unwinders take them.
    pub fn to_unwind(&self) -> turbonop_unwind::dwarf_cfi::Registers {
        let mut r = turbonop_unwind::dwarf_cfi::Registers::default();
        for (n, s) in self.iter() {
            if let RegState::Known { value, .. } = s {
                #[allow(clippy::cast_possible_truncation)]
                r.set(n, Some(value as u64));
            }
        }
        r
    }
}

//! Stack walking: each thread's frames, from its registers, its stack, and
//! its modules' unwind data, with every register's provenance in every
//! frame.
//!
//! A walk starts from the thread's registers (or, for the thread that took
//! the exception, the exception's: the thread's own may be where a handler
//! or the dump writer stopped it). Each step unwinds by the frame's
//! module's unwind data ([`turbonop_unwind::any`]: `.pdata`, compact
//! unwind, EHABI, DWARF CFI), then applies the calling convention
//! ([`turbonop_abi`]): a register the callee may change and didn't restore
//! is [`RegState::Clobbered`] in the caller, never kept. A return address
//! is stripped of any pointer authentication code.
//!
//! Where there's no unwind data (a module whose image wasn't found, code
//! that isn't in a module) or it fails, the walk falls back to the frame
//! pointer chain, then to scanning the stack, and each frame found so is
//! [`Confidence::Likely`]; a candidate return address is taken only if the
//! instruction before it is a call. Linux signal frames (the kernel's
//! return trampolines on x86-64 and AArch64) are recognized, and the
//! interrupted code's registers read from the signal's context.
//!
//! Each frame is symbolized by its module's debug information (its
//! function, and the calls inlined at its address, innermost first), or
//! by its image's own symbols. Threads are walked on every core.

mod calls;
pub mod reconstruct;
mod registers;
mod signal;
mod symbols;

pub use calls::follows_call;
pub use registers::{FrameRegisters, RegState, Source};
pub use symbols::{Modules, Symbol};

use turbonop_abi::{Abi, Os, Preserved, ReturnAddress};
use turbonop_core::Arch;
use turbonop_target::{AddressSpace, Target, Thread};
use turbonop_unwind::any::{self, Method as UnwindMethod};

/// How a frame was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Method {
    /// The thread's (or the exception's) registers: the innermost frame.
    Context,
    /// Its callee's unwind data: a PE image's `.pdata`.
    Pdata,
    /// A leaf callee (no unwind data: its return address where a call
    /// left it).
    Leaf,
    /// Its callee's compact unwind encoding (Mach-O).
    Compact,
    /// Its callee's ARM EHABI entry.
    Ehabi,
    /// Its callee's DWARF call frame information.
    Cfi,
    /// A signal's context, saved on the stack by the kernel.
    SignalFrame,
    /// Its callee's frame record (the frame pointer chain).
    FramePointer,
    /// A scan of the stack for a return address.
    Scan,
}

/// How sure a frame is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Confidence {
    /// Found by unwind data (or a signal context): it is the caller.
    Certain,
    /// Found by a heuristic (the frame pointer chain, a stack scan): it's
    /// probably the caller.
    Likely,
}

/// A frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Its PC: where it is (the innermost frame, or one a signal
    /// interrupted), or the return address its callee returns to.
    pub pc: u64,
    /// Its stack pointer.
    pub sp: u64,
    /// Whether its PC is a return address (so the call is the instruction
    /// before it, and its code is looked up there).
    pub return_address: bool,
    pub method: Method,
    pub confidence: Confidence,
    /// Its registers.
    pub registers: FrameRegisters,
    /// The module its PC is in (an index into the target's modules).
    pub module: Option<usize>,
    /// Its function, and the calls inlined at its PC.
    pub symbol: Option<Symbol>,
}

impl Frame {
    /// The address its code is looked up by: its PC, or (a return
    /// address) the byte before it, in the call.
    pub fn lookup(&self) -> u64 {
        if self.return_address {
            self.pc.wrapping_sub(1)
        } else {
            self.pc
        }
    }
}

/// Why a walk ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// The outermost frame: its unwind data says so, or its return address
    /// is 0.
    Outermost,
    /// No way on was found (no unwind data, and no fallback found a
    /// caller).
    NoCaller,
    /// The next frame's stack pointer wasn't above this one's.
    StackNotGrowing,
    /// The walk came back to a frame it had found.
    Loop,
    /// The most frames a walk takes.
    TooDeep,
    /// The innermost frame's registers aren't known.
    NoRegisters,
}

/// A thread's frames, innermost first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stack {
    pub thread: u64,
    pub frames: Vec<Frame>,
    pub end: End,
}

/// The most frames a walk takes.
pub const MAX_FRAMES: usize = 1024;

/// The most stack slots a scan reads for a return address.
const SCAN_SLOTS: u64 = 2048;

/// Walks every thread's stack, on every core.
pub fn walk_all(
    target: &dyn Target,
    space: &AddressSpace<'_>,
    modules: &Modules<'_>,
) -> Vec<Stack> {
    let threads = target.threads();
    turbonop_par::map(threads, 1, |t| walk(target, space, modules, t))
}

/// Walks a thread's stack.
pub fn walk(
    target: &dyn Target,
    space: &AddressSpace<'_>,
    modules: &Modules<'_>,
    thread: &Thread,
) -> Stack {
    let arch = target.arch();
    let abi = target.abi().or_else(|| Abi::new(arch, Os::Linux));
    let Some(abi) = abi else {
        return Stack {
            thread: thread.id,
            frames: Vec::new(),
            end: End::NoRegisters,
        };
    };
    // The exception's registers, for the thread that took it.
    let start = match target.exception() {
        Some(e) if e.thread == thread.id && e.registers.is_some() => {
            e.registers.as_ref().unwrap_or(&thread.registers)
        }
        _ => &thread.registers,
    };
    let registers = FrameRegisters::from_context(start);
    let (pc_reg, sp_reg) = (abi.program_counter(), abi.stack_pointer());
    let (Some(pc), Some(sp)) = (registers.value(pc_reg), registers.value(sp_reg)) else {
        return Stack {
            thread: thread.id,
            frames: Vec::new(),
            end: End::NoRegisters,
        };
    };
    let walker = Walker {
        target,
        space,
        modules,
        abi,
        arch,
        pc_reg,
        sp_reg,
    };
    let mut frames = vec![walker.frame(
        pc,
        sp,
        false,
        Method::Context,
        Confidence::Certain,
        registers,
    )];
    let end = loop {
        if frames.len() >= MAX_FRAMES {
            break End::TooDeep;
        }
        let callee = frames.last().unwrap_or_else(|| unreachable!());
        let Some(next) = walker.step(callee, frames.len() == 1) else {
            break End::NoCaller;
        };
        let Some(next) = next else {
            break End::Outermost;
        };
        // (A signal frame's interrupted code may be anywhere on the stack
        // the signal ran on: an alternate stack is below or above.)
        if next.method != Method::SignalFrame
            && callee.method != Method::SignalFrame
            && next.sp <= callee.sp
        {
            // (Equal only for a leaf's caller on ARM, whose return address
            // is in a register: the same frame, a step on.)
            let same_sp_ok =
                next.sp == callee.sp && callee.method == Method::Context && next.pc != callee.pc;
            if !same_sp_ok {
                break End::StackNotGrowing;
            }
        }
        if frames.iter().any(|f| f.pc == next.pc && f.sp == next.sp) {
            break End::Loop;
        }
        frames.push(next);
    };
    Stack {
        thread: thread.id,
        frames,
        end,
    }
}

pub(crate) struct Walker<'w, 's, 'm> {
    pub(crate) target: &'w dyn Target,
    pub(crate) space: &'w AddressSpace<'s>,
    pub(crate) modules: &'w Modules<'m>,
    pub(crate) abi: Abi,
    pub(crate) arch: Arch,
    pub(crate) pc_reg: u16,
    pub(crate) sp_reg: u16,
}

impl Walker<'_, '_, '_> {
    pub(crate) fn frame(
        &self,
        pc: u64,
        sp: u64,
        return_address: bool,
        method: Method,
        confidence: Confidence,
        registers: FrameRegisters,
    ) -> Frame {
        let mut f = Frame {
            pc,
            sp,
            return_address,
            method,
            confidence,
            registers,
            module: None,
            symbol: None,
        };
        f.module = self
            .target
            .modules()
            .iter()
            .position(|m| m.contains(f.lookup()));
        f.symbol = f.module.and_then(|m| self.modules.symbol(m, f.lookup()));
        f
    }

    /// The caller of `callee`: `None` if none was found, `Some(None)` if
    /// it's the outermost frame.
    fn step(&self, callee: &Frame, innermost: bool) -> Option<Option<Frame>> {
        // A signal's return trampoline: the interrupted code's registers.
        if let Some(r) = signal::sigreturn(self.space, self.arch, self.abi.os(), callee) {
            let pc = r.value(self.pc_reg)?;
            let sp = r.value(self.sp_reg)?;
            return Some(Some(self.frame(
                pc,
                sp,
                false,
                Method::SignalFrame,
                Confidence::Certain,
                r,
            )));
        }
        if let Some(found) = self.by_unwind_data(callee) {
            return Some(found);
        }
        // The fallbacks: heuristics.
        if let Some(f) = self.by_frame_pointer(callee) {
            return Some(Some(f));
        }
        self.by_scan(callee, innermost).map(Some)
    }

    pub(crate) fn by_unwind_data(&self, callee: &Frame) -> Option<Option<Frame>> {
        let m = callee.module?;
        let (image, bias) = self.modules.image(m)?;
        let mut regs = callee.registers.to_unwind();
        // (The unwinders take the PC as the frame's: for a caller, a
        // return address, which they look up by the byte before.)
        let u = any::unwind(image, bias, callee.return_address, &mut regs, self.space).ok()?;
        let method = match u.method {
            UnwindMethod::Pdata => Method::Pdata,
            UnwindMethod::Leaf => Method::Leaf,
            UnwindMethod::Compact => Method::Compact,
            UnwindMethod::ArmExidx => Method::Ehabi,
            UnwindMethod::Dwarf(_) => Method::Cfi,
        };
        let Some(ra) = u.frame.return_address else {
            return Some(None);
        };
        let pc = self.strip(ra);
        if pc == 0 {
            return Some(None);
        }
        let sp = u.frame.cfa;
        let mut caller = FrameRegisters::new();
        for &n in self.abi.registers() {
            let saved = u.frame.saved.iter().find(|s| s.0 == n).map(|s| s.1);
            let state = if n == self.sp_reg {
                Some(RegState::known(u128::from(sp), Source::Cfa))
            } else if n == self.pc_reg {
                Some(RegState::known(u128::from(pc), Source::ReturnAddress))
            } else if let Some(at) = saved {
                regs.get(n)
                    .map(|v| RegState::known(u128::from(v), Source::Saved(at)))
            } else {
                self.carried(callee.registers.get(n), n)
            };
            caller.set(n, state);
        }
        // (A link register a call wrote: the caller's own isn't known
        // unless the callee saved it.)
        if let ReturnAddress::Register(lr) = self.abi.return_address()
            && !u.frame.saved.iter().any(|s| s.0 == lr)
        {
            caller.set(lr, Some(RegState::Clobbered));
        }
        Some(Some(self.frame(
            pc,
            sp,
            true,
            method,
            Confidence::Certain,
            caller,
        )))
    }

    /// What the caller knows of register `n`, which the callee didn't save:
    /// its value in the callee, if the convention preserves it.
    fn carried(&self, in_callee: Option<RegState>, n: u16) -> Option<RegState> {
        match self.abi.preserved(n) {
            Preserved::Whole | Preserved::Constant => in_callee,
            Preserved::Low64 => match in_callee? {
                RegState::Known { value, from, .. } => Some(RegState::Known {
                    value: value & u128::from(u64::MAX),
                    bits: 64,
                    from,
                }),
                RegState::Clobbered => Some(RegState::Clobbered),
            },
            Preserved::Not => Some(RegState::Clobbered),
        }
    }

    /// A caller with only what a heuristic gives: its PC and stack
    /// pointer (and frame pointer), and the registers that never change.
    pub(crate) fn heuristic_registers(
        &self,
        pc: u64,
        sp: u64,
        from: Source,
        callee: &Frame,
    ) -> FrameRegisters {
        let mut r = FrameRegisters::new();
        for &n in self.abi.registers() {
            if self.abi.preserved(n) == Preserved::Constant {
                r.set(n, callee.registers.get(n));
            }
        }
        r.set(self.pc_reg, Some(RegState::known(u128::from(pc), from)));
        r.set(self.sp_reg, Some(RegState::known(u128::from(sp), from)));
        r
    }

    pub(crate) fn word(&self, at: u64) -> Option<u64> {
        let size = self.arch.pointer_size()?;
        let mut b = [0u8; 8];
        let n = usize::from(size);
        self.space.read(at, &mut b[..n])?;
        Some(if n == 4 {
            u64::from(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        } else {
            u64::from_le_bytes(b)
        })
    }

    /// The frame pointer chain: the callee's frame record (the caller's
    /// frame pointer, then its return address), where the frame pointer
    /// points.
    fn by_frame_pointer(&self, callee: &Frame) -> Option<Frame> {
        let fp_reg = self.abi.frame_pointer()?;
        let fp = callee.registers.value(fp_reg)?;
        let size = u64::from(self.arch.pointer_size()?);
        if fp < callee.sp || fp % size != 0 {
            return None;
        }
        let saved_fp = self.word(fp)?;
        let ra = self.strip(self.word(fp + size)?);
        if ra == 0 || !self.is_code(ra) || !follows_call(self.space, self.arch, ra) {
            return None;
        }
        let sp = fp + 2 * size;
        let mut r = self.heuristic_registers(ra, sp, Source::FrameRecord(fp), callee);
        r.set(
            fp_reg,
            Some(RegState::known(
                u128::from(saved_fp),
                Source::FrameRecord(fp),
            )),
        );
        Some(self.frame(ra, sp, true, Method::FramePointer, Confidence::Likely, r))
    }

    /// A scan of the stack from the callee's stack pointer (or, for the
    /// innermost frame, which may not have pushed anything yet, there
    /// too) for the first word that's a return address: code in a module
    /// that follows a call.
    fn by_scan(&self, callee: &Frame, innermost: bool) -> Option<Frame> {
        let size = u64::from(self.arch.pointer_size()?);
        // (A caller's own frame starts above the callee's return address;
        // past the innermost frame, the callee's stack pointer is the
        // CFA of its own callee, its frame's bottom.)
        let _ = innermost;
        for k in 0..SCAN_SLOTS {
            let at = callee.sp.checked_add(k * size)?;
            let Some(w) = self.word(at) else { break };
            let ra = self.strip(w);
            if ra == callee.pc || !self.is_code(ra) || !follows_call(self.space, self.arch, ra) {
                continue;
            }
            // (On x86, the return address is popped by the return: the
            // caller's stack pointer is above it.)
            let sp = at + size;
            let r = self.heuristic_registers(ra, sp, Source::Scanned(at), callee);
            return Some(self.frame(ra, sp, true, Method::Scan, Confidence::Likely, r));
        }
        None
    }

    /// Whether `address` is in a module (code, as far as can be told).
    pub(crate) fn is_code(&self, address: u64) -> bool {
        self.target.module_at(address).is_some()
    }

    /// A return address stripped of a pointer authentication code (ARM64),
    /// by the narrowest address width that puts it in a module.
    pub(crate) fn strip(&self, address: u64) -> u64 {
        if self.arch != Arch::Arm64 || self.is_code(address) {
            return address;
        }
        for bits in [48, 47, 39, 36] {
            let a = self.abi.strip_code_address(address, bits);
            if self.is_code(a) {
                return a;
            }
        }
        self.abi.strip_code_address(address, 48)
    }
}

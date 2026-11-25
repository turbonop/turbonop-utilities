//! Linux signal frames: when a handler returns, it returns to the kernel's
//! trampoline (`__restore_rt` in glibc on x86-64; `__kernel_rt_sigreturn`
//! in the vDSO on AArch64), which asks the kernel to restore the
//! interrupted code's registers from the `ucontext` on the stack. A frame
//! whose PC is the trampoline is read that way: its caller is the
//! interrupted code, at the instruction it was stopped at.
//!
//! The layouts are the kernel's (`rt_sigframe`, `ucontext`, each
//! architecture's `sigcontext`).

use turbonop_abi::{Os, regs};
use turbonop_core::Arch;
use turbonop_target::AddressSpace;

use crate::{Frame, FrameRegisters, RegState, Source};

/// x86-64's `__restore_rt`: `mov rax, 15` (rt_sigreturn), `syscall`.
const X64_RESTORE_RT: [u8; 9] = [0x48, 0xC7, 0xC0, 0x0F, 0x00, 0x00, 0x00, 0x0F, 0x05];

/// AArch64's `__kernel_rt_sigreturn`: `mov x8, #139`, `svc #0`.
const ARM64_RT_SIGRETURN: [u8; 8] = [0x68, 0x11, 0x80, 0xD2, 0x01, 0x00, 0x00, 0xD4];

/// Where the general registers are, from the stack pointer at the
/// trampoline. x86-64: the `ucontext` is at the stack pointer (the
/// handler's return popped `pretcode`), its `mcontext` 40 bytes in
/// (`uc_flags`, `uc_link`, `uc_stack`), its registers R8..R15, RDI, RSI,
/// RBP, RBX, RDX, RAX, RCX, RSP, RIP, EFLAGS.
const X64_GREGS: u64 = 40;

/// AArch64: the `siginfo` (128 bytes) then the `ucontext`, whose
/// `sigcontext` is 176 bytes in (`uc_flags`, `uc_link`, `uc_stack`,
/// `uc_sigmask` padded to 128 bytes, then 16-aligned): `fault_address`,
/// then X0..X30, SP, PC, PSTATE.
const ARM64_SIGCONTEXT: u64 = 128 + 176;

/// If `frame` is at a signal trampoline, the interrupted code's registers.
pub fn sigreturn(
    space: &AddressSpace<'_>,
    arch: Arch,
    os: Os,
    frame: &Frame,
) -> Option<FrameRegisters> {
    if !matches!(os, Os::Linux | Os::Android) {
        return None;
    }
    // (The trampoline's address itself: a handler returns to its first
    // instruction, so a return address there isn't after a call.)
    let at = frame.pc;
    match arch {
        Arch::X64 => {
            let mut code = [0u8; 9];
            space.read(at, &mut code)?;
            if code != X64_RESTORE_RT {
                return None;
            }
            let base = frame.sp.checked_add(X64_GREGS)?;
            use regs::x64::*;
            let order = [
                R8, R9, R10, R11, R12, R13, R14, R15, RDI, RSI, RBP, RBX, RDX, RAX, RCX, RSP, RIP,
                RFLAGS,
            ];
            let mut r = FrameRegisters::new();
            for (i, n) in (0u64..).zip(order) {
                let slot = base + 8 * i;
                let mut b = [0u8; 8];
                space.read(slot, &mut b)?;
                r.set(
                    n,
                    Some(RegState::known(
                        u128::from(u64::from_le_bytes(b)),
                        Source::SignalContext(slot),
                    )),
                );
            }
            Some(r)
        }
        Arch::Arm64 => {
            let mut code = [0u8; 8];
            space.read(at, &mut code)?;
            if code != ARM64_RT_SIGRETURN {
                return None;
            }
            let base = frame.sp.checked_add(ARM64_SIGCONTEXT + 8)?;
            let mut r = FrameRegisters::new();
            // X0..X30, SP, PC.
            for n in 0u16..33 {
                let slot = base + 8 * u64::from(n);
                let mut b = [0u8; 8];
                space.read(slot, &mut b)?;
                r.set(
                    n,
                    Some(RegState::known(
                        u128::from(u64::from_le_bytes(b)),
                        Source::SignalContext(slot),
                    )),
                );
            }
            Some(r)
        }
        _ => None,
    }
}

//! Minidump register contexts ([`turbonop_minidump::context::Context`]) as
//! [`Registers`], by DWARF number.
//!
//! A context's flags say which groups of registers it holds (Windows'
//! `CONTEXT_CONTROL`, `CONTEXT_INTEGER`, `CONTEXT_SEGMENTS`,
//! `CONTEXT_FLOATING_POINT`; Breakpad's ARM `MD_CONTEXT_ARM_INTEGER` and
//! `MD_CONTEXT_ARM_FLOATING_POINT`): registers in a group it doesn't hold
//! are left unknown, never taken as zero. A context with none of its
//! architecture's group flags set (some writers leave them clear) is read
//! as holding every group.
//!
//! Not read: x87 and MMX registers, x86's XMM registers (in the
//! context's extended registers), and AVX state (in a separate
//! `CONTEXT_XSTATE` area).

use turbonop_abi::regs;
use turbonop_core::Arch;
use turbonop_minidump::context::{BREAKPAD_ARM, Context};

use crate::Registers;

/// `CONTEXT_CONTROL`: the PC, stack and frame pointers, flags (and on
/// ARM, the link register).
pub const CONTROL: u32 = 0x1;
/// `CONTEXT_INTEGER`: the general registers.
pub const INTEGER: u32 = 0x2;
/// x86's and x64's `CONTEXT_SEGMENTS`.
pub const SEGMENTS: u32 = 0x4;
/// x86's and x64's `CONTEXT_FLOATING_POINT` (x64: the XMM registers and
/// MXCSR).
pub const FLOATING_POINT: u32 = 0x8;
/// ARM's and ARM64's `CONTEXT_FLOATING_POINT` (the D or V registers).
pub const ARM_FLOATING_POINT: u32 = 0x4;

/// The architecture of a context and its registers, by DWARF number; None
/// for a layout not decoded.
pub fn registers(context: &Context) -> Option<(Arch, Registers)> {
    let mut out = Registers::new();
    let mut set = |n: u16, value: u128| out.set(n, Some(value));
    let arch = match context {
        Context::X86(c) => {
            use regs::x86::*;
            let g = groups(c.flags, CONTROL | INTEGER | SEGMENTS | FLOATING_POINT);
            if g & CONTROL != 0 {
                for (n, v) in [(EBP, c.ebp), (EIP, c.eip), (EFLAGS, c.eflags), (ESP, c.esp)] {
                    set(n, u128::from(v));
                }
                // CS and SS (the segments are GS, FS, ES, DS, CS, SS).
                set(CS, u128::from(c.segments[4]));
                set(SS, u128::from(c.segments[5]));
            }
            if g & INTEGER != 0 {
                for (n, v) in [
                    (EDI, c.edi),
                    (ESI, c.esi),
                    (EBX, c.ebx),
                    (EDX, c.edx),
                    (ECX, c.ecx),
                    (EAX, c.eax),
                ] {
                    set(n, u128::from(v));
                }
            }
            if g & SEGMENTS != 0 {
                for (n, i) in [(GS, 0), (FS, 1), (ES, 2), (DS, 3)] {
                    set(n, u128::from(c.segments[i]));
                }
            }
            Arch::X86
        }
        Context::Amd64(c) => {
            use regs::x64::*;
            let g = groups(c.flags, CONTROL | INTEGER | SEGMENTS | FLOATING_POINT);
            if g & CONTROL != 0 {
                set(RIP, u128::from(c.rip));
                set(RSP, u128::from(c.rsp));
                set(RFLAGS, u128::from(c.eflags));
                // CS and SS (the segments are CS, DS, ES, FS, GS, SS).
                set(CS, u128::from(c.segments[0]));
                set(SS, u128::from(c.segments[5]));
            }
            if g & INTEGER != 0 {
                for (i, &n) in BY_ENCODING.iter().enumerate() {
                    // (RSP is CONTEXT_CONTROL's.)
                    if n != RSP {
                        set(n, u128::from(c.gpr[i]));
                    }
                }
            }
            if g & SEGMENTS != 0 {
                for (n, i) in [(DS, 1), (ES, 2), (FS, 3), (GS, 4)] {
                    set(n, u128::from(c.segments[i]));
                }
            }
            if g & FLOATING_POINT != 0 {
                for (i, &v) in c.xmm.iter().enumerate() {
                    set(xmm(index(i)), v);
                }
                set(MXCSR, u128::from(c.mxcsr));
            }
            Arch::X64
        }
        Context::Arm(c) => {
            use regs::arm::*;
            let breakpad = c.flags & BREAKPAD_ARM != 0;
            let g = groups(c.flags, CONTROL | INTEGER | ARM_FLOATING_POINT);
            // Breakpad's integer group holds every core register.
            let control = g & CONTROL != 0 || (breakpad && g & INTEGER != 0);
            if control {
                for n in [SP, LR, PC] {
                    set(n, u128::from(c.r[usize::from(n)]));
                }
            }
            if g & INTEGER != 0 {
                for (i, &v) in c.r[..13].iter().enumerate() {
                    set(r(index(i)), u128::from(v));
                }
            }
            if g & ARM_FLOATING_POINT != 0 {
                for (i, &v) in c.d.iter().enumerate() {
                    set(d(index(i)), u128::from(v));
                }
            }
            Arch::Arm
        }
        Context::Arm64(c) => {
            use regs::arm64::*;
            let g = groups(c.flags, CONTROL | INTEGER | ARM_FLOATING_POINT);
            if g & CONTROL != 0 {
                set(FP, u128::from(c.x[29]));
                set(LR, u128::from(c.x[30]));
                set(SP, u128::from(c.sp));
                set(PC, u128::from(c.pc));
            }
            if g & INTEGER != 0 {
                for (i, &v) in c.x[..29].iter().enumerate() {
                    set(x(index(i)), u128::from(v));
                }
            }
            if g & ARM_FLOATING_POINT != 0 {
                for (i, &v) in c.v.iter().enumerate() {
                    set(regs::arm64::v(index(i)), v);
                }
            }
            Arch::Arm64
        }
        Context::Unknown { .. } => return None,
    };
    Some((arch, out))
}

/// The groups a context's flags hold, of `all` (every group, if the flags
/// name none).
fn groups(flags: u32, all: u32) -> u32 {
    match flags & all {
        0 => all,
        g => g,
    }
}

/// A register's index, as a register number offset (a context has fewer
/// than 64 of any kind).
fn index(i: usize) -> u16 {
    u16::try_from(i).unwrap_or(u16::MAX)
}

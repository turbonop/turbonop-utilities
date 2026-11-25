//! Calling conventions, as a debugger needs them: which registers a call
//! preserves (so a caller's value survives in its callee's frame, or is
//! lost), where arguments, results, and return addresses are, and what
//! lies below the stack pointer (a red zone that is live, or nothing that
//! is).
//!
//! An [`Abi`] is an architecture's convention on an operating system:
//!
//! | | Windows | Linux | Android | macOS, iOS |
//! |---|---|---|---|---|
//! | x64 | Windows x64 | System V | System V | System V |
//! | x86 | Windows x86 | i386 System V | i386 System V | i386 System V |
//! | ARM64 | Windows ARM64 | AAPCS64 | AAPCS64 (x18 reserved) | Apple ARM64 |
//! | ARM | Windows ARM | AAPCS (hard float) | AAPCS (soft float) | Apple ARM (r9 volatile) |
//!
//! Registers are by DWARF number ([`regs`]). A register this doesn't track
//! (an x87 or MMX register, a control register) is never [`Preserved`]:
//! a debugger that keeps a value across a call only when the convention
//! says it survives can't be wrong about it.
//!
//! Not covered: ARM64EC (x64's convention on ARM64 registers), the
//! x86-64 x32 ABI, and soft-float ARM Linux (armel).

#![no_std]

pub mod regs;

use turbonop_core::Arch;

/// An operating system, as its conventions differ.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Os {
    Windows,
    Linux,
    Android,
    /// macOS.
    MacOs,
    /// iOS, and the systems that share its conventions (iPadOS, tvOS,
    /// watchOS, visionOS).
    Ios,
}

impl Os {
    /// Whether it's one of Apple's.
    pub fn is_darwin(self) -> bool {
        matches!(self, Os::MacOs | Os::Ios)
    }
}

/// A calling convention.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Convention {
    /// Windows x64 (Microsoft's x64 calling convention).
    WindowsX64,
    /// The System V AMD64 psABI (Linux, Android, macOS).
    SysVX64,
    /// Windows x86 (`__cdecl`, `__stdcall`, `__fastcall`, `__thiscall`,
    /// `__vectorcall`: see [`X86Call`]).
    WindowsX86,
    /// The System V i386 psABI (Linux, Android, macOS).
    SysVI386,
    /// Windows ARM64.
    WindowsArm64,
    /// The Arm 64-bit Procedure Call Standard (Linux, Android).
    Aapcs64,
    /// Apple's ARM64 convention (AAPCS64 with Apple's changes).
    AppleArm64,
    /// Windows ARM (Thumb-2).
    WindowsArm,
    /// The Arm Procedure Call Standard (Linux, Android).
    Aapcs32,
    /// Apple's 32-bit ARM convention (iOS's armv7).
    AppleArm32,
}

/// How much of a register's value a call preserves.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preserved {
    /// All of it: the callee restores it (or never changes it) before it
    /// returns.
    Whole,
    /// Its low 64 bits only (AAPCS64's V8..V15: their D halves).
    Low64,
    /// It's the same in every frame of a thread (a segment register, a
    /// thread pointer the system keeps in a register): no code changes it.
    Constant,
    /// None of it: a call may change it.
    Not,
}

impl Preserved {
    /// Whether any of the caller's value survives a call.
    pub fn survives(self) -> bool {
        self != Preserved::Not
    }
}

/// Where a call leaves its return address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReturnAddress {
    /// Pushed on the stack (at the stack pointer, on entry).
    Stack,
    /// In this register (a link register).
    Register(u16),
}

/// The 32-bit x86 conventions a function can be declared with (Windows
/// names them; System V i386 code is `Cdecl` unless declared otherwise,
/// with GCC's attributes of the same names).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum X86Call {
    /// Arguments on the stack; the caller pops them.
    Cdecl,
    /// Arguments on the stack; the callee pops them.
    Stdcall,
    /// The first two integer arguments in ECX and EDX; the callee pops the
    /// rest.
    Fastcall,
    /// MSVC's: `this` in ECX; the callee pops the rest. (GCC's member
    /// functions are `Cdecl`, with `this` first on the stack.)
    Thiscall,
    /// The first two integer arguments in ECX and EDX, vectors in XMM0..XMM5.
    Vectorcall,
}

impl X86Call {
    /// The registers integer arguments go in, in order.
    pub fn argument_registers(self) -> &'static [u16] {
        use regs::x86::*;
        match self {
            X86Call::Cdecl | X86Call::Stdcall => &[],
            X86Call::Fastcall | X86Call::Vectorcall => &[ECX, EDX],
            X86Call::Thiscall => &[ECX],
        }
    }

    /// Whether the callee pops its stack arguments.
    pub fn callee_pops(self) -> bool {
        !matches!(self, X86Call::Cdecl)
    }
}

/// An architecture's calling convention on an operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Abi {
    arch: Arch,
    os: Os,
    convention: Convention,
}

/// RAX..R15, RIP; XMM0..XMM15; XMM16..XMM31; RFLAGS; ES..GS; FS and GS bases; MXCSR.
const X64_REGISTERS: &[u16] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 49,
    50, 51, 52, 53, 54, 55, 58, 59, 64,
];

/// EAX..EDI, EIP, EFLAGS; XMM0..XMM7; ES..GS.
const X86_REGISTERS: &[u16] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 21, 22, 23, 24, 25, 26, 27, 28, 40, 41, 42, 43, 44, 45,
];

/// X0..X30, SP, PC; V0..V31.
const ARM64_REGISTERS: &[u16] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25,
    26, 27, 28, 29, 30, 31, 32, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80,
    81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95,
];

/// R0..R15; D0..D31.
const ARM_REGISTERS: &[u16] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 256, 257, 258, 259, 260, 261, 262, 263,
    264, 265, 266, 267, 268, 269, 270, 271, 272, 273, 274, 275, 276, 277, 278, 279, 280, 281, 282,
    283, 284, 285, 286, 287,
];

impl Abi {
    /// The convention of `arch` on `os`, if it's one described here (x64,
    /// x86, ARM64, ARM).
    pub fn new(arch: Arch, os: Os) -> Option<Abi> {
        let convention = match (arch, os) {
            (Arch::X64, Os::Windows) => Convention::WindowsX64,
            (Arch::X64, _) => Convention::SysVX64,
            (Arch::X86, Os::Windows) => Convention::WindowsX86,
            (Arch::X86, _) => Convention::SysVI386,
            (Arch::Arm64, Os::Windows) => Convention::WindowsArm64,
            (Arch::Arm64, Os::MacOs | Os::Ios) => Convention::AppleArm64,
            (Arch::Arm64, Os::Linux | Os::Android) => Convention::Aapcs64,
            (Arch::Arm, Os::Windows) => Convention::WindowsArm,
            (Arch::Arm, Os::MacOs | Os::Ios) => Convention::AppleArm32,
            (Arch::Arm, Os::Linux | Os::Android) => Convention::Aapcs32,
            _ => return None,
        };
        Some(Abi {
            arch,
            os,
            convention,
        })
    }

    pub fn arch(self) -> Arch {
        self.arch
    }

    pub fn os(self) -> Os {
        self.os
    }

    pub fn convention(self) -> Convention {
        self.convention
    }

    /// Every register this tracks (general, vector, flags, and the
    /// segment and thread registers that never change), in number order
    /// within each group.
    pub fn registers(self) -> &'static [u16] {
        match self.arch {
            Arch::X64 => X64_REGISTERS,
            Arch::X86 => X86_REGISTERS,
            Arch::Arm64 => ARM64_REGISTERS,
            _ => ARM_REGISTERS,
        }
    }

    /// How much of `register`'s value a call preserves: the caller's
    /// value, as it is in the callee's frame (or in the caller's, once
    /// the callee returns). The stack pointer is [`Preserved::Whole`] (a
    /// callee leaves it as it found it, once its return address and any
    /// arguments it pops are accounted for); the PC is [`Preserved::Not`]
    /// (an unwinder recovers the caller's from the return address).
    pub fn preserved(self, register: u16) -> Preserved {
        use Preserved::*;
        match self.convention {
            Convention::WindowsX64 | Convention::SysVX64 => {
                use regs::x64::*;
                match register {
                    RBX | RBP | RSP | R12..=R15 => Whole,
                    // Windows: RSI, RDI, and XMM6..XMM15 (all 128 bits;
                    // YMM's and ZMM's upper halves are volatile, and
                    // aren't numbered apart).
                    RSI | RDI if self.convention == Convention::WindowsX64 => Whole,
                    23..=32 if self.convention == Convention::WindowsX64 => Whole,
                    ES..=GS | FS_BASE | GS_BASE => Constant,
                    _ => Not,
                }
            }
            Convention::WindowsX86 | Convention::SysVI386 => {
                use regs::x86::*;
                match register {
                    EBX | ESP | EBP | ESI | EDI => Whole,
                    ES..=GS => Constant,
                    _ => Not,
                }
            }
            Convention::WindowsArm64 | Convention::Aapcs64 | Convention::AppleArm64 => {
                use regs::arm64::*;
                match register {
                    X19..=X28 | FP | SP => Whole,
                    // V8..V15's D halves.
                    72..=79 => Low64,
                    X18 => match self.os {
                        // The TEB, which only the system writes.
                        Os::Windows => Constant,
                        // The shadow call stack pointer: each function
                        // that pushes to it pops before it returns.
                        Os::Android => Whole,
                        // Apple reserves it, and the kernel may zero it at
                        // any time; Linux code uses it as a temporary.
                        _ => Not,
                    },
                    _ => Not,
                }
            }
            Convention::WindowsArm | Convention::Aapcs32 | Convention::AppleArm32 => {
                use regs::arm::*;
                match register {
                    // iOS made R9 volatile (since iOS 3).
                    R9 if self.convention == Convention::AppleArm32 => Not,
                    4..=11 | SP => Whole,
                    // D8..D15.
                    264..=271 => Whole,
                    _ => Not,
                }
            }
        }
    }

    /// Whether a call may change `register` (any of it).
    pub fn is_volatile(self, register: u16) -> bool {
        self.preserved(register) != Preserved::Whole
            && self.preserved(register) != Preserved::Constant
    }

    /// The registers integer and pointer arguments go in, in order. For
    /// 32-bit x86, none (the stack): a function declared otherwise has
    /// [`X86Call::argument_registers`].
    pub fn argument_registers(self) -> &'static [u16] {
        match self.convention {
            Convention::WindowsX64 => {
                use regs::x64::*;
                &[RCX, RDX, R8, R9]
            }
            Convention::SysVX64 => {
                use regs::x64::*;
                &[RDI, RSI, RDX, RCX, R8, R9]
            }
            Convention::WindowsX86 | Convention::SysVI386 => &[],
            Convention::WindowsArm64 | Convention::Aapcs64 | Convention::AppleArm64 => {
                &[0, 1, 2, 3, 4, 5, 6, 7]
            }
            Convention::WindowsArm | Convention::Aapcs32 | Convention::AppleArm32 => &[0, 1, 2, 3],
        }
    }

    /// The registers floating-point and vector arguments go in, in order
    /// (none where they go in the integer registers, as with soft-float
    /// ARM: Android's and iOS's).
    pub fn vector_argument_registers(self) -> &'static [u16] {
        match self.convention {
            Convention::WindowsX64 => &[17, 18, 19, 20],
            Convention::SysVX64 => &[17, 18, 19, 20, 21, 22, 23, 24],
            Convention::WindowsX86 | Convention::SysVI386 => &[],
            Convention::WindowsArm64 | Convention::Aapcs64 | Convention::AppleArm64 => {
                &[64, 65, 66, 67, 68, 69, 70, 71]
            }
            Convention::WindowsArm => &[256, 257, 258, 259, 260, 261, 262, 263],
            Convention::Aapcs32 if self.os == Os::Linux => {
                &[256, 257, 258, 259, 260, 261, 262, 263]
            }
            Convention::Aapcs32 | Convention::AppleArm32 => &[],
        }
    }

    /// The registers an integer or pointer result comes back in (the
    /// second for a result twice a register's width).
    pub fn return_registers(self) -> &'static [u16] {
        match self.convention {
            Convention::WindowsX64 => &[regs::x64::RAX],
            Convention::SysVX64 => &[regs::x64::RAX, regs::x64::RDX],
            Convention::WindowsX86 | Convention::SysVI386 => &[regs::x86::EAX, regs::x86::EDX],
            Convention::WindowsArm64 | Convention::Aapcs64 | Convention::AppleArm64 => &[0, 1],
            Convention::WindowsArm | Convention::Aapcs32 | Convention::AppleArm32 => &[0, 1],
        }
    }

    /// The register a structure returned by address has its address in,
    /// where the convention gives it one apart from the arguments
    /// (AAPCS64's X8).
    pub fn indirect_result_register(self) -> Option<u16> {
        match self.arch {
            Arch::Arm64 => Some(regs::arm64::X8),
            _ => None,
        }
    }

    /// The stack pointer.
    pub fn stack_pointer(self) -> u16 {
        match self.arch {
            Arch::X64 => regs::x64::RSP,
            Arch::X86 => regs::x86::ESP,
            Arch::Arm64 => regs::arm64::SP,
            _ => regs::arm::SP,
        }
    }

    /// The program counter.
    pub fn program_counter(self) -> u16 {
        match self.arch {
            Arch::X64 => regs::x64::RIP,
            Arch::X86 => regs::x86::EIP,
            Arch::Arm64 => regs::arm64::PC,
            _ => regs::arm::PC,
        }
    }

    /// The register a frame that keeps a frame pointer keeps it in (x64:
    /// RBP, by convention; Windows' unwind data names any register), or
    /// None where it depends on the code (32-bit ARM Linux: R11 in A32
    /// code, R7 in Thumb).
    pub fn frame_pointer(self) -> Option<u16> {
        match self.convention {
            Convention::WindowsX64 | Convention::SysVX64 => Some(regs::x64::RBP),
            Convention::WindowsX86 | Convention::SysVI386 => Some(regs::x86::EBP),
            Convention::WindowsArm64 | Convention::Aapcs64 | Convention::AppleArm64 => {
                Some(regs::arm64::FP)
            }
            Convention::WindowsArm => Some(11),
            Convention::AppleArm32 => Some(7),
            Convention::Aapcs32 => None,
        }
    }

    /// Whether every function keeps a frame record (the caller's frame
    /// pointer and return address, chained) in the frame pointer: Apple's
    /// ARM conventions require one; elsewhere it's the compiler's choice.
    pub fn frame_records_required(self) -> bool {
        matches!(
            self.convention,
            Convention::AppleArm64 | Convention::AppleArm32
        )
    }

    /// Where a call leaves its return address.
    pub fn return_address(self) -> ReturnAddress {
        match self.arch {
            Arch::X64 | Arch::X86 => ReturnAddress::Stack,
            Arch::Arm64 => ReturnAddress::Register(regs::arm64::LR),
            _ => ReturnAddress::Register(regs::arm::LR),
        }
    }

    /// The bytes below the stack pointer a function may use without moving
    /// it, which a signal handler or debugger must not take as free: what
    /// is there may be live. (Windows has none: anything below the stack
    /// pointer may be overwritten at any time, so it's dead.)
    pub fn red_zone(self) -> u64 {
        match self.convention {
            Convention::SysVX64 | Convention::AppleArm64 => 128,
            _ => 0,
        }
    }

    /// The bytes a caller reserves above the return address for the
    /// callee to save its register arguments in (Windows x64's home
    /// space, or shadow space).
    pub fn home_space(self) -> u64 {
        match self.convention {
            Convention::WindowsX64 => 32,
            _ => 0,
        }
    }

    /// The stack pointer's alignment at a call, in bytes.
    pub fn stack_alignment(self) -> u64 {
        match self.convention {
            Convention::WindowsX86 => 4,
            Convention::WindowsArm | Convention::Aapcs32 | Convention::AppleArm32 => 8,
            _ => 16,
        }
    }

    /// A code address (a return address, a function pointer) as it would
    /// be with any pointer authentication code and tag stripped, given how
    /// many bits the process's virtual addresses have (which the dump or
    /// the system says: Linux's `NT_ARM_PAC_MASK`, a Mach-O core's
    /// `addrable bits`). ARM64's user addresses have bit 55 clear, and
    /// everything above `address_bits` is the PAC or a tag; kernel
    /// addresses have it set, and everything above is ones. Other
    /// architectures' addresses are returned unchanged.
    pub fn strip_code_address(self, address: u64, address_bits: u8) -> u64 {
        if self.arch != Arch::Arm64 || !(1..64).contains(&address_bits) {
            return address;
        }
        let mask = (1u64 << address_bits) - 1;
        if address & (1 << 55) == 0 {
            address & mask
        } else {
            address | !mask
        }
    }
}

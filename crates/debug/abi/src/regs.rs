//! Register numbers: DWARF's, as each architecture's ABI assigns them,
//! which every TurboNOP debugging crate uses for every architecture
//! (whatever a format or unwinder numbers them by: CodeView's numbers, a
//! minidump context's fields, Windows' unwind codes).
//!
//! `turbonop_dwarf::registers::name` names them.

/// x86-64 (the System V psABI's table, which Windows' x64 uses too here).
pub mod x64 {
    pub const RAX: u16 = 0;
    pub const RDX: u16 = 1;
    pub const RCX: u16 = 2;
    pub const RBX: u16 = 3;
    pub const RSI: u16 = 4;
    pub const RDI: u16 = 5;
    pub const RBP: u16 = 6;
    pub const RSP: u16 = 7;
    pub const R8: u16 = 8;
    pub const R9: u16 = 9;
    pub const R10: u16 = 10;
    pub const R11: u16 = 11;
    pub const R12: u16 = 12;
    pub const R13: u16 = 13;
    pub const R14: u16 = 14;
    pub const R15: u16 = 15;
    /// The return address column, and the PC.
    pub const RIP: u16 = 16;
    /// XMM0 (XMM0..XMM15 are 17..32).
    pub const XMM0: u16 = 17;
    pub const RFLAGS: u16 = 49;
    /// ES (ES, CS, SS, DS, FS, GS are 50..55).
    pub const ES: u16 = 50;
    pub const CS: u16 = 51;
    pub const SS: u16 = 52;
    pub const DS: u16 = 53;
    pub const FS: u16 = 54;
    pub const GS: u16 = 55;
    pub const FS_BASE: u16 = 58;
    pub const GS_BASE: u16 = 59;
    pub const MXCSR: u16 = 64;
    /// XMM16 (XMM16..XMM31 are 67..82).
    pub const XMM16: u16 = 67;

    /// The general registers in their encoding's order (RAX, RCX, RDX,
    /// RBX, RSP, RBP, RSI, RDI, R8..R15: Windows' `CONTEXT` and unwind
    /// codes, and the instruction encoding), by number.
    pub const BY_ENCODING: [u16; 16] = [
        RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8, R9, R10, R11, R12, R13, R14, R15,
    ];

    /// XMM`n`.
    pub const fn xmm(n: u16) -> u16 {
        if n < 16 { XMM0 + n } else { XMM16 + (n - 16) }
    }
}

/// 32-bit x86 (the i386 psABI's table; Darwin swaps ESP and EBP in its
/// `.eh_frame`, but not in its DWARF).
pub mod x86 {
    pub const EAX: u16 = 0;
    pub const ECX: u16 = 1;
    pub const EDX: u16 = 2;
    pub const EBX: u16 = 3;
    pub const ESP: u16 = 4;
    pub const EBP: u16 = 5;
    pub const ESI: u16 = 6;
    pub const EDI: u16 = 7;
    /// The return address column, and the PC.
    pub const EIP: u16 = 8;
    pub const EFLAGS: u16 = 9;
    /// XMM0 (XMM0..XMM7 are 21..28).
    pub const XMM0: u16 = 21;
    /// ES (ES, CS, SS, DS, FS, GS are 40..45).
    pub const ES: u16 = 40;
    pub const CS: u16 = 41;
    pub const SS: u16 = 42;
    pub const DS: u16 = 43;
    pub const FS: u16 = 44;
    pub const GS: u16 = 45;
}

/// AArch64 (the Arm ABI's DWARF for the Arm 64-bit Architecture).
pub mod arm64 {
    /// X`n` is `n` (X0..X30).
    pub const X0: u16 = 0;
    /// The indirect result register.
    pub const X8: u16 = 8;
    /// IP0 and IP1, the intra-procedure-call scratch registers.
    pub const X16: u16 = 16;
    pub const X17: u16 = 17;
    /// The platform register.
    pub const X18: u16 = 18;
    pub const X19: u16 = 19;
    pub const X28: u16 = 28;
    /// The frame pointer.
    pub const FP: u16 = 29;
    /// The link register, and the return address column.
    pub const LR: u16 = 30;
    pub const SP: u16 = 31;
    pub const PC: u16 = 32;
    /// V0 (V0..V31 are 64..95).
    pub const V0: u16 = 64;

    /// X`n`.
    pub const fn x(n: u16) -> u16 {
        X0 + n
    }

    /// V`n`.
    pub const fn v(n: u16) -> u16 {
        V0 + n
    }
}

/// 32-bit ARM (the Arm ABI's DWARF for the Arm Architecture).
pub mod arm {
    /// R`n` is `n` (R0..R15).
    pub const R0: u16 = 0;
    /// The platform register.
    pub const R9: u16 = 9;
    /// The intra-procedure-call scratch register.
    pub const IP: u16 = 12;
    pub const SP: u16 = 13;
    /// The link register, and the return address column.
    pub const LR: u16 = 14;
    pub const PC: u16 = 15;
    /// D0 (D0..D31 are 256..287).
    pub const D0: u16 = 256;

    /// R`n`.
    pub const fn r(n: u16) -> u16 {
        R0 + n
    }

    /// D`n`.
    pub const fn d(n: u16) -> u16 {
        D0 + n
    }
}

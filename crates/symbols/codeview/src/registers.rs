//! CodeView's register numbers (`CV_REG_*`, `CV_AMD64_*`, `CV_ARM64_*`,
//! as cvconst.h has them), named as the disassemblers write them.

/// The instruction sets (CodeView numbers registers of x86, x64, and
/// ARM64; others have none here).
pub use turbonop_core::Arch;

const X86_LOW: [&str; 24] = [
    "al", "cl", "dl", "bl", "ah", "ch", "dh", "bh", "ax", "cx", "dx", "bx", "sp", "bp", "si", "di",
    "eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi",
];

const X64_QWORD: [&str; 16] = [
    "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "r8", "r9", "r10", "r11", "r12", "r13",
    "r14", "r15",
];

/// Register `n`'s name on `machine`, if it is one of those read here.
pub fn name(machine: Arch, n: u16) -> Option<String> {
    let s = |x: &str| Some(x.to_string());
    match machine {
        Arch::X86 | Arch::X64 => match n {
            1..=24 => s(X86_LOW[usize::from(n - 1)]),
            25..=30 => s(["es", "cs", "ss", "ds", "fs", "gs"][usize::from(n - 25)]),
            31 => s("ip"),
            32 => s("flags"),
            33 => s(if machine == Arch::X64 { "rip" } else { "eip" }),
            34 => s("eflags"),
            128..=135 => Some(format!("st({})", n - 128)),
            211 => s("mxcsr"),
            154..=161 => Some(format!("xmm{}", n - 154)),
            252..=259 if machine == Arch::X64 => Some(format!("xmm{}", n - 252 + 8)),
            324 => s("sil"),
            325 => s("dil"),
            326 => s("bpl"),
            327 => s("spl"),
            328..=343 if machine == Arch::X64 => s(X64_QWORD[usize::from(n - 328)]),
            344..=351 => Some(format!("r{}b", n - 344 + 8)),
            352..=359 => Some(format!("r{}w", n - 352 + 8)),
            360..=367 => Some(format!("r{}d", n - 360 + 8)),
            368..=383 => Some(format!("ymm{}", n - 368)),
            694..=709 if machine == Arch::X64 => Some(format!("xmm{}", n - 694 + 16)),
            _ => None,
        },
        Arch::Arm64 => match n {
            10..=40 => Some(format!("w{}", n - 10)),
            41 => s("wzr"),
            50..=78 => Some(match n - 50 {
                16 => "xip0".to_string(),
                17 => "xip1".to_string(),
                r => format!("x{r}"),
            }),
            79 => s("fp"),
            80 => s("lr"),
            81 => s("sp"),
            82 => s("xzr"),
            83 => s("pc"),
            90 => s("nzcv"),
            91 => s("cpsr"),
            100..=131 => Some(format!("s{}", n - 100)),
            140..=171 => Some(format!("d{}", n - 140)),
            180..=211 => Some(format!("q{}", n - 180)),
            230..=261 => Some(format!("b{}", n - 230)),
            270..=301 => Some(format!("h{}", n - 270)),
            310..=341 => Some(format!("v{}", n - 310)),
            _ => None,
        },
        Arch::Arm => match n {
            10..=22 => Some(format!("r{}", n - 10)),
            23 => s("sp"),
            24 => s("lr"),
            25 => s("pc"),
            26 => s("cpsr"),
            40 => s("fpscr"),
            50..=81 => Some(format!("s{}", n - 50)),
            300..=331 => Some(format!("d{}", n - 300)),
            400..=415 => Some(format!("q{}", n - 400)),
            _ => None,
        },
        _ => None,
    }
}

/// Part of a register, by DWARF number: bits `lsb..lsb + bits` of DWARF
/// register `register` (the numbers `turbonop-abi`'s `regs` and the
/// unwinders use).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DwarfPart {
    pub register: u16,
    pub lsb: u16,
    pub bits: u16,
}

impl DwarfPart {
    const fn new(register: u16, lsb: u16, bits: u16) -> Option<DwarfPart> {
        Some(DwarfPart {
            register,
            lsb,
            bits,
        })
    }
}

/// x64's DWARF numbers for its general registers by encoding (RAX, RCX,
/// RDX, RBX, RSP, RBP, RSI, RDI, R8..R15).
const X64_DWARF_BY_ENCODING: [u16; 16] = [0, 2, 1, 3, 7, 6, 4, 5, 8, 9, 10, 11, 12, 13, 14, 15];

/// x64's DWARF numbers for CodeView's 64-bit registers (`CV_AMD64_RAX`..
/// `CV_AMD64_R15`: RAX, RBX, RCX, RDX, RSI, RDI, RBP, RSP, R8..R15).
const X64_DWARF_BY_CODEVIEW: [u16; 16] = [0, 3, 2, 1, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// CodeView register `n` of `machine` as the part of a DWARF register it
/// is (`eax`: RAX's low 32 bits; `ah`: bits 8 to 15; ARM's `s3`: the high
/// half of D1), for x86, x64, ARM64, and ARM. None for a register DWARF
/// doesn't number (the zero registers, ARM's CPSR, ARM64's NZCV) or one
/// not read here. ARM's Q registers span two D registers: Q`k` is the
/// 128 bits from D`2k`.
pub fn to_dwarf(machine: Arch, n: u16) -> Option<DwarfPart> {
    let x64 = machine == Arch::X64;
    // The general registers' DWARF numbers by encoding: x86's are its
    // encoding.
    let gpr = |e: u16| {
        if x64 {
            X64_DWARF_BY_ENCODING[usize::from(e)]
        } else {
            e
        }
    };
    match machine {
        Arch::X86 | Arch::X64 => match n {
            // AL, CL, DL, BL; AH, CH, DH, BH.
            1..=4 => DwarfPart::new(gpr(n - 1), 0, 8),
            5..=8 => DwarfPart::new(gpr(n - 5), 8, 8),
            // AX..DI; EAX..EDI.
            9..=16 => DwarfPart::new(gpr(n - 9), 0, 16),
            17..=24 => DwarfPart::new(gpr(n - 17), 0, 32),
            // ES, CS, SS, DS, FS, GS.
            25..=30 => DwarfPart::new(if x64 { 50 } else { 40 } + (n - 25), 0, 16),
            // IP, FLAGS; EIP (x64: RIP), EFLAGS.
            31 => DwarfPart::new(if x64 { 16 } else { 8 }, 0, 16),
            32 => DwarfPart::new(if x64 { 49 } else { 9 }, 0, 16),
            33 => DwarfPart::new(if x64 { 16 } else { 8 }, 0, if x64 { 64 } else { 32 }),
            34 => DwarfPart::new(if x64 { 49 } else { 9 }, 0, 32),
            128..=135 => DwarfPart::new(if x64 { 33 } else { 11 } + (n - 128), 0, 80),
            154..=161 => DwarfPart::new(if x64 { 17 } else { 21 } + (n - 154), 0, 128),
            211 => DwarfPart::new(if x64 { 64 } else { 39 }, 0, 32),
            252..=259 if x64 => DwarfPart::new(17 + 8 + (n - 252), 0, 128),
            // SIL, DIL, BPL, SPL.
            324..=327 if x64 => DwarfPart::new([4, 5, 6, 7][usize::from(n - 324)], 0, 8),
            328..=343 if x64 => DwarfPart::new(X64_DWARF_BY_CODEVIEW[usize::from(n - 328)], 0, 64),
            // R8B..R15B, R8W..R15W, R8D..R15D.
            344..=351 if x64 => DwarfPart::new(8 + (n - 344), 0, 8),
            352..=359 if x64 => DwarfPart::new(8 + (n - 352), 0, 16),
            360..=367 if x64 => DwarfPart::new(8 + (n - 360), 0, 32),
            // YMM0..YMM15: the XMM register, all 256 bits.
            368..=383 if x64 => DwarfPart::new(17 + (n - 368), 0, 256),
            694..=709 if x64 => DwarfPart::new(67 + (n - 694), 0, 128),
            _ => None,
        },
        Arch::Arm64 => match n {
            10..=40 => DwarfPart::new(n - 10, 0, 32),
            50..=83 => match n - 50 {
                // X0..X28, FP, LR, SP; then XZR (none), PC.
                r @ 0..=31 => DwarfPart::new(r, 0, 64),
                33 => DwarfPart::new(32, 0, 64),
                _ => None,
            },
            100..=131 => DwarfPart::new(64 + (n - 100), 0, 32),
            140..=171 => DwarfPart::new(64 + (n - 140), 0, 64),
            180..=211 => DwarfPart::new(64 + (n - 180), 0, 128),
            230..=261 => DwarfPart::new(64 + (n - 230), 0, 8),
            270..=301 => DwarfPart::new(64 + (n - 270), 0, 16),
            310..=341 => DwarfPart::new(64 + (n - 310), 0, 128),
            _ => None,
        },
        Arch::Arm => match n {
            // R0..R12, SP, LR, PC.
            10..=25 => DwarfPart::new(n - 10, 0, 32),
            // S0..S31: halves of D0..D15.
            50..=81 => DwarfPart::new(256 + (n - 50) / 2, 32 * ((n - 50) % 2), 32),
            300..=331 => DwarfPart::new(256 + (n - 300), 0, 64),
            400..=415 => DwarfPart::new(256 + 2 * (n - 400), 0, 128),
            _ => None,
        },
        _ => None,
    }
}

/// The register an S_FRAMEPROC frame pointer encoding names (1 the stack
/// pointer, 2 the frame pointer, 3 the base pointer).
pub fn frame_pointer(machine: Arch, encoded: u8) -> Option<&'static str> {
    Some(match (machine, encoded) {
        (Arch::X86, 1) => "esp",
        (Arch::X86, 2) => "ebp",
        (Arch::X86, 3) => "ebx",
        (Arch::X64, 1) => "rsp",
        (Arch::X64, 2) => "rbp",
        (Arch::X64, 3) => "r13",
        (Arch::Arm64, 1) => "sp",
        (Arch::Arm64, 2) => "fp",
        (Arch::Arm64, 3) => "x19",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(name(Arch::X64, 330).as_deref(), Some("rcx"));
        assert_eq!(name(Arch::X64, 18).as_deref(), Some("ecx"));
        assert_eq!(name(Arch::X64, 336).as_deref(), Some("r8"));
        assert_eq!(name(Arch::X64, 367).as_deref(), Some("r15d"));
        assert_eq!(name(Arch::X64, 154).as_deref(), Some("xmm0"));
        assert_eq!(name(Arch::X64, 259).as_deref(), Some("xmm15"));
        assert_eq!(name(Arch::X86, 22).as_deref(), Some("ebp"));
        assert_eq!(name(Arch::X86, 330), None);
        assert_eq!(name(Arch::Arm64, 50).as_deref(), Some("x0"));
        assert_eq!(name(Arch::Arm64, 10).as_deref(), Some("w0"));
        assert_eq!(name(Arch::Arm64, 66).as_deref(), Some("xip0"));
        assert_eq!(name(Arch::Arm64, 79).as_deref(), Some("fp"));
        assert_eq!(name(Arch::Arm64, 141).as_deref(), Some("d1"));
        assert_eq!(name(Arch::Arm64, 0), None);
        assert_eq!(frame_pointer(Arch::X64, 1), Some("rsp"));
        assert_eq!(frame_pointer(Arch::Arm64, 2), Some("fp"));
        assert_eq!(frame_pointer(Arch::X64, 0), None);
        assert_eq!(name(Arch::Arm64, 83).as_deref(), Some("pc"));
        assert_eq!(name(Arch::Arm64, 341).as_deref(), Some("v31"));
        assert_eq!(name(Arch::Arm, 21).as_deref(), Some("r11"));
        assert_eq!(name(Arch::Arm, 23).as_deref(), Some("sp"));
        assert_eq!(name(Arch::Arm, 81).as_deref(), Some("s31"));
        assert_eq!(name(Arch::Arm, 331).as_deref(), Some("d31"));
        assert_eq!(name(Arch::Arm, 415).as_deref(), Some("q15"));
    }

    fn part(register: u16, lsb: u16, bits: u16) -> Option<DwarfPart> {
        Some(DwarfPart {
            register,
            lsb,
            bits,
        })
    }

    /// Spot checks, by cvconst.h's numbers.
    #[test]
    fn to_dwarf_numbers() {
        let x86 = |n| to_dwarf(Arch::X86, n);
        let x64 = |n| to_dwarf(Arch::X64, n);
        // CV_REG_AL, CV_REG_BH, CV_REG_SI, CV_REG_EBP, CV_REG_EIP.
        assert_eq!(x86(1), part(0, 0, 8));
        assert_eq!(x86(8), part(3, 8, 8));
        assert_eq!(x86(15), part(6, 0, 16));
        assert_eq!(x86(22), part(5, 0, 32));
        assert_eq!(x86(33), part(8, 0, 32));
        assert_eq!(x86(34), part(9, 0, 32));
        assert_eq!(x86(26), part(41, 0, 16)); // CS
        assert_eq!(x86(128), part(11, 0, 80)); // ST0
        assert_eq!(x86(161), part(28, 0, 128)); // XMM7
        assert_eq!(x86(211), part(39, 0, 32)); // MXCSR
        assert_eq!(x86(330), None);
        // CV_AMD64_RAX, _RBX, _RCX, _RDX, _RSI, _RDI, _RBP, _RSP.
        let quads: Vec<_> = (328..336).map(|n| x64(n).unwrap().register).collect();
        assert_eq!(quads, [0, 3, 2, 1, 4, 5, 6, 7]);
        assert_eq!(x64(343), part(15, 0, 64)); // R15
        assert_eq!(x64(18), part(2, 0, 32)); // ECX
        assert_eq!(x64(21), part(7, 0, 32)); // ESP
        assert_eq!(x64(5), part(0, 8, 8)); // AH
        assert_eq!(x64(325), part(5, 0, 8)); // DIL
        assert_eq!(x64(351), part(15, 0, 8)); // R15B
        assert_eq!(x64(359), part(15, 0, 16)); // R15W
        assert_eq!(x64(360), part(8, 0, 32)); // R8D
        assert_eq!(x64(33), part(16, 0, 64)); // RIP
        assert_eq!(x64(34), part(49, 0, 32)); // EFLAGS
        assert_eq!(x64(30), part(55, 0, 16)); // GS
        assert_eq!(x64(161), part(24, 0, 128)); // XMM7
        assert_eq!(x64(252), part(25, 0, 128)); // XMM8
        assert_eq!(x64(259), part(32, 0, 128)); // XMM15
        assert_eq!(x64(383), part(32, 0, 256)); // YMM15
        assert_eq!(x64(694), part(67, 0, 128)); // XMM16
        assert_eq!(x64(709), part(82, 0, 128)); // XMM31
        assert_eq!(x64(211), part(64, 0, 32)); // MXCSR
        assert_eq!(x64(135), part(40, 0, 80)); // ST7
        let a64 = |n| to_dwarf(Arch::Arm64, n);
        assert_eq!(a64(10), part(0, 0, 32)); // W0
        assert_eq!(a64(40), part(30, 0, 32)); // W30
        assert_eq!(a64(41), None); // WZR
        assert_eq!(a64(66), part(16, 0, 64)); // IP0
        assert_eq!(a64(78), part(28, 0, 64)); // X28
        assert_eq!(a64(79), part(29, 0, 64)); // FP
        assert_eq!(a64(80), part(30, 0, 64)); // LR
        assert_eq!(a64(81), part(31, 0, 64)); // SP
        assert_eq!(a64(82), None); // ZR
        assert_eq!(a64(83), part(32, 0, 64)); // PC
        assert_eq!(a64(90), None); // NZCV
        assert_eq!(a64(131), part(95, 0, 32)); // S31
        assert_eq!(a64(140), part(64, 0, 64)); // D0
        assert_eq!(a64(211), part(95, 0, 128)); // Q31
        assert_eq!(a64(230), part(64, 0, 8)); // B0
        assert_eq!(a64(270), part(64, 0, 16)); // H0
        assert_eq!(a64(341), part(95, 0, 128)); // V31
        let arm = |n| to_dwarf(Arch::Arm, n);
        assert_eq!(arm(10), part(0, 0, 32)); // R0
        assert_eq!(arm(23), part(13, 0, 32)); // SP
        assert_eq!(arm(25), part(15, 0, 32)); // PC
        assert_eq!(arm(26), None); // CPSR
        assert_eq!(arm(50), part(256, 0, 32)); // S0
        assert_eq!(arm(51), part(256, 32, 32)); // S1
        assert_eq!(arm(81), part(271, 32, 32)); // S31
        assert_eq!(arm(316), part(272, 0, 64)); // ND16
        assert_eq!(arm(415), part(286, 0, 128)); // NQ15
        assert_eq!(to_dwarf(Arch::RiscV64, 10), None);
    }

    /// Every register mapped whole has the DWARF register's own name
    /// (where the two spell it alike), and every part fits its register.
    #[test]
    fn to_dwarf_agrees_with_dwarf_names() {
        use turbonop_dwarf::registers::name as dwarf_name;
        let width = |arch: Arch, register: u16| -> u16 {
            match (arch, register) {
                (Arch::X86, 11..=18) | (Arch::X64, 33..=40) => 80,
                (Arch::X86, 21..=28) | (Arch::X64, 17..=32 | 67..=82) => 256,
                (Arch::Arm64, 64..=95) => 128,
                // Q`k` spans two D registers.
                (Arch::Arm, 256..=287) => 128,
                (Arch::X86 | Arch::Arm, _) => 32,
                _ => 64,
            }
        };
        let mut whole = 0;
        for arch in [Arch::X86, Arch::X64, Arch::Arm64, Arch::Arm] {
            for n in 0..1000 {
                let Some(p) = to_dwarf(arch, n) else { continue };
                assert!(name(arch, n).is_some(), "{arch} {n} has no name");
                let dwarf = dwarf_name(arch, p.register)
                    .unwrap_or_else(|| panic!("{arch} {n}: DWARF {} has no name", p.register));
                assert!(
                    p.lsb + p.bits <= width(arch, p.register),
                    "{arch} {n}: {p:?}"
                );
                let cv = name(arch, n).unwrap();
                let full = p.lsb == 0 && p.bits == arch.bits().map_or(0, u16::from);
                let general = !cv.starts_with(['s', 'd', 'q', 'b', 'h', 'v', 'x', 'y']);
                if full && general {
                    assert_eq!(cv, dwarf, "{arch} {n}");
                    whole += 1;
                }
                if cv.starts_with("xmm") && p.bits == 128 {
                    assert_eq!(cv, dwarf, "{arch} {n}");
                }
                if arch == Arch::Arm64 && cv.starts_with('x') && cv.len() <= 3 {
                    assert_eq!(cv, dwarf, "{arch} {n}");
                    whole += 1;
                }
            }
        }
        assert!(whole > 40, "{whole}");
    }
}

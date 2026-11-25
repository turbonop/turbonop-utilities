//! The integration tests, as one binary (one link, not one per file).

use turbonop_abi::{Abi, Convention, Os, Preserved, ReturnAddress, X86Call, regs};
use turbonop_core::Arch;

const OSES: [Os; 5] = [Os::Windows, Os::Linux, Os::Android, Os::MacOs, Os::Ios];
const ARCHES: [Arch; 4] = [Arch::X64, Arch::X86, Arch::Arm64, Arch::Arm];

fn abi(arch: Arch, os: Os) -> Abi {
    Abi::new(arch, os).unwrap()
}

/// The registers `abi` preserves, as `kind`, in number order.
fn preserved(abi: Abi, kind: Preserved) -> Vec<u16> {
    abi.registers()
        .iter()
        .copied()
        .filter(|&r| abi.preserved(r) == kind)
        .collect()
}

#[test]
fn conventions() {
    use Convention::*;
    let expect = [
        (Arch::X64, [WindowsX64, SysVX64, SysVX64, SysVX64, SysVX64]),
        (
            Arch::X86,
            [WindowsX86, SysVI386, SysVI386, SysVI386, SysVI386],
        ),
        (
            Arch::Arm64,
            [WindowsArm64, Aapcs64, Aapcs64, AppleArm64, AppleArm64],
        ),
        (
            Arch::Arm,
            [WindowsArm, Aapcs32, Aapcs32, AppleArm32, AppleArm32],
        ),
    ];
    for (arch, conventions) in expect {
        for (os, c) in OSES.iter().zip(conventions) {
            assert_eq!(abi(arch, *os).convention(), c, "{arch} {os:?}");
        }
    }
    assert_eq!(Abi::new(Arch::RiscV64, Os::Linux), None);
    assert_eq!(Abi::new(Arch::Wasm32, Os::Windows), None);
}

// Each convention's preserved registers, from its specification:
//
// - Windows x64: "x64 ABI conventions", register volatility.
// - System V x86-64: the AMD64 psABI, figure 3.4.
// - i386: the i386 psABI, and Microsoft's "Argument passing and naming
//   conventions" (the same callee-saved four, and the stack pointer).
// - AAPCS64: section 6.1.1 (X19..X28, FP, SP; the low 64 bits of V8..V15),
//   and Windows', Apple's, and Android's documented uses of X18.
// - AAPCS: section 5.1.1 (R4..R11, SP; D8..D15), and Apple's "iOS ARMv7
//   function calling conventions" (R9 volatile).

#[test]
fn windows_x64() {
    let a = abi(Arch::X64, Os::Windows);
    use regs::x64::*;
    let mut whole = vec![RBX, RSI, RDI, RBP, RSP, R12, R13, R14, R15];
    whole.extend((6..16).map(xmm));
    whole.sort_unstable();
    assert_eq!(preserved(a, Preserved::Whole), whole);
    assert_eq!(
        preserved(a, Preserved::Constant),
        [ES, CS, SS, DS, FS, GS, FS_BASE, GS_BASE]
    );
    assert!(preserved(a, Preserved::Low64).is_empty());
    for r in [RAX, RCX, RDX, R8, R9, R10, R11, RIP, RFLAGS, MXCSR] {
        assert!(a.is_volatile(r), "{r}");
    }
    for n in (0..6).chain(16..32) {
        assert!(a.is_volatile(xmm(n)), "xmm{n}");
    }
    assert_eq!(a.argument_registers(), [RCX, RDX, R8, R9]);
    assert_eq!(
        a.vector_argument_registers(),
        [xmm(0), xmm(1), xmm(2), xmm(3)]
    );
    assert_eq!(a.return_registers(), [RAX]);
    assert_eq!(a.home_space(), 32);
    assert_eq!(a.red_zone(), 0);
    assert_eq!(a.stack_alignment(), 16);
    assert_eq!(a.return_address(), ReturnAddress::Stack);
    assert_eq!(a.frame_pointer(), Some(RBP));
}

#[test]
fn sysv_x64() {
    use regs::x64::*;
    for os in [Os::Linux, Os::Android, Os::MacOs, Os::Ios] {
        let a = abi(Arch::X64, os);
        assert_eq!(
            preserved(a, Preserved::Whole),
            [RBX, RBP, RSP, R12, R13, R14, R15]
        );
        for r in [RAX, RCX, RDX, RSI, RDI, R8, R9, R10, R11] {
            assert!(a.is_volatile(r), "{r}");
        }
        for n in 0..32 {
            assert!(a.is_volatile(xmm(n)), "xmm{n}");
        }
        assert_eq!(a.argument_registers(), [RDI, RSI, RDX, RCX, R8, R9]);
        assert_eq!(
            a.vector_argument_registers(),
            (0..8).map(xmm).collect::<Vec<_>>()
        );
        assert_eq!(a.return_registers(), [RAX, RDX]);
        assert_eq!(a.red_zone(), 128);
        assert_eq!(a.home_space(), 0);
    }
}

#[test]
fn x86() {
    use regs::x86::*;
    for os in OSES {
        let a = abi(Arch::X86, os);
        assert_eq!(preserved(a, Preserved::Whole), [EBX, ESP, EBP, ESI, EDI]);
        assert_eq!(preserved(a, Preserved::Constant), [ES, CS, SS, DS, FS, GS]);
        for r in [EAX, ECX, EDX, EIP, EFLAGS] {
            assert!(a.is_volatile(r), "{r}");
        }
        assert!(a.argument_registers().is_empty());
        assert_eq!(a.return_registers(), [EAX, EDX]);
        assert_eq!(a.red_zone(), 0);
        assert_eq!(a.frame_pointer(), Some(EBP));
        let align = if os == Os::Windows { 4 } else { 16 };
        assert_eq!(a.stack_alignment(), align, "{os:?}");
    }
    assert_eq!(X86Call::Cdecl.argument_registers(), []);
    assert_eq!(X86Call::Fastcall.argument_registers(), [ECX, EDX]);
    assert_eq!(X86Call::Thiscall.argument_registers(), [ECX]);
    assert_eq!(X86Call::Vectorcall.argument_registers(), [ECX, EDX]);
    assert!(!X86Call::Cdecl.callee_pops());
    assert!(X86Call::Stdcall.callee_pops());
}

#[test]
fn arm64() {
    use regs::arm64::*;
    for os in OSES {
        let a = abi(Arch::Arm64, os);
        let mut whole: Vec<u16> = (X19..=X28).collect();
        whole.extend([FP, SP]);
        if os == Os::Android {
            whole.push(X18);
            whole.sort_unstable();
        }
        assert_eq!(preserved(a, Preserved::Whole), whole, "{os:?}");
        assert_eq!(
            preserved(a, Preserved::Low64),
            (8..16).map(v).collect::<Vec<_>>(),
            "{os:?}"
        );
        let constant: &[u16] = if os == Os::Windows { &[X18] } else { &[] };
        assert_eq!(preserved(a, Preserved::Constant), constant, "{os:?}");
        for r in (0..18).chain([LR, PC]) {
            assert!(a.is_volatile(r), "x{r}");
        }
        assert_eq!(a.is_volatile(X18), !matches!(os, Os::Windows | Os::Android));
        assert_eq!(a.argument_registers(), (0..8).collect::<Vec<_>>());
        assert_eq!(
            a.vector_argument_registers(),
            (0..8).map(v).collect::<Vec<_>>()
        );
        assert_eq!(a.return_registers(), [0, 1]);
        assert_eq!(a.indirect_result_register(), Some(X8));
        assert_eq!(a.return_address(), ReturnAddress::Register(LR));
        assert_eq!(a.frame_pointer(), Some(FP));
        assert_eq!(a.red_zone(), if os.is_darwin() { 128 } else { 0 }, "{os:?}");
        assert_eq!(a.frame_records_required(), os.is_darwin());
    }
}

#[test]
fn arm() {
    use regs::arm::*;
    for os in OSES {
        let a = abi(Arch::Arm, os);
        let mut whole: Vec<u16> = (4..12).filter(|&r| !(os.is_darwin() && r == R9)).collect();
        whole.push(SP);
        whole.extend((8..16).map(d));
        assert_eq!(preserved(a, Preserved::Whole), whole, "{os:?}");
        for r in [0, 1, 2, 3, IP, LR, PC] {
            assert!(a.is_volatile(r), "r{r}");
        }
        assert_eq!(a.argument_registers(), [0, 1, 2, 3]);
        let hard_float = matches!(os, Os::Windows | Os::Linux);
        assert_eq!(
            a.vector_argument_registers().len(),
            if hard_float { 8 } else { 0 }
        );
        assert_eq!(a.stack_alignment(), 8);
        assert_eq!(a.red_zone(), 0);
        let fp = match os {
            Os::Windows => Some(11),
            Os::MacOs | Os::Ios => Some(7),
            _ => None,
        };
        assert_eq!(a.frame_pointer(), fp, "{os:?}");
    }
}

/// What holds for every convention.
#[test]
fn invariants() {
    for arch in ARCHES {
        for os in OSES {
            let a = abi(arch, os);
            let what = format!("{arch} {os:?}");
            let registers = a.registers();
            // Each register once.
            let mut sorted = registers.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), registers.len(), "{what}");
            // The stack pointer survives; the PC doesn't.
            assert_eq!(a.preserved(a.stack_pointer()), Preserved::Whole, "{what}");
            assert_eq!(a.preserved(a.program_counter()), Preserved::Not, "{what}");
            assert!(registers.contains(&a.stack_pointer()), "{what}");
            assert!(registers.contains(&a.program_counter()), "{what}");
            // Arguments and results are volatile, and tracked.
            for &r in a
                .argument_registers()
                .iter()
                .chain(a.vector_argument_registers())
                .chain(a.return_registers())
            {
                assert!(a.is_volatile(r), "{what}: {r}");
                assert!(registers.contains(&r), "{what}: {r}");
            }
            // A link register is volatile: a call writes it.
            if let ReturnAddress::Register(lr) = a.return_address() {
                assert!(a.is_volatile(lr), "{what}");
            }
            // A register not tracked is never taken to survive.
            for r in 0..600u16 {
                if !registers.contains(&r) {
                    assert_eq!(a.preserved(r), Preserved::Not, "{what}: {r}");
                }
            }
            if let Some(fp) = a.frame_pointer() {
                assert_eq!(a.preserved(fp), Preserved::Whole, "{what}");
            }
            assert_eq!(a.arch(), arch);
            assert_eq!(a.os(), os);
        }
    }
}

#[test]
fn register_numbers() {
    // As turbonop-dwarf names them (and as the unwinders number them).
    use turbonop_dwarf::registers::name;
    let n = |arch, r| name(arch, r).unwrap();
    assert_eq!(n(Arch::X64, regs::x64::RCX), "rcx");
    assert_eq!(n(Arch::X64, regs::x64::RIP), "rip");
    assert_eq!(n(Arch::X64, regs::x64::xmm(15)), "xmm15");
    assert_eq!(n(Arch::X64, regs::x64::xmm(16)), "xmm16");
    assert_eq!(n(Arch::X64, regs::x64::xmm(31)), "xmm31");
    assert_eq!(n(Arch::X64, regs::x64::RFLAGS), "rflags");
    assert_eq!(n(Arch::X64, regs::x64::GS), "gs");
    assert_eq!(n(Arch::X64, regs::x64::FS_BASE), "fs.base");
    assert_eq!(n(Arch::X64, regs::x64::MXCSR), "mxcsr");
    let encoding = ["rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi"];
    for (i, e) in encoding.iter().enumerate() {
        assert_eq!(n(Arch::X64, regs::x64::BY_ENCODING[i]), *e);
    }
    assert_eq!(n(Arch::X86, regs::x86::ESP), "esp");
    assert_eq!(n(Arch::X86, regs::x86::EIP), "eip");
    assert_eq!(n(Arch::X86, regs::x86::XMM0 + 7), "xmm7");
    assert_eq!(n(Arch::X86, regs::x86::GS), "gs");
    assert_eq!(n(Arch::Arm64, regs::arm64::FP), "fp");
    assert_eq!(n(Arch::Arm64, regs::arm64::SP), "sp");
    assert_eq!(n(Arch::Arm64, regs::arm64::PC), "pc");
    assert_eq!(n(Arch::Arm64, regs::arm64::v(31)), "v31");
    assert_eq!(n(Arch::Arm, regs::arm::SP), "sp");
    assert_eq!(n(Arch::Arm, regs::arm::d(31)), "d31");
    for arch in ARCHES {
        let a = abi(arch, Os::Linux);
        for &r in a.registers() {
            assert!(name(arch, r).is_some(), "{arch} {r}");
        }
        assert_eq!(
            turbonop_dwarf::registers::stack_pointer(arch),
            Some(a.stack_pointer())
        );
    }
}

#[test]
fn stripping_code_addresses() {
    let a = abi(Arch::Arm64, Os::Ios);
    // A PAC in the high bits of a user address.
    assert_eq!(
        a.strip_code_address(0x002d_0001_0000_1234, 47),
        0x0001_0000_1234
    );
    // A tag in the top byte.
    assert_eq!(
        a.strip_code_address(0xf000_7fff_0000_1000, 48),
        0x7fff_0000_1000
    );
    // Kernel addresses: bit 55 set, the rest ones.
    assert_eq!(
        a.strip_code_address(0x12ff_8000_0000_1000, 48),
        0xffff_8000_0000_1000
    );
    // Nothing to strip.
    assert_eq!(a.strip_code_address(0x1000, 48), 0x1000);
    // A width that isn't one.
    assert_eq!(
        a.strip_code_address(0xff00_0000_0000_1000, 0),
        0xff00_0000_0000_1000
    );
    assert_eq!(
        a.strip_code_address(0xff00_0000_0000_1000, 64),
        0xff00_0000_0000_1000
    );
    // Other architectures' addresses are unchanged.
    let x = abi(Arch::X64, Os::Linux);
    assert_eq!(
        x.strip_code_address(0xff00_0000_0000_1000, 48),
        0xff00_0000_0000_1000
    );
}

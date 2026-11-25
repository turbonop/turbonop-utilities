//! Contexts built at the offsets of Windows' `CONTEXT` structures (winnt.h)
//! and Breakpad's `MDRawContextARM`, decoded, and read as registers.

use turbonop_abi::{Os, regs};
use turbonop_core::Arch;
use turbonop_minidump::context::{
    BREAKPAD_ARM, CONTEXT_AMD64, CONTEXT_ARM, CONTEXT_ARM64, CONTEXT_X86, Context,
};
use turbonop_minidump::{cpu, platform};
use turbonop_target::context::{
    ARM_FLOATING_POINT, CONTROL, FLOATING_POINT, INTEGER, SEGMENTS, registers,
};
use turbonop_target::{Registers, os};

fn put32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn read(raw: &[u8], architecture: u16) -> (Arch, Registers) {
    registers(&Context::parse(raw, Some(architecture))).unwrap()
}

/// An x86 `CONTEXT` with each register distinct.
fn x86(flags: u32) -> Vec<u8> {
    let mut b = vec![0u8; 716];
    put32(&mut b, 0, flags);
    // GS, FS, ES, DS; EDI, ESI, EBX, EDX, ECX, EAX; EBP, EIP, CS, EFLAGS,
    // ESP, SS.
    for (i, at) in (0x8C..=0xC8).step_by(4).enumerate() {
        put32(&mut b, at, 0x100 + u32::try_from(i).unwrap());
    }
    b
}

#[test]
fn x86_all() {
    let (arch, r) = read(&x86(CONTEXT_X86 | CONTROL | INTEGER | SEGMENTS), cpu::X86);
    assert_eq!(arch, Arch::X86);
    use regs::x86::*;
    let expect = [
        (GS, 0x100),
        (FS, 0x101),
        (ES, 0x102),
        (DS, 0x103),
        (EDI, 0x104),
        (ESI, 0x105),
        (EBX, 0x106),
        (EDX, 0x107),
        (ECX, 0x108),
        (EAX, 0x109),
        (EBP, 0x10A),
        (EIP, 0x10B),
        (CS, 0x10C),
        (EFLAGS, 0x10D),
        (ESP, 0x10E),
        (SS, 0x10F),
    ];
    for (n, v) in expect {
        assert_eq!(r.get(n), Some(v), "{n}");
    }
    assert_eq!(r.len(), expect.len());
}

#[test]
fn x86_groups() {
    use regs::x86::*;
    let (_, r) = read(&x86(CONTEXT_X86 | CONTROL), cpu::X86);
    let mut known: Vec<u16> = r.iter().map(|(n, _)| n).collect();
    known.sort_unstable();
    assert_eq!(known, [ESP, EBP, EIP, EFLAGS, CS, SS]);
    let (_, r) = read(&x86(CONTEXT_X86 | INTEGER), cpu::X86);
    assert_eq!(r.len(), 6);
    assert_eq!(r.get(EIP), None);
    assert_eq!(r.get(EAX), Some(0x109));
    // No group named: all of them.
    let (_, r) = read(&x86(CONTEXT_X86), cpu::X86);
    assert_eq!(r.len(), 16);
}

/// An x64 `CONTEXT` with each register distinct.
fn amd64(flags: u32) -> Vec<u8> {
    let mut b = vec![0u8; 0x4D0];
    put32(&mut b, 0x30, flags);
    put32(&mut b, 0x34, 0x1F80); // MXCSR
    for (i, at) in (0x38..=0x42).step_by(2).enumerate() {
        put16(&mut b, at, 0x10 + u16::try_from(i).unwrap()); // CS DS ES FS GS SS
    }
    put32(&mut b, 0x44, 0x246); // EFLAGS
    for i in 0..16u64 {
        // RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8..R15
        put64(&mut b, 0x78 + 8 * usize::try_from(i).unwrap(), 0x1000 + i);
    }
    put64(&mut b, 0xF8, 0x7FF6_0000_1234); // RIP
    for i in 0..16usize {
        let v = (u128::from(0xA0u8) + u128::try_from(i).unwrap()) << 64 | 7;
        b[0x1A0 + 16 * i..0x1B0 + 16 * i].copy_from_slice(&v.to_le_bytes());
    }
    b
}

#[test]
fn amd64_all() {
    let all = CONTEXT_AMD64 | CONTROL | INTEGER | SEGMENTS | FLOATING_POINT;
    let (arch, r) = read(&amd64(all), cpu::AMD64);
    assert_eq!(arch, Arch::X64);
    use regs::x64::*;
    for (i, &n) in BY_ENCODING.iter().enumerate() {
        assert_eq!(r.get(n), Some(0x1000 + u128::try_from(i).unwrap()), "{n}");
    }
    assert_eq!(r.get(RSP), Some(0x1004));
    assert_eq!(r.get(RIP), Some(0x7FF6_0000_1234));
    assert_eq!(r.get(RFLAGS), Some(0x246));
    assert_eq!(r.get(MXCSR), Some(0x1F80));
    for (n, v) in [
        (CS, 0x10),
        (DS, 0x11),
        (ES, 0x12),
        (FS, 0x13),
        (GS, 0x14),
        (SS, 0x15),
    ] {
        assert_eq!(r.get(n), Some(v), "{n}");
    }
    for i in 0..16 {
        assert_eq!(
            r.get(xmm(i)),
            Some((0xA0 + u128::from(i)) << 64 | 7),
            "xmm{i}"
        );
    }
    assert_eq!(r.len(), 16 + 1 + 1 + 1 + 6 + 16);
}

#[test]
fn amd64_groups() {
    use regs::x64::*;
    let (_, r) = read(&amd64(CONTEXT_AMD64 | CONTROL), cpu::AMD64);
    let mut known: Vec<u16> = r.iter().map(|(n, _)| n).collect();
    known.sort_unstable();
    assert_eq!(known, [RSP, RIP, RFLAGS, CS, SS]);
    let (_, r) = read(&amd64(CONTEXT_AMD64 | INTEGER), cpu::AMD64);
    // Every general register but RSP.
    assert_eq!(r.len(), 15);
    assert_eq!(r.get(RSP), None);
    assert_eq!(r.get(RBP), Some(0x1005));
    let (_, r) = read(&amd64(CONTEXT_AMD64 | FLOATING_POINT), cpu::AMD64);
    assert_eq!(r.len(), 17);
    // Without the processor architecture: by the flags and size.
    let r2 = registers(&Context::parse(
        &amd64(CONTEXT_AMD64 | FLOATING_POINT),
        None,
    ))
    .unwrap()
    .1;
    assert_eq!(r, r2);
}

/// An ARM64 `CONTEXT` with each register distinct.
fn arm64(flags: u32) -> Vec<u8> {
    let mut b = vec![0u8; 0x390];
    put32(&mut b, 0, flags);
    put32(&mut b, 4, 0x6000_0000); // CPSR
    for i in 0..31u64 {
        put64(&mut b, 8 + 8 * usize::try_from(i).unwrap(), 0x2000 + i);
    }
    put64(&mut b, 0x100, 0x16F0_0000); // SP
    put64(&mut b, 0x108, 0x1_0000_4000); // PC
    for i in 0..32usize {
        let v = u128::from(0xB0u8) + u128::try_from(i).unwrap();
        b[0x110 + 16 * i..0x120 + 16 * i].copy_from_slice(&v.to_le_bytes());
    }
    b
}

#[test]
fn arm64_all() {
    let all = CONTEXT_ARM64 | CONTROL | INTEGER | ARM_FLOATING_POINT;
    let (arch, r) = read(&arm64(all), cpu::ARM64);
    assert_eq!(arch, Arch::Arm64);
    use regs::arm64::*;
    for i in 0..31u16 {
        assert_eq!(r.get(x(i)), Some(0x2000 + u128::from(i)), "x{i}");
    }
    assert_eq!(r.get(SP), Some(0x16F0_0000));
    assert_eq!(r.get(PC), Some(0x1_0000_4000));
    for i in 0..32u16 {
        assert_eq!(r.get(v(i)), Some(0xB0 + u128::from(i)), "v{i}");
    }
    assert_eq!(r.len(), 33 + 32);
}

#[test]
fn arm64_groups() {
    use regs::arm64::*;
    let (_, r) = read(&arm64(CONTEXT_ARM64 | CONTROL), cpu::ARM64);
    let mut known: Vec<u16> = r.iter().map(|(n, _)| n).collect();
    known.sort_unstable();
    assert_eq!(known, [FP, LR, SP, PC]);
    let (_, r) = read(&arm64(CONTEXT_ARM64 | INTEGER), cpu::ARM64);
    assert_eq!(r.len(), 29);
    assert_eq!(r.get(X28), Some(0x2000 + 28));
    assert_eq!(r.get(FP), None);
    let (_, r) = read(&arm64(CONTEXT_ARM64 | ARM_FLOATING_POINT), cpu::ARM64);
    assert_eq!(r.len(), 32);
}

/// A Windows ARM `CONTEXT`, or Breakpad's `MDRawContextARM`, with each
/// register distinct.
fn arm(flags: u32, breakpad: bool) -> Vec<u8> {
    let mut b = vec![0u8; if breakpad { 368 } else { 0x1A0 }];
    put32(&mut b, 0, flags);
    for i in 0..16u32 {
        put32(&mut b, 4 + 4 * usize::try_from(i).unwrap(), 0x3000 + i);
    }
    // The D registers are at 0x50 in both: after Windows' 32-bit FPSCR
    // and its padding, or Breakpad's 64-bit one.
    let d = 0x50;
    for i in 0..32u64 {
        put64(&mut b, d + 8 * usize::try_from(i).unwrap(), 0xD00 + i);
    }
    b
}

#[test]
fn arm_windows_and_breakpad() {
    use regs::arm::*;
    let windows = CONTEXT_ARM | CONTROL | INTEGER | ARM_FLOATING_POINT;
    let breakpad = BREAKPAD_ARM | INTEGER | ARM_FLOATING_POINT;
    for (raw, what) in [
        (arm(windows, false), "Windows"),
        (arm(breakpad, true), "Breakpad"),
    ] {
        let (arch, r) = read(&raw, cpu::ARM);
        assert_eq!(arch, Arch::Arm, "{what}");
        for i in 0..16u16 {
            assert_eq!(
                r.get(self::r(i)),
                Some(0x3000 + u128::from(i)),
                "{what} r{i}"
            );
        }
        for i in 0..32u16 {
            assert_eq!(r.get(d(i)), Some(0xD00 + u128::from(i)), "{what} d{i}");
        }
        assert_eq!(r.len(), 48, "{what}");
    }
    // Windows' control group alone.
    let (_, r) = read(&arm(CONTEXT_ARM | CONTROL, false), cpu::ARM);
    assert_eq!(r.iter().map(|(n, _)| n).collect::<Vec<_>>(), [SP, LR, PC]);
    // Breakpad's integer group alone: every core register.
    let (_, r) = read(&arm(BREAKPAD_ARM | INTEGER, true), cpu::ARM);
    assert_eq!(r.len(), 16);
}

/// `r` the register function (here, where `r` is also a binding).
fn r(n: u16) -> u16 {
    regs::arm::r(n)
}

#[test]
fn unknown_layouts() {
    assert_eq!(registers(&Context::Unknown { flags: 0x8000_0000 }), None);
    // Breakpad's old ARM64 layout.
    let mut b = vec![0u8; 0x390];
    put32(&mut b, 0, 0x8000_0003);
    assert_eq!(registers(&Context::parse(&b, Some(cpu::ARM64_OLD))), None);
}

#[test]
fn operating_systems() {
    assert_eq!(os::of_minidump(platform::WIN32_NT), Some(Os::Windows));
    assert_eq!(os::of_minidump(platform::LINUX), Some(Os::Linux));
    assert_eq!(os::of_minidump(platform::ANDROID), Some(Os::Android));
    assert_eq!(os::of_minidump(platform::MAC_OS_X), Some(Os::MacOs));
    assert_eq!(os::of_minidump(platform::IOS), Some(Os::Ios));
    assert_eq!(os::of_minidump(platform::SOLARIS), None);
    assert_eq!(os::of_minidump(platform::FUCHSIA), None);
    assert_eq!(os::of_minidump(1), None);
}

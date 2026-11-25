//! Access data: its invariants over every form, and hand-picked cases.

use std::collections::BTreeSet;
use turbonop_x86::{
    Flags, Instruction, MemSize, Memory, Mnemonic, Mode, Operand, RegClass, RegSet, Register, Rw,
    decode, form_origin,
};

fn bytes(hex: &str) -> Vec<u8> {
    hex.split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect()
}

fn dec64(hex: &str) -> Instruction {
    decode(&bytes(hex), Mode::Bits64, 0x1000).unwrap()
}

fn dec32(hex: &str) -> Instruction {
    decode(&bytes(hex), Mode::Bits32, 0x1000).unwrap()
}

/// The golden file has every form the enumeration reaches: when the tables
/// change, regenerate it (see `tools/x86access/build.sh`).
#[test]
fn golden_covers_every_form() {
    let mut covered = BTreeSet::new();
    for line in include_str!("../data/access.txt").lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let head = line.split(" | ").next().unwrap();
        let (mode, hex) = head.split_once(' ').unwrap();
        let mode = if mode == "64" {
            Mode::Bits64
        } else {
            Mode::Bits32
        };
        if let Ok(i) = decode(&bytes(hex), mode, 0x1000) {
            covered.insert(i.form());
        }
    }
    let missing: Vec<&str> = crate::forms::found(Mode::Bits64)
        .keys()
        .chain(crate::forms::found(Mode::Bits32).keys())
        .filter(|f| !covered.contains(*f))
        .map(|&f| form_origin(f).unwrap())
        .collect();
    assert!(
        missing.is_empty(),
        "{} forms have no golden sample (regenerate tests/data/access.txt):\n{}",
        missing.len(),
        missing.join("\n")
    );
}

/// Each form's register and memory samples, in both modes.
fn samples() -> Vec<Instruction> {
    let mut out = Vec::new();
    for mode in [Mode::Bits64, Mode::Bits32] {
        for f in crate::forms::found(mode).values() {
            for (_, b) in [&f.reg, &f.mem].into_iter().flatten() {
                out.push(decode(b, mode, 0x1000).unwrap());
            }
        }
    }
    out
}

/// Mnemonics (or patterns, with one `*`) whose first operand isn't
/// written.
const NO_DESTINATION: &[&str] = &[
    "cmp",
    "test",
    "bt",
    "push*",
    "jmp*",
    "call",
    "loop*",
    "out",
    "outs",
    "bound",
    "bndc*",
    "ptest",
    "vptest",
    "vtestp*",
    "comis*",
    "ucomis*",
    "vcomis*",
    "vucomis*",
    "vcomx*",
    "vucomx*",
    "kortest*",
    "ktest*",
    "pcmpestri",
    "pcmpestrm",
    "pcmpistri",
    "pcmpistrm",
    "vpcmpestri",
    "vpcmpestrm",
    "vpcmpistri",
    "vpcmpistrm",
    "ret*",
    "int",
    "monitor*",
    "mwait",
    "umonitor",
    "umwait",
    "tpause",
    "invlpg*",
    "prefetch*",
    "clflush*",
    "clwb",
    "cldemote",
    "nop",
    "lgdt",
    "lidt",
    "lldt",
    "ltr",
    "lmsw",
    "lkgs",
    "verr",
    "verw",
    "ptwrite",
    "senduipi",
    "wrfsbase",
    "wrgsbase",
    "ccmp*",
    "ctest*",
    "fcom*",
    "ficom*",
    "fucom*",
    "fld",
    "fild",
    "fbld",
    "fiadd",
    "fimul",
    "fisub",
    "fisubr",
    "fidiv",
    "fidivr",
    "fldcw",
    "fldenv",
    "frstor",
    "fxrstor*",
    "xrstor*",
    "ldmxcsr",
    "vldmxcsr",
    "ldtilecfg",
    "incssp*",
    "vmclear",
    "vmptrld",
    "vmxon",
    "vmwrite",
    "invept",
    "invvpid",
    "invpcid",
    "enqcmd*",
    "movdir64b",
    "maskmov*",
    "vmaskmovdqu",
    "hreset",
    "vmrun",
    "vmload",
    "vmsave",
    "rmp*",
    "pvalidate",
    "psmash",
    "skinit",
    "jmpe",
    "jmpabs",
    "uwrmsr",
    "wrmsrns",
    "loadiwkey",
    "aesencwide*",
    "aesdecwide*",
    "vgatherpf*",
    "vscatterpf*",
    "ffree*",
    "j*",
    "fadd",
    "fmul",
    "fsub",
    "fsubr",
    "fdiv",
    "fdivr",
    "xabort",
    "xbegin",
    "bndcl",
    "bndcn",
    "bndcu",
    "tilestored",
    "cmps",
    "lods",
    "scas",
    "xlat",
];

fn matches_any(list: &[&str], name: &str) -> bool {
    list.iter().any(|m| match m.split_once('*') {
        Some((head, tail)) => {
            name.len() >= head.len() + tail.len() && name.starts_with(head) && name.ends_with(tail)
        }
        None => *m == name,
    })
}

#[test]
fn destinations_are_written() {
    let mut wrong = BTreeSet::new();
    for inst in samples() {
        let name = inst.mnemonic().name();
        let Some(first) = inst.operands().first() else {
            continue;
        };
        if !matches!(first, Operand::Reg(_) | Operand::Mem(_)) || matches_any(NO_DESTINATION, name)
        {
            continue;
        }
        // One-operand multiplies and divides: the operand is the source.
        if inst.operands().len() == 1 && matches!(name, "mul" | "imul" | "div" | "idiv") {
            continue;
        }
        if !inst.operand_access(0).writes() {
            wrong.insert(form_origin(inst.form()).unwrap());
        }
    }
    assert!(
        wrong.is_empty(),
        "first operands not written:\n{}",
        wrong.into_iter().collect::<Vec<_>>().join("\n")
    );
}

#[test]
fn moves_write_their_destination_and_read_their_source() {
    let mut checked = 0;
    for inst in samples() {
        let movelike = matches!(
            inst.mnemonic().name(),
            "mov"
                | "movzx"
                | "movsx"
                | "movsxd"
                | "movaps"
                | "movups"
                | "movapd"
                | "movupd"
                | "movdqa"
                | "movdqu"
                | "vmovaps"
                | "vmovups"
                | "vmovdqa"
                | "vmovdqu"
                | "vmovdqa32"
                | "vmovdqa64"
                | "vmovdqu8"
                | "vmovdqu16"
                | "vmovdqu32"
                | "vmovdqu64"
                | "movbe"
                | "movnti"
        );
        if !movelike {
            continue;
        }
        checked += 1;
        let what = form_origin(inst.form()).unwrap();
        let first = inst.operand_access(0);
        // (Merge-masking also reads a register destination.)
        let merging = inst.mask().is_some() && !inst.zeroing();
        assert!(
            first == Rw::Write || merging && first == Rw::ReadWrite,
            "{what}: {first:?}"
        );
        assert_eq!(inst.operand_access(1), Rw::Read, "{what}");
    }
    assert!(checked > 50);
}

/// Every operand has an access; only address-only memory operands have
/// none. Accesses list the explicit operands first, in order.
#[test]
fn operands_have_accesses() {
    for inst in samples() {
        let what = form_origin(inst.form()).unwrap();
        for (i, op) in inst.operands().iter().enumerate() {
            if inst.operand_access(i) == Rw::None {
                // (And a multi-byte NOP's operand.)
                let address_only = matches!(op, Operand::Mem(_))
                    && matches!(inst.mnemonic(), Mnemonic::Lea | Mnemonic::Bndmk)
                    || inst.mnemonic() == Mnemonic::Nop;
                assert!(address_only, "{what}: operand {i}");
            }
        }
        let explicit: Vec<Operand> = inst
            .accesses()
            .filter(|a| !a.implicit)
            .map(|a| a.operand)
            .collect();
        let expected: Vec<Operand> = inst
            .operands()
            .iter()
            .copied()
            .filter(|o| matches!(o, Operand::Reg(_) | Operand::Mem(_)))
            .collect();
        assert_eq!(explicit, expected, "{what}");
        // Past the operands, nothing.
        assert_eq!(inst.operand_access(inst.operands().len()), Rw::None);
    }
}

fn reg(class: RegClass, index: u8) -> Register {
    Register::new(class, index)
}

fn gpr(index: u8) -> Register {
    reg(RegClass::Gpr64, index)
}

fn set(regs: &[Register]) -> RegSet {
    let mut s = RegSet::EMPTY;
    for &r in regs {
        s.insert(r);
    }
    s
}

const RAX: u8 = 0;
const RCX: u8 = 1;
const RDX: u8 = 2;
const RBX: u8 = 3;
const RSP: u8 = 4;
const RBP: u8 = 5;
const RSI: u8 = 6;
const RDI: u8 = 7;

fn implicit_memory(i: &Instruction) -> Vec<(Memory, Rw)> {
    i.accesses()
        .filter(|a| a.implicit)
        .filter_map(|a| match a.operand {
            Operand::Mem(m) => Some((m, a.rw)),
            _ => None,
        })
        .collect()
}

#[test]
fn arithmetic() {
    // add rax, rbx
    let i = dec64("48 01 D8");
    assert_eq!(i.operand_access(0), Rw::ReadWrite);
    assert_eq!(i.operand_access(1), Rw::Read);
    assert_eq!(i.written_registers(), set(&[gpr(RAX)]));
    assert_eq!(i.read_registers(), set(&[gpr(RAX), gpr(RBX)]));
    assert_eq!(i.flags().written, Flags::STATUS);
    assert!(i.flags().read.is_empty());
    // adc reads CF.
    assert_eq!(dec64("48 11 D8").flags().read, Flags::CF);
    // xor eax, eax: a write of eax is a write of rax.
    let i = dec64("31 C0");
    assert!(i.written_registers().contains(Register::RAX));
    assert_eq!(i.flags().undefined, Flags::AF);
    // cmp writes only the flags.
    let i = dec64("48 39 D8");
    assert!(i.written_registers().is_empty());
    assert_eq!(i.flags().written, Flags::STATUS);
    // inc leaves CF.
    assert_eq!(
        dec64("FF C0").flags().written,
        Flags(Flags::STATUS.0 & !Flags::CF.0)
    );
}

#[test]
fn multiply_and_divide() {
    // mul rcx: rdx:rax = rax * rcx.
    let i = dec64("48 F7 E1");
    assert_eq!(i.written_registers(), set(&[gpr(RAX), gpr(RDX)]));
    assert_eq!(i.read_registers(), set(&[gpr(RAX), gpr(RCX)]));
    // mul cl: ax = al * cl.
    let i = dec64("F6 E1");
    assert_eq!(i.written_registers(), set(&[gpr(RAX)]));
    let ax = Operand::Reg(reg(RegClass::Gpr16, 0));
    assert!(i.accesses().any(|a| a.operand == ax && a.rw.writes()));
    // div rcx reads and writes rdx:rax; the flags are undefined.
    let i = dec64("48 F7 F1");
    assert_eq!(i.read_registers(), set(&[gpr(RAX), gpr(RCX), gpr(RDX)]));
    assert_eq!(i.written_registers(), set(&[gpr(RAX), gpr(RDX)]));
    assert_eq!(i.flags().undefined, Flags::STATUS);
    // imul rax, rcx, 5: no rdx.
    assert_eq!(dec64("48 6B C1 05").written_registers(), set(&[gpr(RAX)]));
    // mulx rax, rcx, rbx: rdx is the implicit source.
    let i = dec64("C4 E2 F3 F6 C3");
    assert_eq!(i.written_registers(), set(&[gpr(RAX), gpr(RCX)]));
    assert!(i.read_registers().contains(gpr(RDX)));
    // cqo
    let i = dec64("48 99");
    assert_eq!(i.written_registers(), set(&[gpr(RDX)]));
    assert_eq!(i.read_registers(), set(&[gpr(RAX)]));
}

#[test]
fn strings() {
    // movsb: rsi and rdi move; rcx only with rep.
    let i = dec64("A4");
    assert_eq!(i.written_registers(), set(&[gpr(RSI), gpr(RDI)]));
    assert!(i.reads_memory() && i.writes_memory());
    assert_eq!(i.flags().read, Flags::DF);
    let i = dec64("F3 A4");
    assert_eq!(i.written_registers(), set(&[gpr(RCX), gpr(RSI), gpr(RDI)]));
    // rep stosd reads eax; with a 32-bit address, ecx and edi.
    let i = dec64("67 F3 AB");
    assert_eq!(i.written_registers(), set(&[gpr(RCX), gpr(RDI)]));
    assert!(i.read_registers().contains(gpr(RAX)));
    let ecx = Operand::Reg(reg(RegClass::Gpr32, RCX));
    assert!(i.accesses().any(|a| a.operand == ecx));
    assert!(i.writes_memory() && !i.reads_memory());
    // repne scasb writes the flags, not memory.
    let i = dec64("F2 AE");
    assert_eq!(i.flags().written, Flags::STATUS);
    assert!(!i.writes_memory() && i.reads_memory());
    // lodsq writes rax.
    assert_eq!(
        dec64("48 AD").written_registers(),
        set(&[gpr(RAX), gpr(RSI)])
    );
}

#[test]
fn stack() {
    // push rbx writes [rsp-8].
    let i = dec64("53");
    assert_eq!(i.written_registers(), set(&[gpr(RSP)]));
    assert_eq!(i.read_registers(), set(&[gpr(RBX), gpr(RSP)]));
    let mem = implicit_memory(&i);
    assert_eq!(mem.len(), 1);
    assert_eq!(
        (mem[0].0.base, mem[0].0.disp, mem[0].0.size, mem[0].1),
        (Some(Register::RSP), -8, MemSize::Qword, Rw::Write)
    );
    // pop rbx reads [rsp].
    let i = dec64("5B");
    assert_eq!(i.written_registers(), set(&[gpr(RBX), gpr(RSP)]));
    assert_eq!(implicit_memory(&i)[0].0.disp, 0);
    assert!(i.reads_memory() && !i.writes_memory());
    // A 16-bit push.
    let mem = implicit_memory(&dec64("66 53"));
    assert_eq!((mem[0].0.disp, mem[0].0.size), (-2, MemSize::Word));
    // 32-bit code: esp.
    let esp = Operand::Reg(reg(RegClass::Gpr32, RSP));
    assert!(dec32("53").accesses().any(|a| a.operand == esp));
    // call and ret (and the shadow stack pointer, under CET).
    let i = dec64("E8 00 00 00 00");
    assert_eq!(i.written_registers(), set(&[gpr(RSP), Register::SSP]));
    assert!(i.writes_memory());
    let i = dec64("C3");
    assert!(i.reads_memory() && i.written_registers().contains(Register::RSP));
    // leave: rsp = rbp, then pop rbp, from [rbp].
    let i = dec64("C9");
    assert_eq!(i.written_registers(), set(&[gpr(RSP), gpr(RBP)]));
    assert_eq!(implicit_memory(&i)[0].0.base, Some(gpr(RBP)));
    // pushfq reads the flags; popfq writes them.
    assert!(dec64("9C").flags().read.contains(Flags::STATUS | Flags::DF));
    assert!(
        dec64("9D")
            .flags()
            .written
            .contains(Flags::STATUS | Flags::DF | Flags::IF)
    );
    // pusha and popa (32-bit): all eight.
    assert_eq!(dec32("60").read_registers().len(), 8);
    assert_eq!(dec32("61").written_registers().len(), 8);
}

#[test]
fn conditional() {
    // sete al
    let i = dec64("0F 94 C0");
    assert_eq!(i.flags().read, Flags::ZF);
    assert_eq!(i.operand_access(0), Rw::Write);
    // cmovbe eax, ecx: a conditional write.
    let i = dec64("0F 46 C1");
    assert_eq!(i.operand_access(0), Rw::CondWrite);
    assert!(i.operand_access(0).writes() && i.operand_access(0).is_conditional());
    assert_eq!(i.flags().read, Flags::CF | Flags::ZF);
    assert!(i.written_registers().contains(Register::RAX));
    // jg
    assert_eq!(
        dec64("7F 00").flags().read,
        Flags::ZF | Flags::SF | Flags::OF
    );
    // jrcxz reads rcx; loop writes it.
    assert_eq!(dec64("E3 00").read_registers(), set(&[gpr(RCX)]));
    assert_eq!(dec64("E2 00").written_registers(), set(&[gpr(RCX)]));
    // bt writes only CF (and undefined flags).
    let i = dec64("0F A3 C8");
    assert!(i.written_registers().is_empty());
    assert!(i.flags().written.contains(Flags::CF));
}

#[test]
fn exchanges() {
    // cmpxchg [rcx], edx: the accumulator is read and written.
    let i = dec64("0F B1 11");
    assert_eq!(i.operand_access(0), Rw::ReadCondWrite);
    assert_eq!(i.written_registers(), set(&[gpr(RAX)]));
    assert!(i.read_registers().contains(gpr(RDX)) && i.read_registers().contains(gpr(RCX)));
    assert!(i.writes_memory());
    // xadd and xchg write both.
    assert_eq!(
        dec64("0F C1 C8").written_registers(),
        set(&[gpr(RAX), gpr(RCX)])
    );
    assert_eq!(
        dec64("87 C8").written_registers(),
        set(&[gpr(RAX), gpr(RCX)])
    );
    // cmpxchg16b
    let i = dec64("48 0F C7 0E");
    assert_eq!(i.written_registers(), set(&[gpr(RAX), gpr(RDX)]));
    assert_eq!(i.flags().written, Flags::ZF);
}

#[test]
fn system() {
    // cpuid
    let i = dec64("0F A2");
    assert_eq!(
        i.written_registers(),
        set(&[gpr(RAX), gpr(RCX), gpr(RDX), gpr(RBX)])
    );
    // rdtsc
    let i = dec64("0F 31");
    assert_eq!(i.written_registers(), set(&[gpr(RAX), gpr(RDX)]));
    assert!(i.read_registers().contains(Register::MSR));
    // wrmsr
    assert!(dec64("0F 30").written_registers().contains(Register::MSR));
    // syscall: rcx and r11 at least (and the kernel's results).
    let w = dec64("0F 05").written_registers();
    assert!(w.contains(gpr(RCX)) && w.contains(gpr(11)) && w.contains(gpr(RAX)));
    // xgetbv
    assert!(dec64("0F 01 D0").read_registers().contains(Register::XCR0));
    // ldmxcsr [rcx]
    assert_eq!(
        dec64("0F AE 11").written_registers(),
        set(&[Register::MXCSR])
    );
    // cld, stac
    assert_eq!(dec64("FC").flags().written, Flags::DF);
    assert_eq!(dec64("0F 01 CB").flags().written, Flags::AC);
    // xrstor writes (among others) every vector and mask register.
    let w = dec64("0F AE 29").written_registers();
    assert!(w.contains(reg(RegClass::Zmm, 31)) && w.contains(reg(RegClass::Mask, 7)));
}

#[test]
fn address_only() {
    // lea rax, [rbx+rcx*4]: reads the address registers, not memory.
    let i = dec64("48 8D 04 8B");
    assert_eq!(i.operand_access(1), Rw::None);
    assert_eq!(i.read_registers(), set(&[gpr(RBX), gpr(RCX)]));
    assert!(!i.reads_memory());
    // mov [rbx+rcx*4], rax: a store reads its address, not its memory.
    let i = dec64("48 89 04 8B");
    assert_eq!(i.read_registers(), set(&[gpr(RAX), gpr(RBX), gpr(RCX)]));
    assert!(i.writes_memory() && !i.reads_memory());
}

#[test]
fn vector() {
    // vaddps zmm1{k1}, zmm2, zmm3: merge-masking reads the destination.
    let i = dec64("62 F1 6C 49 58 CB");
    assert_eq!(i.operand_access(0), Rw::ReadWrite);
    assert!(i.read_registers().contains(reg(RegClass::Mask, 1)));
    // With {z}, it doesn't.
    assert_eq!(dec64("62 F1 6C C9 58 CB").operand_access(0), Rw::Write);
    // A legacy SSE operation reads its destination; a move doesn't.
    assert_eq!(dec64("0F 58 C1").operand_access(0), Rw::ReadWrite);
    assert_eq!(dec64("0F 28 C1").operand_access(0), Rw::Write);
    // vaddps xmm0, xmm1, xmm2
    let i = dec64("C5 F0 58 C2");
    assert_eq!(i.operand_access(0), Rw::Write);
    assert_eq!(i.written_registers(), set(&[reg(RegClass::Zmm, 0)]));
    // vfmadd231ps accumulates.
    assert_eq!(dec64("C4 E2 71 B8 C2").operand_access(0), Rw::ReadWrite);
    // vzeroupper: sixteen registers, or eight outside 64-bit mode.
    assert_eq!(dec64("C5 F8 77").written_registers().len(), 16);
    assert_eq!(dec32("C5 F8 77").written_registers().len(), 8);
    // Gathers clear their mask.
    let i = dec64("62 F2 7D 49 90 04 88");
    assert!(i.written_registers().contains(reg(RegClass::Mask, 1)));
    // vp2intersectd writes a pair.
    assert_eq!(
        dec64("62 F2 6F 08 68 D3").written_registers(),
        set(&[reg(RegClass::Mask, 2), reg(RegClass::Mask, 3)])
    );
    // comiss writes flags only; pcmpistri writes ecx.
    let i = dec64("0F 2F C1");
    assert!(i.written_registers().is_empty());
    assert_eq!(i.flags().written, Flags::STATUS);
    assert_eq!(
        dec64("66 0F 3A 63 C1 00").written_registers(),
        set(&[gpr(RCX)])
    );
    // maskmovdqu stores at [rdi].
    let i = dec64("66 0F F7 C1");
    assert!(i.writes_memory() && i.read_registers().contains(gpr(RDI)));
    // movdir64b stores at the address in its first operand.
    let mem = implicit_memory(&dec64("66 0F 38 F8 01"));
    assert_eq!((mem[0].0.base, mem[0].1), (Some(gpr(RAX)), Rw::Write));
}

#[test]
fn x87_and_mmx() {
    // fld st(1) pushes: every st(i) is renamed, and they alias the MMX
    // registers.
    let w = dec64("D9 C1").written_registers();
    for n in 0..8 {
        assert!(w.contains(reg(RegClass::St, n)) && w.contains(reg(RegClass::Mmx, n)));
    }
    assert!(w.contains(Register::FPSW));
    // fadd st, st(1) writes st(0) (some MMX register), not st(1).
    let w = dec64("D8 C1").written_registers();
    assert!(w.contains(reg(RegClass::St, 0)) && !w.contains(reg(RegClass::St, 1)));
    assert!(w.contains(reg(RegClass::Mmx, 5)));
    // paddd mm0, mm1: an MMX write changes every st(i).
    assert!(
        dec64("0F FE C1")
            .written_registers()
            .contains(reg(RegClass::St, 7))
    );
    // fcomi writes EFLAGS.
    assert_eq!(dec64("DB F1").flags().written, Flags::STATUS);
    // fnstsw ax
    assert!(dec64("DF E0").written_registers().contains(Register::RAX));
    // fcmovb reads CF.
    assert_eq!(dec64("DA C1").flags().read, Flags::CF);
}

#[test]
fn apx() {
    // {nf} add ebx, eax: no flags.
    let i = dec64("62 F4 7C 0C 01 C3");
    assert!(i.no_flags());
    assert!(i.flags().written.is_empty());
    assert_eq!(i.operand_access(0), Rw::ReadWrite);
    // add ecx, ebx, eax: a new destination, written; the rest read.
    let i = dec64("62 F4 74 18 01 C3");
    assert_eq!(
        (0..3).map(|n| i.operand_access(n)).collect::<Vec<_>>(),
        [Rw::Write, Rw::Read, Rw::Read]
    );
    assert_eq!(i.written_registers(), set(&[gpr(RCX)]));
    assert_eq!(i.flags().written, Flags::STATUS);
}

#[test]
fn interrupts() {
    // int 0x80 is a system call: every general-purpose register may change.
    assert_eq!(dec32("CD 80").written_registers().len(), 8);
    // int3 (a debugger's) doesn't.
    assert!(dec64("CC").written_registers().is_empty());
}

#[test]
fn reg_set() {
    let mut s = RegSet::EMPTY;
    assert!(s.is_empty());
    s.insert(reg(RegClass::Gpr8High, 0)); // ah
    s.insert(reg(RegClass::Xmm, 3));
    s.insert(Register::MXCSR);
    assert!(s.contains(Register::RAX) && s.contains(reg(RegClass::Gpr8, 0)));
    assert!(s.contains(reg(RegClass::Ymm, 3)) && s.contains(reg(RegClass::Zmm, 3)));
    assert_eq!(
        s.iter().collect::<Vec<_>>(),
        [Register::RAX, reg(RegClass::Zmm, 3), Register::MXCSR]
    );
    assert_eq!(s.len(), 3);
    let t = set(&[Register::RAX, gpr(RCX)]);
    assert_eq!(s.intersection(&t), set(&[Register::RAX]));
    assert_eq!(s.union(&t).len(), 4);
    assert_eq!(format!("{:?}", set(&[gpr(RCX)])), "{rcx}");
}

//! Hand-checked reads and writes of representative instructions (from
//! the Arm ARM, not from LLVM): what compilers emit most, and the cases a
//! write set is easiest to get wrong (writeback, pairs, status registers,
//! LR, NZCV, FFR, the vector state).

use turbonop_arm64::{Effects, Reg, RegSet};

fn set(list: &str) -> RegSet {
    list.split_whitespace()
        .map(|name| {
            let num = |p: &str| name.strip_prefix(p).and_then(|n| n.parse::<u8>().ok());
            if let Some(n) = num("X") {
                return Reg::X(n);
            }
            if let Some(n) = num("V") {
                return Reg::V(n);
            }
            if let Some(n) = num("P") {
                return Reg::P(n);
            }
            match name {
                "SP" => Reg::Sp,
                "LR" => Reg::X(30),
                "NZCV" => Reg::Nzcv,
                "FPCR" => Reg::Fpcr,
                "FPSR" => Reg::Fpsr,
                "FFR" => Reg::Ffr,
                "ZA" => Reg::Za,
                "ZT0" => Reg::Zt0,
                "SVCR" => Reg::Svcr,
                "SYS" => Reg::SysRegs,
                _ => panic!("{name}"),
            }
        })
        .collect()
}

/// (word, what it is, writes, reads, memory: "", "r", "w", or "rw").
const CASES: &[(u32, &str, &str, &str, &str)] = &[
    (0xB100_0420, "adds x0, x1, #1", "X0 NZCV", "X1", ""),
    (0x0B02_0020, "add w0, w1, w2", "X0", "X1 X2", ""),
    (0xF100_043F, "cmp x1, #1", "NZCV", "X1", ""),
    (0x7200_1C7F, "tst w3, #0xff", "NZCV", "X3", ""),
    (0xA9C1_07E0, "ldp x0, x1, [sp, #16]!", "X0 X1 SP", "SP", "r"),
    (0xA8FE_0C82, "ldp x2, x3, [x4], #-32", "X2 X3 X4", "X4", "r"),
    (
        0xA9BF_7BFD,
        "stp x29, x30, [sp, #-16]!",
        "SP",
        "X29 X30 SP",
        "w",
    ),
    (0xB800_4420, "str w0, [x1], #4", "X1", "X0 X1", "w"),
    (0xF862_7820, "ldr x0, [x1, x2, lsl #3]", "X0", "X1 X2", "r"),
    (0xB867_D8C5, "ldr w5, [x6, w7, sxtw #2]", "X5", "X6 X7", "r"),
    (0x5800_0200, "ldr x0, #64 (literal)", "X0", "", "r"),
    (0xB820_0041, "ldadd w0, w1, [x2]", "X1", "X0 X2", "rw"),
    (0xB820_005F, "stadd w0, [x2]", "", "X0 X2", "rw"),
    (0xC8A0_7C41, "cas x0, x1, [x2]", "X0", "X0 X1 X2", "rw"),
    (
        0x4820_7C82,
        "casp x0, x1, x2, x3, [x4]",
        "X0 X1",
        "X0 X1 X2 X3 X4",
        "rw",
    ),
    (0xF825_80E6, "swp x5, x6, [x7]", "X6", "X5 X7", "rw"),
    (0xC85F_7C41, "ldxr x1, [x2]", "X1", "X2", "r"),
    (
        0xC803_7C20,
        "stxr w3, x0, [x1] (the status register)",
        "X3",
        "X0 X1",
        "rw",
    ),
    (0xC87F_8440, "ldaxp x0, x1, [x2]", "X0 X1", "X2", "r"),
    // (Calls read SP: LLVM's, for the call's stack.)
    (0x9400_0040, "bl #0x100", "LR", "SP", ""),
    (0xD63F_0100, "blr x8", "LR", "X8 SP", ""),
    (0xD65F_03C0, "ret", "", "LR", ""),
    (0xD61F_0200, "br x16", "", "X16", ""),
    (0x3400_0040, "cbz w0, #8", "", "X0", ""),
    (0xB608_0041, "tbz x1, #33, #8", "", "X1", ""),
    (0x9A82_0020, "csel x0, x1, x2, eq", "X0", "X1 X2 NZCV", ""),
    (0x1A82_1420, "csinc w0, w1, w2, ne", "X0", "X1 X2 NZCV", ""),
    (0xFA43_1800, "ccmp x0, #3, #0, ne", "NZCV", "X0 NZCV", ""),
    (0xD53B_4200, "mrs x0, nzcv", "X0", "NZCV", ""),
    (0xD51B_4200, "msr nzcv, x0", "NZCV", "X0", ""),
    (0xD51B_4401, "msr fpcr, x1", "FPCR", "X1", ""),
    (0x9E67_0020, "fmov d0, x1", "V0", "X1", ""),
    (0x1E22_2820, "fadd s0, s1, s2", "V0 FPSR", "V1 V2 FPCR", ""),
    (0x1E61_2000, "fcmp d0, d1", "NZCV FPSR", "V0 V1 FPCR", ""),
    (0x4EA1_1C20, "mov v0.16b, v1.16b", "V0", "V1", ""),
    (
        0x4E0C_1C20,
        "mov v0.s[1], w1 (a lane: the rest is kept)",
        "V0",
        "V0 X1",
        "",
    ),
    (
        0x04C0_0020,
        "add z0.d, p0/m, z0.d, z1.d",
        "V0",
        "V0 V1 P0",
        "",
    ),
    (
        0xA401_6000,
        "ldff1b {z0.b}, p0/z, [x0, x1]",
        "V0 FFR",
        "P0 X0 X1 FFR",
        "r",
    ),
    (0x2550_C020, "ptest p0, p1.b", "NZCV", "P0 P1", ""),
    (0x25A1_1C00, "whilelo p0.s, x0, x1", "P0 NZCV", "X0 X1", ""),
    (
        0x4CDF_A000,
        "ld1 {v0.16b, v1.16b}, [x0], #32",
        "V0 V1 X0",
        "X0",
        "r",
    ),
    (0x9B02_7C20, "mul x0, x1, x2", "X0", "X1 X2", ""),
    (0x9B02_0C20, "madd x0, x1, x2, x3", "X0", "X1 X2 X3", ""),
    (0xB000_0000, "adrp x0, #4096", "X0", "", ""),
    (0xD503_233F, "paciasp", "LR", "LR SP", ""),
    (0xD503_201F, "nop", "", "", ""),
    (
        0xF83F_D020,
        "ld64b x0, [x1]",
        "X0 X1 X2 X3 X4 X5 X6 X7",
        "X1",
        "r",
    ),
    (
        0x1901_0440,
        "cpyfp [x0]!, [x1]!, x2!",
        "X0 X1 X2 NZCV",
        "X0 X1 X2",
        "rw",
    ),
];

#[test]
fn cases() {
    let mut failures = Vec::new();
    for &(word, what, writes, reads, mem) in CASES {
        let inst = turbonop_arm64::decode(word, 0x1000).unwrap_or_else(|| panic!("{what}"));
        let (w, r) = (inst.written_registers(), inst.read_registers());
        let m = match (inst.reads_memory(), inst.writes_memory()) {
            (true, true) => "rw",
            (true, false) => "r",
            (false, true) => "w",
            (false, false) => "",
        };
        if w != set(writes) || r != set(reads) || m != mem {
            failures.push(format!(
                "{what} ({}): writes {:?} reads {:?} memory {m:?}",
                turbonop_arm64::format(&inst),
                w.iter().collect::<Vec<_>>(),
                r.iter().collect::<Vec<_>>()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn effects() {
    let e = |w: u32| turbonop_arm64::decode(w, 0).unwrap().effects();
    assert!(e(0x9400_0040).contains(Effects::CALL));
    assert!(e(0xD63F_0100).contains(Effects::CALL | Effects::INDIRECT));
    assert!(e(0xD65F_03C0).contains(Effects::RETURN | Effects::NO_FALLTHROUGH));
    assert!(e(0xD61F_0200).contains(Effects::BRANCH | Effects::INDIRECT));
    assert!(e(0x3400_0040).contains(Effects::BRANCH));
    assert!(!e(0x3400_0040).contains(Effects::NO_FALLTHROUGH));
    assert!(e(0xD400_0001).contains(Effects::EXCEPTION));
    assert!(e(0x1E22_2820).contains(Effects::FP_EXCEPTION));
    assert_eq!(e(0x0B02_0020), Effects::NONE);
}

#[test]
fn system_state() {
    let inst = |w: u32| turbonop_arm64::decode(w, 0).unwrap();
    // MSR SVCR, as SMSTART and SMSTOP, zeroes the SVE and SME state.
    let w = inst(0xD51B_4240).written_registers();
    for r in [
        Reg::Z(0),
        Reg::Z(31),
        Reg::P(15),
        Reg::Ffr,
        Reg::Za,
        Reg::Zt0,
        Reg::Svcr,
    ] {
        assert!(w.contains(r), "msr svcr: {r:?}");
    }
    // DC ZVA writes memory.
    assert!(inst(0xD50B_7420).writes_memory());
    // W, S, and Q writes are of X, V (and Z).
    let w = inst(0x0B02_0020).written_registers();
    assert!(w.contains(Reg::W(0)) && w.contains(Reg::X(0)));
    let w = inst(0x1E22_2820).written_registers();
    assert!(w.contains(Reg::S(0)) && w.contains(Reg::Q(0)) && w.contains(Reg::Z(0)));
    // The zero register is never written or read.
    let cmp = inst(0xF100_043F);
    assert!(!cmp.written_registers().contains(Reg::Xzr));
    assert_eq!(cmp.written_registers().len(), 1);
}

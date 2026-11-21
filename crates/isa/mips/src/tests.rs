//! Hermetic tests: a few encodings of each kind, as llvm-objdump writes
//! them.

use alloc::string::String;
use alloc::vec::Vec;

use crate::format::format;
use crate::{Features, branch_target, decode, has_delay_slot};

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `w`, as LLVM writes it at 0x1000 with `features` on, both byte orders
/// alike.
fn text(w: u32, features: &Features) -> String {
    let be = decode(&w.to_be_bytes(), true, features).map(|i| norm(&format(&i, 0x1000, features)));
    let le = decode(&w.to_le_bytes(), false, features).map(|i| norm(&format(&i, 0x1000, features)));
    assert_eq!(be, le, "{w:08x}");
    be.unwrap_or_else(|| panic!("{w:08x}: doesn't decode"))
}

#[test]
fn base_and_aliases() {
    let f = Features::of(&["mips32", "mips32r2"]);
    assert_eq!(text(0x27bd_ffe0, &f), "addiu $sp, $sp, -0x20");
    assert_eq!(text(0x8fa4_0008, &f), "lw $4, 0x8($sp)");
    assert_eq!(text(0x0060_1025, &f), "move $2, $3");
    assert_eq!(text(0x03e0_0008, &f), "jr $ra");
    assert_eq!(text(0x0320_f809, &f), "jalr $25");
    assert_eq!(text(0x1000_0003, &f), "b 0x1010");
    assert_eq!(text(0x1080_0001, &f), "beqz $4, 0x1008");
    assert_eq!(text(0x0c10_0040, &f), "jal 0x400100");
    assert_eq!(text(0x4002_6000, &f), "mfc0 $2, $12, 0");
    assert_eq!(text(0x7c62_3900, &f), "ext $2, $3, 0x4, 0x8");
}

/// `rdhwr`, inside `.set push` and `.set pop`, as LLVM writes it.
#[test]
fn rdhwr() {
    let f = Features::of(&["mips32", "mips32r2"]);
    assert_eq!(
        text(0x7c03_e83b, &f),
        ".set push .set mips32r2 rdhwr $3, $29 .set pop"
    );
}

/// `dext` and its forms for high bits: all written as `dext`.
#[test]
fn dext() {
    let f = Features::of(&["mips64", "mips64r2"]);
    // dext $2, $3, 4, 8; dextm $2, $3, 4, 40; dextu $2, $3, 36, 8.
    assert_eq!(text(0x7c62_3903, &f), "dext $2, $3, 0x4, 0x8");
    assert_eq!(text(0x7c62_3901, &f), "dext $2, $3, 0x4, 0x28");
    assert_eq!(text(0x7c62_3902, &f), "dext $2, $3, 0x24, 0x8");
}

/// Release 6's compact branches, told apart by their fields; and `blez`,
/// which its group's decoder leaves to MIPS32's table.
#[test]
fn release6_branches() {
    let f = Features::of(&["mips32", "mips32r6"]);
    // (rs >= rt: bovc; rs < rt, rs not 0: beqc; rs 0: beqzalc.)
    assert!(text(0x2083_0000, &f).starts_with("bovc $4, $3,"));
    assert!(text(0x2064_0000, &f).starts_with("beqc $3, $4,"));
    assert!(text(0x2004_0000, &f).starts_with("beqzalc $4,"));
    assert!(text(0x1880_0000, &f).starts_with("blez $4,"));
    let r2 = Features::of(&["mips32", "mips32r2"]);
    assert!(text(0x2083_0000, &r2).starts_with("addi $3, $4,"));
}

/// Delay slots, and branch and jump targets.
#[test]
fn branches() {
    let f = Features::of(&["mips32", "mips32r2"]);
    let i = decode(&0x1085_0005u32.to_be_bytes(), true, &f).unwrap();
    assert!(has_delay_slot(&i));
    assert_eq!(branch_target(&i, 0x1000), Some(0x1018));
    let j = decode(&0x0c10_0040u32.to_be_bytes(), true, &f).unwrap();
    assert_eq!(branch_target(&j, 0x1000_0000), Some(0x1040_0100));
    let r6 = Features::of(&["mips32", "mips32r6"]);
    let c = decode(&0x2064_0000u32.to_be_bytes(), true, &r6).unwrap();
    assert!(!has_delay_slot(&c));
}

#[test]
fn not_decoded() {
    let f = Features::from_elf(0x7000_0000, false);
    assert!(decode(&[0xff; 4], true, &f).is_none());
    assert!(decode(&[0; 3], true, &f).is_none());
    // (microMIPS code isn't decoded.)
    let mm = Features::from_elf(0x7000_0000 | 0x0200_0000, false);
    assert!(decode(&0x27bd_ffe0u32.to_be_bytes(), true, &mm).is_none());
}

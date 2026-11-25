//! Hermetic tests: a few encodings of each kind, as llvm-objdump writes
//! them.

use alloc::string::String;
use alloc::vec::Vec;

use crate::format::format;
use crate::{Features, branch_target, decode, has_delay_slot, is_micromips, size};

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
    // (MIPS16 code isn't decoded.)
    let m16 = Features::from_elf(0x7000_0000 | 0x0400_0000, false);
    assert!(decode(&0x27bd_ffe0u32.to_be_bytes(), true, &m16).is_none());
}

/// microMIPS code (its image's flags say so): 16- and 32-bit
/// instructions, their sizes, delay slots, and branch and jump targets,
/// from the instruction after the branch.
#[test]
fn micromips() {
    let mm = Features::from_elf(0x7000_0000 | 0x0200_0000, false);
    assert!(mm.contains("micromips"));
    // addiusp -0x18 (16 bits), then the next halfword's.
    let i = decode(&[0x4f, 0xf5, 0, 0], true, &mm).unwrap();
    assert_eq!((i.name(), size(&i)), ("ADDIUSP_MM", 2));
    assert!(is_micromips(&i));
    // jals 0x20264 (32 bits, a 16-bit delay slot), from 0x2028e.
    let j = decode(&[0x74, 0x01, 0x01, 0x32], true, &mm).unwrap();
    assert_eq!((j.name(), size(&j)), ("JALS_MM", 4));
    assert!(has_delay_slot(&j));
    assert_eq!(branch_target(&j, 0x2028e), Some(0x20264));
    // Little-endian: each halfword so, the first the high one.
    let le = decode(&[0x01, 0x74, 0x32, 0x01], false, &mm).unwrap();
    assert_eq!(le.name(), "JALS_MM");
    // b16 (16 bits): from the instruction after it, which LLVM's text
    // doesn't count (it writes 0xa for this one, at 4).
    let b = decode(&[0xcc, 0x03], true, &mm).unwrap();
    assert_eq!((b.name(), size(&b)), ("B16_MM", 2));
    assert_eq!(branch_target(&b, 4), Some(0xc));
    assert_eq!(crate::format::format(&b, 4, &mm), "b16\t0xa");
    // Release 6's bc (32 bits): from the instruction after it, too.
    let r6 = Features::from_elf(0x9000_0000 | 0x0200_0000, false);
    let bc = decode(&[0x94, 0x00, 0x00, 0x08], true, &r6).unwrap();
    assert_eq!(bc.name(), "BC_MMR6");
    assert!(!has_delay_slot(&bc));
    assert_eq!(branch_target(&bc, 0), Some(0x14));
    // Too short, or no instruction.
    assert!(decode(&[0x4f], true, &mm).is_none());
    assert!(decode(&[0x74, 0x01], true, &mm).is_none());
}

/// The microMIPS tables number the features as the standard ones do (one
/// `Features` serves both).
#[test]
fn features_alike() {
    let names = |f: &[turbonop_tablegen::Feature]| f.iter().map(|f| f.name).collect::<Vec<_>>();
    assert_eq!(
        names(crate::tables::FEATURES),
        names(crate::micromips::FEATURES)
    );
}

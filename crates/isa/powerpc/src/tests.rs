//! Hermetic tests: a few encodings of each kind, as llvm-objdump writes
//! them.

use alloc::string::String;
use alloc::vec::Vec;

use crate::format::format;
use crate::{Features, branch_target, decode, memory, r};

/// The instruction of `words` (big-endian, or little: `big`), as LLVM
/// writes it at 0x1000.
fn text(words: &[u32], big: bool) -> String {
    let f = Features::standard(true);
    let bytes: Vec<u8> = words
        .iter()
        .flat_map(|w| {
            if big {
                w.to_be_bytes()
            } else {
                w.to_le_bytes()
            }
        })
        .collect();
    let i = decode(&bytes, big, &f).unwrap_or_else(|| panic!("{words:08x?}: doesn't decode"));
    format(&i, 0x1000, &f)
}

fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn base_and_aliases() {
    for big in [true, false] {
        let t = |w: u32| norm(&text(&[w], big));
        assert_eq!(t(0x3863_0001), "addi 3, 3, 1");
        assert_eq!(t(0x3860_ffff), "li 3, -1");
        assert_eq!(t(0x3c80_1234), "lis 4, 4660");
        assert_eq!(t(0x4e80_0020), "blr");
        assert_eq!(t(0x7c08_02a6), "mflr 0");
        assert_eq!(t(0x7c83_2378), "mr 3, 4");
        assert_eq!(t(0x6000_0000), "nop");
        assert_eq!(t(0x2c03_0000), "cmpwi 3, 0");
        assert_eq!(t(0x7fa3_2000), "cmpd 7, 3, 4");
        assert_eq!(t(0x7c64_2a14), "add 3, 4, 5");
    }
}

/// Memory operands: a displacement and its base (r0 as a base is 0), and
/// a base and an index.
#[test]
fn memory_operands() {
    let t = |w: u32| norm(&text(&[w], true));
    assert_eq!(t(0x8061_0008), "lwz 3, 8(1)");
    assert_eq!(t(0xe881_fff0), "ld 4, -16(1)");
    assert_eq!(t(0xf821_ffe1), "stdu 1, -32(1)");
    assert_eq!(t(0x7c64_282e), "lwzx 3, 4, 5");
    assert_eq!(t(0x7c60_282e), "lwzx 3, 0, 5");
    let f = Features::standard(true);
    let i = decode(&0x8061_0008u32.to_be_bytes(), true, &f).unwrap();
    assert_eq!(memory(&i), Some((8, r(1))));
}

/// VSX operands number Altivec's registers 32 to 63; Altivec's own, 0 to
/// 31.
#[test]
fn vector_registers() {
    let t = |w: u32| norm(&text(&[w], false));
    assert_eq!(t(0xf043_1c97), "xxlor 34, 35, 35");
    assert_eq!(t(0xf022_1c90), "xxlor 1, 2, 3");
    assert_eq!(t(0x1043_2000), "vaddubm 2, 3, 4");
    assert_eq!(t(0xfc22_182a), "fadd 1, 2, 3");
}

/// The shifts and cache hints LLVM's printer writes by hand.
#[test]
fn by_hand() {
    let t = |w: u32| norm(&text(&[w], true));
    assert_eq!(t(0x5483_103a), "slwi 3, 4, 2");
    assert_eq!(t(0x5483_f0be), "srwi 3, 4, 2");
    assert_eq!(t(0x7883_1764), "sldi 3, 4, 2");
    assert_eq!(t(0x5483_10c8), "rlwinm 3, 4, 2, 3, 4");
    assert_eq!(t(0x7c03_222c), "dcbt 3, 4");
    assert_eq!(t(0x7e03_222c), "dcbtt 3, 4");
    assert_eq!(t(0x7c03_20ac), "dcbf 3, 4");
}

/// Branches: their targets, relative and absolute.
#[test]
fn branches() {
    let f = Features::standard(true);
    let at = |w: u32, pc: u64| {
        let i = decode(&w.to_be_bytes(), true, &f).unwrap();
        (norm(&format(&i, pc, &f)), branch_target(&i, pc, &f))
    };
    assert_eq!(at(0x4800_0011, 0x18), ("bl 0x28".into(), Some(0x28)));
    assert_eq!(at(0x4bff_fff8, 0x1c), ("b 0x14".into(), Some(0x14)));
    assert_eq!(at(0x4800_0102, 0), ("ba 0x100".into(), Some(0x100)));
    assert_eq!(at(0x4182_0008, 0), ("bt 2, 0x8".into(), Some(8)));
    assert_eq!(at(0x4200_0008, 8), ("bdnz 0x10".into(), Some(0x10)));
    assert_eq!(at(0x4e80_0020, 0), ("blr".into(), None));
}

/// Prefixed instructions: 8 bytes, the prefix first in memory whatever
/// the byte order; not decoded without the prefixed instructions.
#[test]
fn prefixed() {
    for big in [true, false] {
        // pld 3, 8(4), 0; pld 3, 8(0), 1
        assert_eq!(
            norm(&text(&[0x0400_0000, 0xe464_0008], big)),
            "pld 3, 8(4), 0"
        );
        assert_eq!(
            norm(&text(&[0x0410_0000, 0xe460_0008], big)),
            "pld 3, 8(0), 1"
        );
    }
    let mut old = Features::of(&["altivec", "vsx"], true);
    let bytes: Vec<u8> = [0x0400_0000u32, 0xe464_0008]
        .iter()
        .flat_map(|w| w.to_be_bytes())
        .collect();
    let i = decode(&bytes, true, &old);
    assert!(i.is_none_or(|i| i.size == 4));
    old = Features::standard(true);
    assert_eq!(decode(&bytes, true, &old).map(|i| i.size), Some(8));
}

#[test]
fn not_instructions() {
    let f = Features::standard(true);
    assert!(decode(&[0; 4], true, &f).is_none());
    assert!(decode(&[0; 3], true, &f).is_none());
    assert!(decode(&[], false, &f).is_none());
}

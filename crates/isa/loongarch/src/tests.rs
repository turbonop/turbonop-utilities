//! Hermetic tests: a few encodings of each kind, as LLVM writes them.

use alloc::string::String;

use crate::format::format;
use crate::{Features, branch_target, decode};

fn text(w: u32, la64: bool) -> String {
    let f = if la64 {
        Features::la64()
    } else {
        Features::la32()
    };
    let i = decode(&w.to_le_bytes(), &f).unwrap_or_else(|| panic!("{w:08x}: doesn't decode"));
    format(&i, 0x1000, &f)
}

#[test]
fn base_and_aliases() {
    // addi.d $a0, $sp, -16; nop; ret; move $a0, $a1; ori $a0, $zero, 255
    assert_eq!(text(0x02ff_c064, true), "addi.d\t$a0, $sp, -0x10");
    assert_eq!(text(0x0340_0000, true), "nop");
    assert_eq!(text(0x4c00_0020, true), "ret");
    assert_eq!(text(0x0015_00a4, true), "move\t$a0, $a1");
    assert_eq!(text(0x0383_fc04, true), "ori\t$a0, $zero, 0xff");
}

#[test]
fn vectors_and_floats() {
    assert_eq!(text(0x0101_0820, true), "fadd.d\t$fa0, $fa1, $fa2");
    assert_eq!(text(0x700a_0820, true), "vadd.b\t$vr0, $vr1, $vr2");
    assert_eq!(text(0x740a_0820, true), "xvadd.b\t$xr0, $xr1, $xr2");
}

/// Branches: their offsets, as LLVM writes them, and where they go.
#[test]
fn branches() {
    let f = Features::la64();
    // beq $a0, $a1, 16
    let i = decode(&0x5800_1085u32.to_le_bytes(), &f).unwrap();
    assert_eq!(format(&i, 4, &f), "beq\t$a0, $a1, 0x10");
    assert_eq!(branch_target(&i, 4), Some(0x14));
    // b -8
    let i = decode(&0x53ff_fbffu32.to_le_bytes(), &f).unwrap();
    assert_eq!(format(&i, 8, &f), "b\t-0x8");
    assert_eq!(branch_target(&i, 8), Some(0));
}

/// `ud` and `amswap.w` share an encoding: `ud` when rk is 1 and rd is rj.
#[test]
fn ud_or_amswap() {
    assert_eq!(text(0x3860_0421, true), "ud\t0x1");
    assert_eq!(text(0x3860_14c4, true), "amswap.w\t$a0, $a1, $a2");
    assert_eq!(text(0x3860_04c4, true), "amswap.w\t$a0, $ra, $a2");
}

/// LA64's own instructions don't decode as LA32's.
#[test]
fn la32() {
    // ld.d $a0, $a1, 8
    let w = 0x28c0_20a4u32;
    assert!(decode(&w.to_le_bytes(), &Features::la32()).is_none());
    assert_eq!(text(w, true), "ld.d\t$a0, $a1, 0x8");
}

#[test]
fn not_instructions() {
    let f = Features::la64();
    assert!(decode(&[0xff; 4], &f).is_none());
    assert!(decode(&[0; 3], &f).is_none());
    assert!(decode(&0u32.to_le_bytes(), &f).is_none());
}
